use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use super::{manifest::PackageJson, pm::pnpm::PnpmWorkspace};
use crate::{DepKind, Requirement, Unit};

/// A declared npm dependency and its manifest alias.
pub(super) struct Dependency {
    pub(super) alias: String,
    pub(super) name: String,
    pub(super) requirement: Requirement,
    pub(super) kind: DepKind,
}

/// A workspace project and its direct dependencies.
pub(super) struct Project {
    pub(super) name: String,
    pub(super) unit: Unit,
    pub(super) dependencies: Vec<Dependency>,
}

impl Project {
    fn from_manifest(path: PathBuf, manifest: PackageJson, workspace: bool) -> Self {
        let name = manifest.name.clone().unwrap_or_else(|| {
            path.parent()
                .unwrap_or(Path::new("."))
                .display()
                .to_string()
        });
        let unit = if workspace {
            Unit::Workspace { manifest: path }
        } else {
            Unit::Project {
                manifest: path,
                name: name.clone(),
            }
        };
        let mut dependencies = Vec::new();
        for (kind, entries) in [
            (DepKind::Normal, &manifest.dependencies),
            (DepKind::Dev, &manifest.dev_dependencies),
            (DepKind::Optional, &manifest.optional_dependencies),
            (DepKind::Peer, &manifest.peer_dependencies),
        ] {
            for (alias, spec) in entries {
                let (package, range) = if let Some(alias_spec) = spec.strip_prefix("npm:") {
                    match alias_spec.rsplit_once('@') {
                        Some((package, range)) => (package, range),
                        None => continue,
                    }
                } else {
                    (alias.as_str(), spec.as_str())
                };
                let Ok(requirement) = Requirement::from_node(spec) else {
                    log::debug!("skipping non-semver dependency {alias}: {spec}");
                    continue;
                };
                let Some(current) = requirement.current_version() else {
                    continue;
                };
                if range == "*" || requirement.with_version(&current).is_none() {
                    continue;
                }
                dependencies.push(Dependency {
                    alias: alias.clone(),
                    name: package.to_string(),
                    requirement,
                    kind,
                });
            }
        }
        Self {
            name,
            unit,
            dependencies,
        }
    }
}

/// Read the root package and explicitly included workspace members.
pub(super) fn discover(root: &Path, selected: &[String]) -> Result<Vec<Project>, String> {
    let root_path = root.join("package.json");
    let root_manifest = PackageJson::from_path(&root_path)?;
    let workspace = PnpmWorkspace::from_root(root)?;
    let has_workspace = root_manifest.workspaces.is_some() || !workspace.packages.is_empty();
    let mut paths = BTreeSet::new();
    let mut excluded = BTreeSet::new();
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    // Escape directory names, not Windows drive/UNC prefixes such as `\\?\C:\`.
    let root_pattern: PathBuf = root
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => {
                glob::Pattern::escape(&name.to_string_lossy()).into()
            }
            _ => component.as_os_str().to_owned(),
        })
        .collect();
    let patterns = root_manifest
        .workspaces
        .as_ref()
        .map(|workspaces| workspaces.patterns())
        .unwrap_or_default();
    for pattern in patterns.iter().chain(&workspace.packages) {
        let (exclude, pattern) = match pattern.strip_prefix('!') {
            Some(pattern) => (true, pattern),
            None => (false, pattern.as_str()),
        };
        if Path::new(pattern).is_absolute() || pattern.split('/').any(|part| part == "..") {
            continue;
        }
        let pattern = root_pattern.join(pattern).join("package.json");
        for path in glob::glob(&pattern.to_string_lossy()).map_err(|error| error.to_string())? {
            let path = path.map_err(|error| error.to_string())?;
            if path
                .components()
                .any(|part| part.as_os_str() == "node_modules")
                || !path
                    .canonicalize()
                    .map_err(|error| error.to_string())?
                    .starts_with(&canonical_root)
            {
                continue;
            }
            if exclude {
                excluded.insert(path);
            } else {
                paths.insert(path);
            }
        }
    }
    paths.retain(|path| !excluded.contains(path));
    paths.remove(&root_path);
    let mut projects = vec![Project::from_manifest(
        root_path,
        root_manifest,
        has_workspace,
    )];
    for path in paths {
        let manifest = PackageJson::from_path(&path)?;
        projects.push(Project::from_manifest(path, manifest, false));
    }
    if !selected.is_empty() {
        projects.retain(|project| selected.contains(&project.name));
    }
    Ok(projects)
}

