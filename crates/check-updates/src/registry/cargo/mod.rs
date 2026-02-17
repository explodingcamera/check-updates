use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    rc::Rc,
};

use cargo_metadata::{CargoOpt, MetadataCommand};
use crates_index::SparseIndex;
use semver::Version;
use thiserror::Error;

use crate::{
    Purl, State,
    package::{DepKind, Package, PackageVersion, Packages, Unit, Usage},
    registry::RegistryError,
};

mod fetch;

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
}

/// Resolve workspace member IDs to their package metadata.
fn workspace_members(metadata: &cargo_metadata::Metadata) -> Vec<&cargo_metadata::Package> {
    metadata
        .workspace_members
        .iter()
        .filter_map(|member_id| metadata.packages.iter().find(|p| &p.id == member_id))
        .collect()
}

fn parse_versions(
    index: &SparseIndex,
    name: &str,
    response: http::Response<Vec<u8>>,
) -> Result<Vec<PackageVersion>, CargoError> {
    let krate = index.parse_cache_response(name, response, true)?;

    let Some(krate) = krate else {
        log::debug!("no crate data returned for '{name}'");
        return Ok(Vec::new());
    };

    Ok(krate
        .versions()
        .iter()
        .filter_map(|v| {
            let version = Version::parse(v.version()).ok()?;
            Some(PackageVersion {
                version,
                yanked: v.is_yanked(),
                features: v.features().clone(),
                rust_version: v.rust_version().and_then(|s| Version::parse(s).ok()),
            })
        })
        .collect())
}

/// Read a member's raw Cargo.toml and return the set of dependency names
/// that are inherited from the workspace (i.e. have `workspace = true`).
fn workspace_inherited_deps(manifest_path: &Path) -> HashSet<String> {
    let contents = match std::fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to read {}: {e}", manifest_path.display());
            return HashSet::new();
        }
    };

    let doc = match contents.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("failed to parse {}: {e}", manifest_path.display());
            return HashSet::new();
        }
    };

    let mut inherited = HashSet::new();

    for section in &["dependencies", "dev-dependencies", "build-dependencies"] {
        if let Some(table) = doc.get(section).and_then(|v| v.as_table()) {
            for (name, value) in table {
                let is_workspace = value
                    .as_table_like()
                    .and_then(|t| t.get("workspace"))
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if is_workspace {
                    inherited.insert(name.to_string());
                }
            }
        }
    }

    inherited
}

fn collect_crates_io_deps(members: &[&cargo_metadata::Package]) -> HashSet<String> {
    members
        .iter()
        .flat_map(|member| &member.dependencies)
        .filter(|dep| dep.source.as_ref().is_some_and(|s| s.is_crates_io()))
        .map(|dep| dep.name.clone())
        .collect()
}

/// Best-effort metadata extracted from locally cached crate sources.
struct CrateMeta {
    repository: Option<String>,
    homepage: Option<String>,
}

/// Build a lookup of crate metadata (repository, homepage) from the resolved
/// dependency graph. When multiple versions of a crate are present, the latest
/// version wins.
fn crate_meta_from_packages(
    all_packages: &[cargo_metadata::Package],
) -> HashMap<String, CrateMeta> {
    let mut best_version: HashMap<String, Version> = HashMap::new();
    let mut map: HashMap<String, CrateMeta> = HashMap::new();

    for pkg in all_packages {
        let is_crates_io = pkg.source.as_ref().is_some_and(|s| s.is_crates_io());

        if !is_crates_io {
            continue;
        }

        let dominated = best_version
            .get(pkg.name.as_str())
            .is_some_and(|existing| existing >= &pkg.version);

        if dominated {
            continue;
        }

        best_version.insert(pkg.name.to_string(), pkg.version.clone());
        map.insert(
            pkg.name.to_string(),
            CrateMeta {
                repository: pkg.repository.clone(),
                homepage: pkg.homepage.clone(),
            },
        );
    }

    map
}

