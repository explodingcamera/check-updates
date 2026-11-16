use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

use super::{ConfigEnv, ManagerImpl, PublishAgeConfig};
use crate::config::read_toml;
use crate::publish_age::{numeric_duration, parse_duration};

/// Resolve Bun's global and workspace bunfig files.
pub(super) struct Bun;

impl ManagerImpl for Bun {
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
                    parse_duration(value)
                } else if let Some(value) = value.as_integer() {
                    numeric_duration(&value.to_string(), 1)
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
            command
                .arg("--minimum-release-age")
                .arg(minimum.as_secs().to_string());
        }
        command
    }
}

#[cfg(test)]
mod tests {
    use super::super::{PackageManager, publish_age};
    use super::*;

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
        let policy = publish_age(&root, PackageManager::Bun, None, false, &env);
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
        assert!(!publish_age(&root, PackageManager::Bun, None, false, &env).enabled());
        std::fs::write(
            root.join("bunfig.toml"),
            "[install]\nminimumReleaseAge = '1 minute'\n",
        )
        .unwrap();
        assert!(publish_age(&root, PackageManager::Bun, None, false, &env).enabled());
    }
}
