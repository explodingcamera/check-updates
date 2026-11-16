use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::super::npmrc::{NpmrcValues, last, read_npmrc, split_excludes};
use super::{ConfigEnv, ManagerImpl, PublishAgeConfig};
use crate::config::read_optional;
use crate::publish_age::numeric_duration;

mod workspace;
pub(in crate::backend::npm) use workspace::PnpmWorkspace;

/// Resolve pnpm's npmrc, YAML, and environment layers.
pub(super) struct Pnpm;

impl ManagerImpl for Pnpm {
    fn resolve_config(&self, root: &Path, env: &ConfigEnv) -> PublishAgeConfig {
        let mut config = PublishAgeConfig {
            patterns: true,
            ..PublishAgeConfig::default()
        };
        let [global, user, project] = npmrc_sources(root, env);
        // Each layer overrides the previous one, with YAML following its npmrc source.
        for (source, path) in [
            (
                global,
                config_dir(root, env).map(|dir| dir.join("config.yaml")),
            ),
            (user, None),
            (project, Some(root.join("pnpm-workspace.yaml"))),
        ] {
            if let Some(value) = last(&source, "minimum-release-age") {
                match numeric_duration(value, 60) {
                    Ok(minimum) => config.minimum = Some(minimum),
                    Err(error) => log::warn!("ignoring invalid minimum-release-age: {error}"),
                }
            }
            if let Some(values) = source.get("minimum-release-age-exclude") {
                config.excludes = values
                    .iter()
                    .flat_map(|value| split_excludes(value))
                    .collect();
            }
            if let Some(value) = last(&source, "minimum-release-age-ignore-missing-time") {
                match value.parse::<bool>() {
                    Ok(ignore) => config.ignore_missing = Some(ignore),
                    Err(error) => log::warn!(
                        "ignoring invalid minimum-release-age-ignore-missing-time: {error}"
                    ),
                }
            }
            let Some(path) = path else { continue };
            let workspace = read_optional(&path)
                .and_then(|text| text.map(|text| PnpmWorkspace::parse(&text)).transpose());
            let workspace = match workspace {
                Ok(Some(workspace)) => workspace,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!(
                        "{}: ignoring invalid pnpm configuration: {error}",
                        path.display()
                    );
                    continue;
                }
            };
            if let Some(value) = workspace.minimum_release_age {
                let minimum = env
                    .expand(&value)
                    .and_then(|value| numeric_duration(&value, 60));
                match minimum {
                    Ok(minimum) => config.minimum = Some(minimum),
                    Err(error) => log::warn!(
                        "{}: ignoring invalid minimumReleaseAge: {error}",
                        path.display()
                    ),
                }
            }
            if let Some(values) = workspace.minimum_release_age_exclude {
                match values
                    .iter()
                    .map(|value| env.expand(value))
                    .collect::<Result<_, _>>()
                {
                    Ok(excludes) => config.excludes = excludes,
                    Err(error) => log::warn!(
                        "{}: ignoring invalid minimumReleaseAgeExclude: {error}",
                        path.display()
                    ),
                }
            }
            if let Some(value) = workspace.minimum_release_age_ignore_missing_time {
                let ignore = env
                    .expand(&value)
                    .and_then(|value| value.parse::<bool>().map_err(|error| error.to_string()));
                match ignore {
                    Ok(ignore) => config.ignore_missing = Some(ignore),
                    Err(error) => log::warn!(
                        "{}: ignoring invalid minimumReleaseAgeIgnoreMissingTime: {error}",
                        path.display()
                    ),
                }
            }
        }
        if let Some(value) = env.pnpm("minimum-release-age") {
            match numeric_duration(value, 60) {
                Ok(minimum) => config.minimum = Some(minimum),
                Err(error) => log::warn!("ignoring invalid minimum-release-age: {error}"),
            }
        }
        if let Some(value) = env.pnpm("minimum-release-age-exclude") {
            if value.trim_start().starts_with('[') {
                match serde_json::from_str(value) {
                    Ok(excludes) => config.excludes = excludes,
                    Err(error) => {
                        log::warn!("ignoring invalid minimum-release-age-exclude: {error}")
                    }
                }
            } else {
                config.excludes = split_excludes(value);
            }
        }
        if let Some(value) = env.pnpm("minimum-release-age-ignore-missing-time") {
            match value.parse::<bool>() {
                Ok(ignore) => config.ignore_missing = Some(ignore),
                Err(error) => {
                    log::warn!("ignoring invalid minimum-release-age-ignore-missing-time: {error}")
                }
            }
        }
        config
    }

    fn upgrade_command(&self, lockfile_only: bool, minimum: Option<Duration>) -> Command {
        let mut command = Command::new("pnpm");
        command.args(["update", "--no-save", "--ignore-scripts"]);
        if lockfile_only {
            command.arg("--lockfile-only");
        }
        if let Some(minimum) = minimum {
            command.arg(format!(
                "--minimum-release-age={}",
                minimum.as_secs_f64() / 60.0
            ));
        }
        command
    }
}