/// Find the nearest package, promoting it only to a workspace that explicitly includes it.
pub(super) fn root(start: &Path) -> Result<Option<PathBuf>, String> {
    let start = start
        .canonicalize()
        .map_err(|error| format!("{}: {error}", start.display()))?;
    let Some(package) = start
        .ancestors()
        .find(|directory| directory.join("package.json").is_file())
    else {
        return Ok(None);
    };
    let manifest = PackageJson::from_path(&package.join("package.json"))?;
    if manifest.workspaces.is_some() || package.join("pnpm-workspace.yaml").is_file() {
        return Ok(Some(package.to_path_buf()));
    }
    for parent in package.ancestors().skip(1) {
        let manifest_path = parent.join("package.json");
        let pnpm_workspace = parent.join("pnpm-workspace.yaml").is_file();
        if !manifest_path.is_file() && !pnpm_workspace {
            continue;
        }
        // A parent package is a boundary, not permission to search arbitrary outer projects.
        let patterns = if pnpm_workspace {
            PnpmWorkspace::from_root(parent)?.packages
        } else {
            PackageJson::from_path(&manifest_path)?
                .workspaces
                .as_ref()
                .map(|workspace| workspace.patterns().to_vec())
                .unwrap_or_default()
        };
        let relative = package.strip_prefix(parent).expect("ancestor path");
        if includes(&patterns, relative)? && manifest_path.is_file() {
            return Ok(Some(parent.to_path_buf()));
        }
        break;
    }
    Ok(Some(package.to_path_buf()))
}

fn includes(patterns: &[String], relative: &Path) -> Result<bool, String> {
    let relative = relative.to_string_lossy().replace('\\', "/");
    let options = glob::MatchOptions {
        case_sensitive: true,
        require_literal_separator: true,
        require_literal_leading_dot: false,
    };
    let mut included = false;
    let mut excluded = false;
    for pattern in patterns {
        let (exclude, pattern) = pattern
            .strip_prefix('!')
            .map(|pattern| (true, pattern))
            .unwrap_or((false, pattern.as_str()));
        if Path::new(pattern).is_absolute() || pattern.split('/').any(|part| part == "..") {
            continue;
        }
        let pattern =
            glob::Pattern::new(pattern.trim_end_matches('/')).map_err(|error| error.to_string())?;
        if pattern.matches_with(&relative, options) {
            if exclude {
                excluded = true;
            } else {
                included = true;
            }
        }
    }
    Ok(included && !excluded)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn package(path: &Path, contents: &str) {
        std::fs::create_dir_all(path.join("src")).unwrap();
        std::fs::write(path.join("package.json"), contents).unwrap();
    }

    #[test]
    fn uses_only_explicit_workspace_members() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        package(&base, r#"{"workspaces":["packages/*","!packages/skip"]}"#);
        for (relative, promoted) in [
            ("packages/app", true),
            ("packages/skip", false),
            ("packages/deep/app", false),
            ("unrelated/app", false),
        ] {
            let member = base.join(relative);
            package(&member, "{}");
            assert_eq!(
                root(&member.join("src")).unwrap(),
                Some(if promoted { base.clone() } else { member })
            );
        }
    }

    #[test]
    fn stops_at_standalone_package_boundaries() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().canonicalize().unwrap();
        package(&base, r#"{"workspaces":["packages/**"]}"#);
        let standalone = base.join("packages/standalone");
        package(&standalone, "{}");
        let nested = standalone.join("vendor/nested");
        package(&nested, "{}");
        assert_eq!(root(&nested.join("src")).unwrap(), Some(nested));
        std::fs::write(base.join("package.json"), "{}").unwrap();
        assert_eq!(root(&standalone).unwrap(), Some(standalone.clone()));
        std::fs::write(
            standalone.join("package.json"),
            r#"{"workspaces":["children/*"]}"#,
        )
        .unwrap();
        let child = standalone.join("children/app");
        package(&child, "{}");
        assert_eq!(root(&child).unwrap(), Some(standalone));
    }
}
