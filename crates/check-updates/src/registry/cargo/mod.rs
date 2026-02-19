use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    rc::Rc,
};

use cargo_metadata::{CargoOpt, MetadataCommand};
use crates_index::SparseIndex;
use semver::VersionReq;
use thiserror::Error;

use crate::{
    State,
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
        let index = SparseIndex::from_url_with_hash_kind(
            crates_index::sparse::URL,
            &crates_index::HashKind::Stable,
        )
        .map_err(CargoError::from)?;

        let metadata = MetadataCommand::new()
            .features(CargoOpt::AllFeatures)
            .exec()
            .or_else(|_| {
                MetadataCommand::new()
                    .features(CargoOpt::SomeFeatures(vec![]))
                    .exec()
            })
            .or_else(|_| MetadataCommand::new().exec())
            .map_err(|e| CargoError::Metadata(e.to_string()))?;

        let members = workspace_members(&metadata);
        let names = collect_crates_io_deps(&members);

        let requests: Vec<(String, http::Request<()>)> = names
            .iter()
            .filter_map(|name| {
                let request = index
                    .make_cache_request(name)
                    .map_err(|e| e.to_string())
                    .and_then(|b| b.body(()).map_err(|e| e.to_string()));

                match request {
                    Ok(req) => Some((name.clone(), req)),
                    Err(e) => {
                        log::warn!("failed to build cache request for '{name}': {e}");
                        None
                    }
                }
            })
            .collect();

        let responses = fetch::fetch_all(self.state.multi(), requests);

        let versions: HashMap<String, Vec<PackageVersion>> = names
            .into_iter()
            .filter_map(|name| {
                let response = match responses.get(&name) {
                    Some(Ok(r)) => r.clone(),
                    Some(Err(e)) => {
                        log::warn!("failed to fetch index for '{name}': {e}");
                        return None;
                    }
                    None => return None,
                };

                match parse_versions(&index, &name, response) {
                    Ok(v) => Some((name, v)),
                    Err(e) => {
                        log::warn!("failed to parse versions for '{name}': {e}");
                        None
                    }
                }
            })
            .collect();

        let workspace_root_manifest: PathBuf = metadata.workspace_root.join("Cargo.toml").into();
        let crate_meta = crate_meta_from_packages(&metadata.packages);

        let inherited_deps: HashMap<PathBuf, HashSet<String>> = members
            .iter()
            .map(|member| {
                let path: PathBuf = member.manifest_path.clone().into();
                let inherited = workspace_inherited_deps(&path);
                (path, inherited)
            })
            .collect();

        Ok(build_packages(
            &members,
            &versions,
            &workspace_root_manifest,
            &crate_meta,
            &inherited_deps,
        ))
    }

    fn update_versions<'a>(
        &self,
        packages: impl IntoIterator<Item = (&'a Usage, &'a Package, VersionReq)>,
    ) -> Result<(), RegistryError> {
        // Group all edits by manifest path so we only read/write each file once.
        let mut edits: HashMap<PathBuf, Vec<ManifestEdit>> = HashMap::new();

        for (usage, package, new_req) in packages {
            let manifest = match usage.unit.path() {
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
            let toml_key = usage
                .rename
                .as_deref()
                .unwrap_or_else(|| package.purl.name())
                .to_string();

            let section = match &usage.unit {
                Unit::Workspace { .. } => "workspace.dependencies".to_string(),
                _ => usage.kind.to_string(),
            };

            edits.entry(manifest).or_default().push(ManifestEdit {
                section,
                toml_key,
                new_req: new_req.clone(),
            });
        }

        // Apply all edits, one manifest at a time.
        for (manifest, file_edits) in &edits {
            apply_manifest_edits(manifest, file_edits)?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::package::DepKind;
    use crate::registry::RegistryImpl;

    fn init() -> CargoRegistry {
        CargoRegistry::new(State::new().into())
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
}
