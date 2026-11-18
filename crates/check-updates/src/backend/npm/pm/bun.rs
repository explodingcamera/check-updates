use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{ConfigEnv, PackageManagerImpl, PublishAgeConfig};
use crate::config::read_toml;
use crate::publish_age::parse_span;

/// Resolve Bun's global and workspace bunfig files.
pub(super) struct Bun;

impl PackageManagerImpl for Bun {
    fn resolve_config(&self, root: &Path, env: &ConfigEnv) -> PublishAgeConfig {
        let mut config = PublishAgeConfig::default();
        let global = env
            .get("XDG_CONFIG_HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .filter(|path| path.is_absolute())
            .or_else(|| env.home());
        for path in global
            .map(|dir| dir.join(".bunfig.toml"))
            .into_iter()
            .chain([root.join("bunfig.toml")])
        {
            let doc = match read_toml(&path) {
                Ok(Some(doc)) => doc,
                Ok(None) => continue,
                Err(error) => {
                    log::warn!("ignoring invalid Bun configuration: {error}");
                    continue;
                }
            };
            let Some(install) = doc.get("install") else {
                continue;
            };
            if let Some(value) = install.get("minimumReleaseAge") {
                let minimum = if let Some(value) = value.as_str() {
                    parse_span(value)
                } else if let Some(value) = value.as_integer() {
                    if value < 0 {
                        Err("minimumReleaseAge must be non-negative".into())
                    } else {
                        jiff::Span::new()
                            .try_seconds(value)
                            .map_err(|error| error.to_string())
                    }
                } else {
                    Err("minimumReleaseAge must be seconds or a duration string".into())
                };
                match minimum {
                    Ok(minimum) => config.minimum = Some(minimum),
                    Err(error) => log::warn!(
                        "{}: ignoring invalid minimumReleaseAge: {error}",
                        path.display()
                    ),
                }
            }
            if let Some(value) = install.get("minimumReleaseAgeExcludes") {
                let excludes = value.as_array().and_then(|values| {
                    values
                        .iter()
                        .map(|value| value.as_str().map(String::from))
                        .collect()
                });
                match excludes {
                    Some(excludes) => config.excludes = excludes,
                    None => log::warn!(
                        "{}: ignoring invalid minimumReleaseAgeExcludes, expected an array of strings",
                        path.display()
                    ),
                }
            }
        }
        config
    }

    fn upgrade_command(&self, lockfile_only: bool, minimum: Option<Duration>) -> Command {
        let mut command = Command::new("bun");
        // Bun update rewrites package.json, so apply our edits with install instead.
        command.args(["install", "--ignore-scripts"]);
        if lockfile_only {
            command.arg("--lockfile-only");
        }
        if let Some(minimum) = minimum {
            let seconds = minimum.as_secs() + u64::from(minimum.subsec_nanos() != 0);
            command
                .arg("--minimum-release-age")
                .arg(seconds.to_string());
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::super::PackageManager;
    use super::*;
    use crate::publish_age::PublishAge;

    #[test]
    fn merges_global_and_local_config() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let mut env = ConfigEnv::default();
        env.0
            .insert("HOME".into(), dir.path().to_string_lossy().into());
        std::fs::write(
            dir.path().join(".bunfig.toml"),
            "[install]\nminimumReleaseAge = 86400\nminimumReleaseAgeExcludes = ['pkg@1.0.0']\n",
        )
        .unwrap();
        let policy =
            PublishAge::try_from_npm_config(&root, PackageManager::Bun, None, false, &env).unwrap();
        assert!(policy.enabled());
        assert!(policy.allows("pkg", "1.0.0", Some("2099-01-01T00:00:00Z")));
        assert!(!policy.allows("pkg", "2.0.0", Some("2099-01-01T00:00:00Z")));
        let config = dir.path().join("config");
        std::fs::create_dir(&config).unwrap();
        std::fs::write(
            config.join(".bunfig.toml"),
            "[install]\nminimumReleaseAge=0\n",
        )
        .unwrap();
        env.0
            .insert("XDG_CONFIG_HOME".into(), config.to_string_lossy().into());
        assert!(
            !PublishAge::try_from_npm_config(&root, PackageManager::Bun, None, false, &env)
                .unwrap()
                .enabled()
        );
        std::fs::write(
            root.join("bunfig.toml"),
            "[install]\nminimumReleaseAge = '1 minute'\n",
        )
        .unwrap();
        assert!(
            PublishAge::try_from_npm_config(&root, PackageManager::Bun, None, false, &env)
                .unwrap()
                .enabled()
        );
    }
}
