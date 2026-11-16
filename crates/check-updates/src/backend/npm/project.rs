use std::path::{Path, PathBuf};

use super::{manifest::PackageJson, pm::pnpm::PnpmWorkspace};

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
