use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use semver::Version;
use tame_index::IndexKrate;

use crate::{
    Purl, Requirement,
    package::{DepKind, Package, PackageVersion, Unit, Usage},
};

#[derive(Default)]
pub(super) struct InheritedDeps {
    root: HashSet<(DepKind, String)>,
    target: HashSet<(DepKind, String, String)>,
}

impl InheritedDeps {
    fn contains(&self, dep: &cargo_metadata::Dependency) -> bool {
        let key = dep
            .rename
            .as_deref()
            .unwrap_or(dep.name.as_str())
            .to_string();
        let kind = match dep.kind {
            cargo_metadata::DependencyKind::Normal => DepKind::Normal,
            cargo_metadata::DependencyKind::Development => DepKind::Dev,
            cargo_metadata::DependencyKind::Build => DepKind::Build,
            _ => DepKind::Normal,
        };

        if let Some(target) = &dep.target {
            return self.target.contains(&(kind, key, target.to_string()));
        }

        self.root.contains(&(kind, key))
    }
}

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

pub(super) fn versions_from_krate(krate: &IndexKrate) -> Vec<PackageVersion> {
    krate
        .versions
        .iter()
        .filter_map(|v| {
            Some(PackageVersion {
                version: Version::parse(&v.version).ok()?,
                yanked: v.is_yanked(),
                features: v
                    .features()
                    .map(|(name, members)| (name.clone(), members.clone()))
                    .collect(),
                required_toolchain_version: v.rust_version().and_then(|s| Version::parse(s).ok()),
            })
        })
        .collect()
}

/// Read a member's raw Cargo.toml and return dependencies inherited from
/// workspace (`workspace = true`), split by root vs target tables.
pub(super) fn workspace_inherited_deps(manifest_path: &Path) -> InheritedDeps {
    let contents = match std::fs::read_to_string(manifest_path) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("failed to read {}: {e}", manifest_path.display());
            return InheritedDeps::default();
        }
    };

    let doc = match contents.parse::<toml_edit::DocumentMut>() {
        Ok(d) => d,
        Err(e) => {
            log::warn!("failed to parse {}: {e}", manifest_path.display());
            return InheritedDeps::default();
        }
    };

    let mut inherited = InheritedDeps::default();

    let dep_sections = ["dependencies", "dev-dependencies", "build-dependencies"];

    for section in dep_sections {
        if let Some(table) = doc.get(section).and_then(|v| v.as_table()) {
            collect_workspace_inherited_from_table(table, section, None, &mut inherited);
        }
    }

    if let Some(targets) = doc.get("target").and_then(|v| v.as_table()) {
        for (target_name, target_item) in targets {
            let Some(target_table) = target_item.as_table() else {
                continue;
            };

            for section in dep_sections {
                if let Some(table) = target_table.get(section).and_then(|v| v.as_table()) {
                    collect_workspace_inherited_from_table(
                        table,
                        section,
                        Some(target_name),
                        &mut inherited,
                    );
                }
            }
        }
    }

    inherited
}

fn collect_workspace_inherited_from_table(
    table: &toml_edit::Table,
    section: &str,
    target: Option<&str>,
    out: &mut InheritedDeps,
) {
    let kind = match section {
        "dependencies" => DepKind::Normal,
        "dev-dependencies" => DepKind::Dev,
        "build-dependencies" => DepKind::Build,
        _ => return,
    };

    for (name, value) in table {
        let is_workspace = value
            .as_table_like()
            .and_then(|t| t.get("workspace"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if is_workspace {
            if let Some(target_name) = target {
                out.target
                    .insert((kind, name.to_string(), target_name.to_string()));
            } else {
                out.root.insert((kind, name.to_string()));
            }
        }
    }
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
    let mut latest: HashMap<&str, &cargo_metadata::Package> = HashMap::new();
    for package in all_packages.iter().filter(|package| {
        package
            .source
            .as_ref()
            .is_some_and(|source| source.is_crates_io())
    }) {
        if latest
            .get(package.name.as_str())
            .is_none_or(|current| current.version < package.version)
        {
            latest.insert(package.name.as_str(), package);
        }
    }
    latest
        .into_iter()
        .map(|(name, package)| {
            (
                name.to_string(),
                CrateMeta {
                    repository: package.repository.clone(),
                    homepage: package.homepage.clone(),
                },
            )
        })
        .collect()
}

pub(super) fn build_packages(
    members: &[&cargo_metadata::Package],
    versions: &HashMap<String, Vec<PackageVersion>>,
    workspace_root_manifest: &Path,
    crate_meta: &HashMap<String, CrateMeta>,
    inherited_deps: &HashMap<PathBuf, InheritedDeps>,
) -> Vec<Package> {
    let mut packages = HashMap::new();
    let workspace_unit = Unit::Workspace {
        manifest: workspace_root_manifest.to_path_buf(),
    };

    for member in members {
        let member_path: PathBuf = member.manifest_path.clone().into();
        let member_unit = Unit::Project {
            manifest: member_path.clone(),
            name: member.name.to_string(),
        };
        let inherited = inherited_deps.get(&member_path);

        for dep in member
            .dependencies
            .iter()
            .filter(|dep| dep.source.as_ref().is_some_and(|s| s.is_crates_io()))
        {
            let Ok(purl) = Purl::new("cargo".to_string(), dep.name.clone()) else {
                continue;
            };
            let usage = Usage {
                unit: if inherited.is_some_and(|set| set.contains(dep)) {
                    workspace_unit.clone()
                } else {
                    member_unit.clone()
                },
                req: Requirement::from_cargo(dep.req.clone()),
                kind: match dep.kind {
                    cargo_metadata::DependencyKind::Normal => DepKind::Normal,
                    cargo_metadata::DependencyKind::Development => DepKind::Dev,
                    cargo_metadata::DependencyKind::Build => DepKind::Build,
                    _ => DepKind::Normal,
                },
                rename: dep.rename.clone(),
                supported_toolchain_version: member.rust_version.clone(),
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
        }
    }

    packages.into_values().collect()
}
