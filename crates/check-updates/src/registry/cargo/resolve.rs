use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
};

use crates_index::SparseIndex;
use semver::Version;

use crate::{
    Purl,
    package::{DepKind, Package, PackageVersion, Unit, Usage},
};

use super::CargoError;

/// Resolve workspace member IDs to their package metadata.
pub(super) fn workspace_members(
    metadata: &cargo_metadata::Metadata,
) -> Vec<&cargo_metadata::Package> {
    metadata
        .workspace_members
        .iter()
        .filter_map(|member_id| metadata.packages.iter().find(|p| &p.id == member_id))
        .collect()
}

pub(super) fn parse_versions(
    index: &SparseIndex,
    name: &str,
    response: http::Response<Vec<u8>>,
    write_cache_entry: bool,
) -> Result<Vec<PackageVersion>, CargoError> {
    let krate = index.parse_cache_response(name, response, write_cache_entry)?;

    let Some(krate) = krate else {
        log::debug!("no crate data returned for '{name}'");
        return Ok(Vec::new());
    };

    Ok(versions_from_crate(&krate))
}

pub(super) fn versions_from_crate(krate: &crates_index::Crate) -> Vec<PackageVersion> {
    krate
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
        .collect()
}

/// Read a member's raw Cargo.toml and return the set of dependency names
/// that are inherited from the workspace (i.e. have `workspace = true`).
pub(super) fn workspace_inherited_deps(manifest_path: &Path) -> HashSet<String> {
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

pub(super) fn collect_crates_io_deps(members: &[&cargo_metadata::Package]) -> HashSet<String> {
    members
        .iter()
        .flat_map(|member| &member.dependencies)
        .filter(|dep| dep.source.as_ref().is_some_and(|s| s.is_crates_io()))
        .map(|dep| dep.name.clone())
        .collect()
}

/// Best-effort metadata extracted from locally cached crate sources.
pub(super) struct CrateMeta {
    pub repository: Option<String>,
    pub homepage: Option<String>,
}

/// Build a lookup of crate metadata (repository, homepage) from the resolved
/// dependency graph. When multiple versions of a crate are present, the latest
/// version wins.
pub(super) fn crate_meta_from_packages(
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

pub(super) fn build_packages(
    members: &[&cargo_metadata::Package],
    versions: &HashMap<String, Vec<PackageVersion>>,
    workspace_root_manifest: &Path,
    crate_meta: &HashMap<String, CrateMeta>,
    inherited_deps: &HashMap<PathBuf, HashSet<String>>,
) -> impl IntoIterator<Item = Package> + use<> {
    let mut packages = HashMap::new();
    let workspace_unit = Unit::Workspace {
        manifest: workspace_root_manifest.to_path_buf(),
    };

    for member in members {
        let member_path: PathBuf = member.manifest_path.clone().into();
        let member_name = member.name.to_string();
        let member_unit = Unit::Project {
            manifest: member_path.clone(),
            name: member_name,
        };
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
                let dep_key = dep.rename.as_deref().unwrap_or(dep.name.as_str());
                let unit = if inherited.is_some_and(|set| set.contains(dep_key)) {
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
                    rename: dep.rename.clone(),
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

    packages.into_values()
}
