use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
    time::Instant,
};

use cargo_metadata::{CargoOpt, MetadataCommand};
use crates_index::SparseIndex;
use semver::VersionReq;
use thiserror::Error;

use crate::{
    RegistryCachePolicy, State,
    package::{Package, PackageVersion, Unit, Usage},
    registry::RegistryError,
};

mod fetch;
mod resolve;
mod update;

use resolve::*;
use update::*;

#[derive(Debug, Error)]
pub enum CargoError {
    #[error("failed to get cargo metadata: {0}")]
    Metadata(String),
    #[error("failed to access crates.io index: {0}")]
    Index(#[from] crates_index::Error),
    #[error("HTTP request failed: {0}")]
    Http(#[from] http::Error),
    #[error("curl error: {0}")]
    Curl(#[from] curl::Error),
    #[error("curl multi error: {0}")]
    CurlMulti(#[from] curl::MultiError),
    #[error("manifest is not writable: {0}")]
    ReadOnly(PathBuf),
}

pub struct CargoRegistry {
    state: Rc<State>,
}

impl CargoRegistry {
    pub fn new(state: Rc<State>) -> Self {
        Self { state }
    }
}

impl super::RegistryImpl for CargoRegistry {
    fn packages(&self) -> Result<impl IntoIterator<Item = Package>, RegistryError> {
        let total_start = Instant::now();

        let index = SparseIndex::new_cargo_default().map_err(CargoError::from)?;

        let metadata_cmd = || {
            let mut cmd = MetadataCommand::new();
            if let Some(root) = self.state.root() {
                cmd.current_dir(root);
            }
            cmd
        };

        let metadata_start = Instant::now();
        let metadata = metadata_cmd()
            .features(CargoOpt::AllFeatures)
            .exec()
            .or_else(|_| {
                metadata_cmd()
                    .features(CargoOpt::SomeFeatures(vec![]))
                    .exec()
            })
            .or_else(|_| metadata_cmd().exec())
            .map_err(|e| CargoError::Metadata(e.to_string()))?;
        log::debug!(
            "cargo.packages: cargo metadata resolution took {:?}",
            metadata_start.elapsed()
        );

        let registry_start = Instant::now();
        let members = workspace_members(&metadata);
        let names = collect_crates_io_deps(&members);

        let mut versions: HashMap<String, Vec<PackageVersion>> = HashMap::new();
        let mut requests: Vec<(String, http::Request<()>)> = Vec::new();
        let mut cache_hits = 0usize;
        let policy = self.state.registry_cache_policy();
        let use_local_cache = matches!(policy, RegistryCachePolicy::PreferLocal);

        for name in &names {
            if use_local_cache {
                match index.crate_from_cache(name) {
                    Ok(krate) => {
                        versions.insert(name.clone(), versions_from_crate(&krate));
                        cache_hits += 1;
                        continue;
                    }
                    Err(err) => {
                        log::debug!("cargo.packages: cache miss for '{name}': {err}");
                    }
                }
            }

            let request = index
                .make_cache_request(name)
                .map_err(|e| e.to_string())
                .and_then(|b| b.body(()).map_err(|e| e.to_string()));

            match request {
                Ok(req) => requests.push((name.clone(), req)),
                Err(e) => {
                    log::warn!("failed to build cache request for '{name}': {e}");
                }
            }
        }

        log::debug!(
            "cargo.packages: cache policy {:?}, cache hits {}, network requests {}, registry resolve took {:?}",
            policy,
            cache_hits,
            requests.len(),
            registry_start.elapsed()
        );

        if !requests.is_empty() {
            let mut responses = fetch::fetch_all(self.state.multi(), requests);

            for name in names {
                if versions.contains_key(&name) {
                    continue;
                }

                let response = match responses.remove(&name) {
                    Some(Ok(r)) => r,
                    Some(Err(e)) => {
                        log::warn!("failed to fetch index for '{name}': {e}");
                        continue;
                    }
                    None => continue,
                };

                let write_cache_entry = !matches!(policy, RegistryCachePolicy::NoCache);
                match parse_versions(&index, &name, response, write_cache_entry) {
                    Ok(v) => {
                        versions.insert(name, v);
                    }
                    Err(e) => {
                        log::warn!("failed to parse versions for '{name}': {e}");
                    }
                }
            }
        }

        let workspace_root_manifest: PathBuf = metadata.workspace_root.join("Cargo.toml").into();

        let crate_meta = crate_meta_from_packages(&metadata.packages);

        let inherited_deps: HashMap<PathBuf, InheritedDeps> = members
            .iter()
            .map(|member| {
                let path: PathBuf = member.manifest_path.clone().into();
                let inherited = workspace_inherited_deps(&path);
                (path, inherited)
            })
            .collect();

        let packages: Vec<Package> = build_packages(
            &members,
            &versions,
            &workspace_root_manifest,
            &crate_meta,
            &inherited_deps,
        )
        .into_iter()
        .collect();
        log::debug!(
            "cargo.packages: total resolution took {:?}",
            total_start.elapsed()
        );

        Ok(packages)
    }

    fn update_versions<'a>(
        &self,
        packages: impl IntoIterator<Item = (&'a Usage, &'a Package, VersionReq)>,
    ) -> Result<(), RegistryError> {
        // Group all edits by manifest path so we only read/write each file once.
        let mut edits: HashMap<PathBuf, Vec<ManifestEdit>> = HashMap::new();

        for (usage, package, new_req) in packages {
            // Fan out to every dep-kind section that shares the same unit + req.
            // This handles the case where a package appears under both
            // [dependencies] and [dev-dependencies] with the same version.
            let matching_usages = package
                .usages
                .iter()
                .filter(|u| u.unit == usage.unit && u.req == usage.req);

            for u in matching_usages {
                let manifest = match u.unit.path() {
                    Some(p) => p.to_path_buf(),
                    None => {
                        log::warn!(
                            "skipping update for '{}': unit has no manifest path",
                            package.purl.name()
                        );
                        continue;
                    }
                };

                // The TOML key is the rename (alias) if present, otherwise the
                // real crate name.
                let toml_key = u
                    .rename
                    .as_deref()
                    .unwrap_or_else(|| package.purl.name())
                    .to_string();

                let section = match &u.unit {
                    Unit::Workspace { .. } => {
                        vec!["workspace".to_string(), "dependencies".to_string()]
                    }
                    _ => vec![u.kind.to_string()],
                };

                edits.entry(manifest).or_default().push(ManifestEdit {
                    section,
                    toml_key,
                    new_req: new_req.clone(),
                    old_req: u.req.clone(),
                    preserve_bare: true,
                });
            }
        }

        // Apply all edits, one manifest at a time.
        for (manifest, file_edits) in &edits {
            let mut seen = HashSet::new();
            let deduped: Vec<_> = file_edits
                .iter()
                .filter(|edit| {
                    seen.insert((
                        edit.section.clone(),
                        edit.toml_key.clone(),
                        edit.old_req.to_string(),
                        edit.new_req.to_string(),
                    ))
                })
                .cloned()
                .collect();

            apply_manifest_edits(manifest, &deduped)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::DepKind;
    use crate::registry::RegistryImpl;
    use std::path::PathBuf;

    fn init() -> CargoRegistry {
        CargoRegistry::new(State::new(None, crate::Options::default()).into())
    }

    fn init_workspace_demo() -> CargoRegistry {
        let root =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cargo/workspace-demo");
        CargoRegistry::new(State::new(Some(root), crate::Options::default()).into())
    }

    #[test]
    fn fetch_this_workspace() {
        let registry = init();
        let packages = registry.packages().unwrap();
        assert!(packages.into_iter().any(|p| p.purl.name() == "clap"));
    }

    #[test]
    fn workspace_deps_use_workspace_unit() {
        let registry = init();
        let packages = registry.packages().unwrap();

        // All deps in this project use `workspace = true`, so every usage
        // should point at the workspace root Cargo.toml via Unit::Workspace.
        for package in packages {
            for usage in &package.usages {
                assert!(
                    matches!(&usage.unit, Unit::Workspace { .. }),
                    "expected Unit::Workspace for '{}', got {:?}",
                    package.purl.name(),
                    usage.unit
                );
            }
        }
    }

    #[test]
    fn workspace_unit_has_path() {
        let registry = init();
        let packages = registry.packages().unwrap();

        let any_workspace_usage = packages
            .into_iter()
            .flat_map(|pkg| pkg.usages)
            .find(|u| matches!(&u.unit, Unit::Workspace { .. }))
            .expect("should have at least one workspace usage");

        let path = any_workspace_usage.unit.path().unwrap();
        assert!(
            path.ends_with("Cargo.toml"),
            "workspace unit path should point to Cargo.toml, got {}",
            path.display()
        );
    }

    #[test]
    fn dep_kinds_are_set() {
        let registry = init();
        let packages = registry.packages().unwrap();

        // semver is a normal dependency of check-updates
        let semver = packages
            .into_iter()
            .find(|p| p.purl.name() == "semver")
            .expect("semver should be in packages");

        assert!(
            semver.usages.iter().any(|u| u.kind == DepKind::Normal),
            "semver should have at least one Normal usage"
        );
    }

    #[test]
    fn crate_meta_populated() {
        let registry = init();
        let packages = registry.packages().unwrap();

        // clap is a well-known crate that has a repository field
        let clap = packages
            .into_iter()
            .find(|p| p.purl.name() == "clap")
            .expect("clap should be in packages");

        assert!(
            clap.repository.is_some(),
            "clap should have a repository URL"
        );
    }

    #[test]
    fn renamed_deps_are_not_mistaken_for_workspace_inherited() {
        let registry = init_workspace_demo();
        let packages = registry.packages().unwrap();

        let rand = packages
            .into_iter()
            .find(|p| p.purl.name() == "rand")
            .expect("rand should be in packages");

        let renamed_usages: Vec<_> = rand
            .usages
            .iter()
            .filter(|u| u.rename.as_deref() == Some("rand07"))
            .collect();

        assert!(
            !renamed_usages.is_empty(),
            "workspace demo should include renamed rand07 usages"
        );
        assert!(
            renamed_usages
                .iter()
                .all(|u| matches!(u.unit, Unit::Project { .. })),
            "renamed rand07 usages should map to project manifests, not workspace"
        );
    }

    #[test]
    fn target_workspace_inherited_stays_workspace_unit() {
        let registry = init_workspace_demo();
        let packages = registry.packages().unwrap();

        let anyhow = packages
            .into_iter()
            .find(|p| p.purl.name() == "anyhow")
            .expect("anyhow should be in packages");

        assert!(
            anyhow
                .usages
                .iter()
                .any(|u| matches!(u.unit, Unit::Workspace { .. })),
            "workspace-inherited anyhow usages should include workspace unit"
        );
    }

    #[test]
    fn target_non_workspace_dep_stays_project_unit() {
        let registry = init_workspace_demo();
        let packages = registry.packages().unwrap();

        let tokio = packages
            .into_iter()
            .find(|p| p.purl.name() == "tokio")
            .expect("tokio should be in packages");

        assert!(
            tokio
                .usages
                .iter()
                .any(|u| matches!(u.unit, Unit::Project { .. })),
            "target non-workspace tokio usage should include project unit"
        );
    }
}
