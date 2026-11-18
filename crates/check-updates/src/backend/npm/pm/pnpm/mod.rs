use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::super::npmrc::{NpmrcValues, last, read_npmrc, split_excludes};
use super::{ConfigEnv, PackageManagerImpl, PublishAgeConfig};
use crate::config::read_optional;

mod workspace;
pub(crate) use workspace::PnpmWorkspace;

/// pnpm configuration and native upgrade commands.
pub(super) struct Pnpm;

impl Pnpm {
    fn parse_minimum_age(value: &str) -> Result<jiff::Span, String> {
        if let Ok(minutes) = value.trim().parse::<i64>() {
            if minutes < 0 {
                return Err("minimumReleaseAge must be non-negative".into());
            }
            return jiff::Span::new()
                .try_minutes(minutes)
                .map_err(|error| error.to_string());
        }
        let minutes = value
            .trim()
            .parse::<f64>()
            .map_err(|error| error.to_string())?;
        if !minutes.is_finite() || minutes < 0.0 {
            return Err("minimumReleaseAge must be finite and non-negative".into());
        }
        jiff::SignedDuration::try_from_secs_f64(minutes * 60.0)
            .and_then(jiff::Span::try_from)
            .map_err(|error| error.to_string())
    }

    fn apply_npmrc(&self, config: &mut PublishAgeConfig, source: &NpmrcValues) {
        if let Some(value) = last(source, "minimum-release-age") {
            match Self::parse_minimum_age(value) {
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
        if let Some(value) = last(source, "minimum-release-age-ignore-missing-time") {
            match value.parse::<bool>() {
                Ok(ignore) => config.ignore_missing = Some(ignore),
                Err(error) => {
                    log::warn!("ignoring invalid minimum-release-age-ignore-missing-time: {error}")
                }
            }
        }
    }

    fn apply_yaml(&self, config: &mut PublishAgeConfig, path: &Path, env: &ConfigEnv) {
        let workspace = match read_optional(path)
            .and_then(|text| text.map(|text| PnpmWorkspace::parse(&text)).transpose())
        {
            Ok(Some(workspace)) => workspace,
            Ok(None) => return,
            Err(error) => {
                log::warn!(
                    "{}: ignoring invalid pnpm configuration: {error}",
                    path.display()
                );
                return;
            }
        };
        if let Some(value) = workspace.minimum_release_age {
            match env
                .expand(&value)
                .and_then(|value| Self::parse_minimum_age(&value))
            {
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
            match env
                .expand(&value)
                .and_then(|value| value.parse::<bool>().map_err(|error| error.to_string()))
            {
                Ok(ignore) => config.ignore_missing = Some(ignore),
                Err(error) => log::warn!(
                    "{}: ignoring invalid minimumReleaseAgeIgnoreMissingTime: {error}",
                    path.display()
                ),
            }
        }
    }
}

impl PackageManagerImpl for Pnpm {
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
            self.apply_npmrc(&mut config, &source);
            if let Some(path) = path {
                self.apply_yaml(&mut config, &path, env);
            }
        }
        if let Some(value) = env.pnpm("minimum-release-age") {
            match Self::parse_minimum_age(value) {
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
                "--config.minimum-release-age={}",
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