fn build_packages(
    members: &[&cargo_metadata::Package],
    versions: &HashMap<String, Vec<PackageVersion>>,
    workspace_root_manifest: &Path,
    crate_meta: &HashMap<String, CrateMeta>,
    inherited_deps: &HashMap<PathBuf, HashSet<String>>,
) -> Packages {
    let mut packages = Packages::new();
    let workspace_unit = Unit::Workspace(workspace_root_manifest.to_path_buf());

    for member in members {
        let member_path: PathBuf = member.manifest_path.clone().into();
        let member_unit = Unit::Project(member_path.clone());
        let inherited = inherited_deps.get(&member_path);

        member
            .dependencies
            .iter()
            .filter(|dep| dep.source.as_ref().is_some_and(|s| s.is_crates_io()))
            .filter_map(|dep| {
                Purl::new("cargo".to_string(), dep.name.clone())
                    .ok()
                    .map(|purl| (purl, dep))
            })
            .for_each(|(purl, dep)| {
                let unit = if inherited.is_some_and(|set| set.contains(&dep.name)) {
                    workspace_unit.clone()
                } else {
                    member_unit.clone()
                };

                let kind = match dep.kind {
                    cargo_metadata::DependencyKind::Normal => DepKind::Normal,
                    cargo_metadata::DependencyKind::Development => DepKind::Dev,
                    cargo_metadata::DependencyKind::Build => DepKind::Build,
                    _ => DepKind::Normal,
                };

                let usage = Usage {
                    unit,
                    req: dep.req.clone(),
                    kind,
                };
                packages
                    .entry(purl.clone())
                    .or_insert_with(|| {
                        let meta = crate_meta.get(&dep.name);
                        Package {
                            purl,
                            usages: vec![],
                            versions: versions.get(&dep.name).cloned().unwrap_or_default(),
                            repository: meta.and_then(|m| m.repository.clone()),
                            homepage: meta.and_then(|m| m.homepage.clone()),
                        }
                    })
                    .usages
                    .push(usage);
            });
    }

    packages
}

pub struct CargoRegistry {
    packages: Packages,
}

impl super::RegistryImpl for CargoRegistry {
    const TYPE: &'static str = "cargo";

    fn initialize(state: Rc<State>) -> Result<Self, RegistryError> {
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

        let responses = fetch::fetch_all(state.multi(), requests);

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

        Ok(Self {
            packages: build_packages(
                &members,
                &versions,
                &workspace_root_manifest,
                &crate_meta,
                &inherited_deps,
            ),
        })
    }

    fn packages(&self) -> &Packages {
        &self.packages
    }

    fn update_versions<'a>(
        &self,
        _unit: &Unit,
        _packages: impl IntoIterator<Item = &'a Purl>,
    ) -> Result<(), RegistryError> {
        todo!()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::RegistryImpl;

    fn init() -> CargoRegistry {
        let state = Rc::new(State::new());
        CargoRegistry::initialize(state).unwrap()
    }

    #[test]
    fn fetch_this_workspace() {
        let registry = init();
        let packages = registry.packages();
        assert!(packages.keys().any(|p: &Purl| p.name() == "clap"));
    }

    #[test]
    fn workspace_deps_use_workspace_unit() {
        let registry = init();
        let packages = registry.packages();

        // All deps in this project use `workspace = true`, so every usage
        // should point at the workspace root Cargo.toml via Unit::Workspace.
        for (purl, package) in packages {
            for usage in &package.usages {
                assert!(
                    matches!(&usage.unit, Unit::Workspace(_)),
                    "expected Unit::Workspace for '{}', got {:?}",
                    purl.name(),
                    usage.unit
                );
            }
        }
    }

    #[test]
    fn workspace_unit_has_path() {
        let registry = init();
        let packages = registry.packages();

        let any_workspace_usage = packages
            .values()
            .flat_map(|pkg| &pkg.usages)
            .find(|u| matches!(&u.unit, Unit::Workspace(_)))
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
        let packages = registry.packages();

        // semver is a normal dependency of check-updates
        let semver = packages
            .iter()
            .find(|(p, _)| p.name() == "semver")
            .map(|(_, pkg)| pkg)
            .expect("semver should be in packages");

        assert!(
            semver.usages.iter().any(|u| u.kind == DepKind::Normal),
            "semver should have at least one Normal usage"
        );
    }

    #[test]
    fn crate_meta_populated() {
        let registry = init();
        let packages = registry.packages();

        // clap is a well-known crate that has a repository field
        let clap = packages
            .iter()
            .find(|(p, _)| p.name() == "clap")
            .map(|(_, pkg)| pkg)
            .expect("clap should be in packages");

        assert!(
            clap.repository.is_some(),
            "clap should have a repository URL"
        );
    }
}