/// Resolve pnpm's global, user, and project npmrc files in precedence order.
pub(super) fn npmrc_sources(root: &Path, env: &ConfigEnv) -> [NpmrcValues; 3] {
    let project = read_npmrc(&root.join(".npmrc"), env);
    let user_path = env
        .pnpm("npmrc-auth-file")
        .or_else(|| env.pnpm("userconfig"))
        .or_else(|| last(&project, "userconfig"))
        .and_then(|value| env.path(value, root))
        .or_else(|| env.home().map(|home| home.join(".npmrc")));
    let user = user_path
        .as_deref()
        .map(|path| read_npmrc(path, env))
        .unwrap_or_default();
    let global_path = if let Some(value) = env
        .pnpm("globalconfig")
        .or_else(|| last(&project, "globalconfig"))
        .or_else(|| last(&user, "globalconfig"))
    {
        env.path(value, root)
    } else {
        config_dir(root, env).map(|dir| dir.join("rc"))
    };
    let global = global_path
        .as_deref()
        .map(|path| read_npmrc(path, env))
        .unwrap_or_default();
    [global, user, project]
}

fn config_dir(root: &Path, env: &ConfigEnv) -> Option<PathBuf> {
    if let Some(value) = env.get("XDG_CONFIG_HOME").filter(|value| !value.is_empty())
        && let Some(path) = env.path(value, root)
    {
        return Some(path.join("pnpm"));
    }
    #[cfg(windows)]
    if let Some(value) = env.get("LOCALAPPDATA").filter(|value| !value.is_empty())
        && let Some(path) = env.path(value, root)
    {
        return Some(path.join("pnpm/config"));
    }
    #[cfg(target_os = "macos")]
    let suffix = "Library/Preferences/pnpm";
    #[cfg(not(target_os = "macos"))]
    let suffix = ".config/pnpm";
    env.user_home().map(|home| home.join(suffix))
}

#[cfg(test)]
mod tests {
    use super::super::{PackageManager, publish_age};
    use super::*;

    #[test]
    fn resolves_global_directory() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let mut env = ConfigEnv::default();
        env.0.insert("HOME".into(), root.to_string_lossy().into());
        env.0
            .insert("USERPROFILE".into(), root.to_string_lossy().into());
        #[cfg(target_os = "macos")]
        let expected = "Library/Preferences/pnpm";
        #[cfg(not(target_os = "macos"))]
        let expected = ".config/pnpm";
        assert_eq!(config_dir(root, &env), Some(root.join(expected)));
        #[cfg(windows)]
        {
            env.0.insert("LocalAppData".into(), "local".into());
            assert_eq!(config_dir(root, &env), Some(root.join("local/pnpm/config")));
            env.0
                .insert("XDG_CONFIG_HOME".into(), "%LocalAppData%/custom".into());
            assert_eq!(config_dir(root, &env), Some(root.join("local/custom/pnpm")));
        }
        env.0.insert("XDG_CONFIG_HOME".into(), "custom".into());
        assert_eq!(config_dir(root, &env), Some(root.join("custom/pnpm")));
    }

    #[test]
    fn resolves_files_and_environment_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let global = dir.path().join("config/pnpm");
        let root = dir.path().join("project");
        std::fs::create_dir_all(&global).unwrap();
        std::fs::create_dir_all(&root).unwrap();
        let mut env = ConfigEnv::default();
        env.0.insert(
            "XDG_CONFIG_HOME".into(),
            dir.path().join("config").to_string_lossy().into(),
        );
        std::fs::write(
            global.join("config.yaml"),
            "minimumReleaseAge: 1440\nminimumReleaseAgeExclude: ['@org/*']\n",
        )
        .unwrap();
        let settings = Pnpm.resolve_config(&root, &env);
        assert_eq!(settings.minimum.unwrap().as_secs(), 86400);
        assert!(publish_age(&root, PackageManager::Pnpm, None, false, &env).enabled());
        std::fs::write(root.join("pnpm-workspace.yaml"), "minimumReleaseAge: 0\nminimumReleaseAgeExclude: []\nminimumReleaseAgeIgnoreMissingTime: false\n").unwrap();
        assert!(!publish_age(&root, PackageManager::Pnpm, None, false, &env).enabled());
        assert!(!publish_age(&root, PackageManager::Npm, None, false, &env).enabled());
        env.0
            .insert("NPM_CONFIG_MINIMUM_RELEASE_AGE".into(), "0".into());
        env.0
            .insert("PNPM_CONFIG_MINIMUM_RELEASE_AGE".into(), "1440".into());
        env.0.insert(
            "PNPM_CONFIG_MINIMUM_RELEASE_AGE_EXCLUDE".into(),
            r#"["@org/*","pkg@1.0.0"]"#.into(),
        );
        let policy = publish_age(&root, PackageManager::Pnpm, None, false, &env);
        assert!(policy.enabled());
        assert!(policy.allows("@org/app", "1.0.0", None));
        assert!(policy.allows("pkg", "1.0.0", None));
        assert!(!policy.allows("other", "1.0.0", None));
        env.0.remove("PNPM_CONFIG_MINIMUM_RELEASE_AGE_EXCLUDE");
        std::fs::write(root.join("pnpm-workspace.yaml"), "minimumReleaseAgeExclude: ['${UNSET_EXCLUSION}']\nminimumReleaseAgeIgnoreMissingTime: true\n").unwrap();
        let settings = Pnpm.resolve_config(&root, &env);
        assert_eq!(settings.ignore_missing, Some(true));
        assert_eq!(settings.excludes, ["@org/*"]);
    }
}
