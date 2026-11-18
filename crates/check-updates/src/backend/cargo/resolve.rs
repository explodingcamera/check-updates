use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

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

pub(super) fn versions_from_krate(
    krate: &IndexKrate,
    age: &crate::publish_age::PublishAge,
) -> Vec<PackageVersion> {
    krate
        .versions
        .iter()
        // Keep yanked entries for current-version warnings. The selector never recommends them.
        .filter(|v| v.is_yanked() || age.allows(krate.name(), &v.version, v.pubtime.as_deref()))
        .filter_map(|v| {
            Some(PackageVersion {
                version: Version::parse(&v.version).ok()?,
                published_at: v.pubtime.as_deref().and_then(|value| value.parse().ok()),
                yanked: v.is_yanked(),
                features: v
                    .features()
                    .map(|(name, members)| (name.clone(), members.clone()))
                    .collect(),
                required_toolchain_version: v.rust_version().and_then(|value| {
                    // Cargo accepts a rust-version without a minor or patch component.
                    match value.bytes().filter(|&byte| byte == b'.').count() {
                        0 => Version::parse(&format!("{value}.0.0")),
                        1 => Version::parse(&format!("{value}.0")),
                        _ => Version::parse(value),
                    }
                    .ok()
                }),
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

pub(super) fn build_packages(
    members: &[&cargo_metadata::Package],
    mut versions: HashMap<String, Arc<[PackageVersion]>>,
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
                        versions: versions.remove(&dep.name).unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish_age::PublishAge;
    use jiff::{Span, Timestamp};

    #[test]
    fn filters_index_publication_times() {
        let lines = [
            ("1.0.0", None, false),
            ("1.1.0", Some("2000-01-01T00:00:00Z"), false),
            ("1.2.0", Some("2099-01-01T00:00:00Z"), false),
            ("1.3.0", Some("2099-01-01T00:00:00Z"), true),
        ]
        .map(|(version, time, yanked)| {
            let mut entry = serde_json::json!({
                "name": "example", "vers": version, "deps": [],
                "cksum": "0000000000000000000000000000000000000000000000000000000000000000",
                "features": {}, "yanked": yanked, "rust_version": "1.56"
            });
            if let Some(time) = time {
                entry["pubtime"] = time.into();
            }
            entry.to_string()
        })
        .join("\n");
        let krate = IndexKrate::from_slice(lines.as_bytes()).unwrap();
        let versions = versions_from_krate(
            &krate,
            &PublishAge::try_new(Some(Span::new().days(1)), Timestamp::now()).unwrap(),
        );
        assert_eq!(
            versions
                .iter()
                .map(|entry| entry.version.to_string())
                .collect::<Vec<_>>(),
            ["1.0.0", "1.1.0", "1.3.0"]
        );
        assert!(versions.last().unwrap().yanked);
        assert_eq!(
            versions[0].required_toolchain_version,
            Some(Version::new(1, 56, 0))
        );
        assert_eq!(versions_from_krate(&krate, &PublishAge::default()).len(), 4);
    }
}
