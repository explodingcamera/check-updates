use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::super::npmrc::{NpmrcValues, last, read_npmrc, split_excludes};
use super::{ConfigEnv, ManagerImpl, PublishAgeConfig};
use crate::publish_age::numeric_duration;

/// Resolve npm's layered configuration and environment overrides.
pub(super) struct Npm;

impl ManagerImpl for Npm {
    fn resolve_config(&self, root: &Path, env: &ConfigEnv) -> PublishAgeConfig {
        let mut config = PublishAgeConfig {
            patterns: true,
            ..PublishAgeConfig::default()
        };
        let sources = npmrc_sources(root, env);
        for value in sources
            .iter()
            .filter_map(|source| last(source, "min-release-age"))
            .chain(env.npm("min-release-age"))
        {
            match numeric_duration(value, 86400) {
                Ok(minimum) => config.minimum = Some(minimum),
                Err(error) => log::warn!("ignoring invalid min-release-age: {error}"),
            }
        }
        for source in &sources {
            if let Some(values) = source.get("min-release-age-exclude") {
                config.excludes = values
                    .iter()
                    .flat_map(|value| split_excludes(value))
                    .collect();
            }
        }
        if let Some(value) = env.npm("min-release-age-exclude") {
            config.excludes = split_excludes(value);
        }
        config
    }

    fn upgrade_command(&self, lockfile_only: bool, minimum: Option<Duration>) -> Command {
        let mut command = Command::new("npm");
        command.args(["update", "--ignore-scripts"]);
        if lockfile_only {
            command.arg("--package-lock-only");
        }
        if let Some(minimum) = minimum {
            command.arg(format!(
                "--min-release-age={}",
                minimum.as_secs_f64() / 86400.0
            ));
        }
        command
    }
}

/// Resolve npm's global, user, and project npmrc files in precedence order.
pub(super) fn npmrc_sources(root: &Path, env: &ConfigEnv) -> [NpmrcValues; 3] {
    let project = read_npmrc(&root.join(".npmrc"), env);
    let user_path = env
        .npm("userconfig")
        .or_else(|| last(&project, "userconfig"))
        .and_then(|value| env.path(value, root))
        .or_else(|| env.home().map(|home| home.join(".npmrc")));
    let user = user_path
        .as_deref()
        .map(|path| read_npmrc(path, env))
        .unwrap_or_default();
    let global_path = if let Some(value) = env
        .npm("globalconfig")
        .or_else(|| last(&project, "globalconfig"))
        .or_else(|| last(&user, "globalconfig"))
    {
        env.path(value, root)
    } else {
        let builtin = npm_builtin(root, env);
        let prefix = env
            .npm("prefix")
            .or_else(|| last(&user, "prefix"))
            .or_else(|| last(&builtin, "prefix"))
            .or_else(|| env.get("PREFIX"));
        let prefix = prefix
            .and_then(|value| env.path(value, root))
            .or_else(|| node_prefix(root, env));
        prefix.map(|prefix| prefix.join("etc/npmrc"))
    };
    let global = global_path
        .as_deref()
        .map(|path| read_npmrc(path, env))
        .unwrap_or_default();
    [global, user, project]
}

fn executable(root: &Path, env: &ConfigEnv, name: &str) -> Option<PathBuf> {
    std::env::split_paths(env.get("PATH")?)
        .map(|directory| root.join(directory).join(name))
        .find(|path| path.is_file())
}

fn node_prefix(root: &Path, env: &ConfigEnv) -> Option<PathBuf> {
    #[cfg(windows)]
    let name = "node.exe";
    #[cfg(not(windows))]
    let name = "node";
    let node = executable(root, env, name)?.canonicalize().ok()?;
    let directory = node.parent()?;
    #[cfg(windows)]
    {
        Some(directory.to_path_buf())
    }
    #[cfg(not(windows))]
    {
        Some(directory.parent()?.to_path_buf())
    }
}

fn npm_builtin(root: &Path, env: &ConfigEnv) -> NpmrcValues {
    #[cfg(windows)]
    let path = executable(root, env, "npm.cmd")
        .and_then(|path| path.parent().map(|dir| dir.join("node_modules/npm/npmrc")));
    #[cfg(not(windows))]
    let path = executable(root, env, "npm")
        .and_then(|path| path.canonicalize().ok())
        .and_then(|path| path.parent()?.parent().map(|dir| dir.join("npmrc")));
    path.as_deref()
        .map(|path| read_npmrc(path, env))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::super::{PackageManager, publish_age};
    use super::*;

    #[test]
    #[cfg(unix)]
    fn follows_node_symlinks_for_global_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("install/bin")).unwrap();
        std::fs::create_dir_all(root.join("install/etc")).unwrap();
        std::fs::create_dir(root.join("shims")).unwrap();
        std::fs::write(root.join("install/bin/node"), "").unwrap();
        std::os::unix::fs::symlink(root.join("install/bin/node"), root.join("shims/node")).unwrap();
        std::fs::write(root.join("install/etc/npmrc"), "min-release-age=7\n").unwrap();
        let mut env = ConfigEnv::default();
        env.0
            .insert("PATH".into(), root.join("shims").to_string_lossy().into());
        assert_eq!(
            last(&npmrc_sources(root, &env)[0], "min-release-age"),
            Some("7")
        );
    }

    #[test]
    #[cfg(windows)]
    fn reads_windows_npm_builtin_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("node/node_modules/npm")).unwrap();
        std::fs::create_dir_all(root.join("custom/etc")).unwrap();
        std::fs::write(root.join("node/npm.cmd"), "").unwrap();
        std::fs::write(
            root.join("node/node_modules/npm/npmrc"),
            "prefix=${APPDATA}/custom\n",
        )
        .unwrap();
        std::fs::write(root.join("custom/etc/npmrc"), "min-release-age=7\n").unwrap();
        let mut env = ConfigEnv::default();
        env.0
            .insert("Path".into(), root.join("node").to_string_lossy().into());
        env.0
            .insert("APPDATA".into(), root.to_string_lossy().into());
        assert_eq!(
            last(&npmrc_sources(root, &env)[0], "min-release-age"),
            Some("7")
        );
    }

    #[test]
    fn layers_use_days_and_replace_exclusions() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let root = dir.path().join("project");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let mut env = ConfigEnv::default();
        env.0.insert("HOME".into(), home.to_string_lossy().into());
        env.0.insert(
            "npm_config_globalconfig".into(),
            dir.path().join("globalrc").to_string_lossy().into(),
        );
        std::fs::write(
            dir.path().join("globalrc"),
            "min-release-age=7\nmin-release-age-exclude[]=global\n",
        )
        .unwrap();
        std::fs::write(
            home.join(".npmrc"),
            "min-release-age=2\nmin-release-age-exclude[]=user\n",
        )
        .unwrap();
        std::fs::write(root.join(".npmrc"), "min-release-age=${AGE:-1} # days\nmin-release-age-exclude[]=local\nmin-release-age-exclude[]=@org/*\n_authToken=${UNSET_SECRET}\n").unwrap();
        let policy = publish_age(&root, PackageManager::Npm, None, false, &env);
        assert_eq!(
            Npm.resolve_config(&root, &env).minimum,
            Some(Duration::from_secs(86400))
        );
        assert!(policy.enabled());
        assert_eq!(policy.excludes, ["local", "@org/*"]);
        let future = Some("2099-01-01T00:00:00Z");
        assert!(policy.allows("@org/pkg", "1.0.0", future));
        assert!(!policy.allows("global", "1.0.0", future));
        env.0
            .insert("NPM_CONFIG_MIN_RELEASE_AGE".into(), "0".into());
        assert!(!publish_age(&root, PackageManager::Npm, None, false, &env).enabled());
        let minimum = Some(Duration::from_secs(60));
        assert!(publish_age(&root, PackageManager::Npm, minimum, false, &env).enabled());
        env.0.remove("NPM_CONFIG_MIN_RELEASE_AGE");
        std::fs::write(root.join(".npmrc"), "min-release-age=invalid\n").unwrap();
        assert_eq!(
            Npm.resolve_config(&root, &env).minimum,
            Some(Duration::from_secs(2 * 86400))
        );
        for (minimum, ignore, enabled) in [
            (Some(Duration::ZERO), false, false),
            (Some(Duration::from_secs(60)), false, true),
            (None, true, false),
        ] {
            assert_eq!(
                publish_age(&root, PackageManager::Npm, minimum, ignore, &env).enabled(),
                enabled
            );
        }
    }
}
