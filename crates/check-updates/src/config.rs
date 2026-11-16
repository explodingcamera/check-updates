use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Read an optional configuration file, reporting errors other than a missing file.
pub(crate) fn read_optional(path: &Path) -> Result<Option<String>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

/// Read an optional TOML configuration file.
pub(crate) fn read_toml(path: &Path) -> Result<Option<toml_edit::DocumentMut>, String> {
    read_optional(path)?
        .map(|text| {
            text.parse()
                .map_err(|error| format!("{}: {error}", path.display()))
        })
        .transpose()
}

/// Environment snapshot used by configuration readers and isolated tests.
#[derive(Default)]
pub(crate) struct ConfigEnv(pub(crate) HashMap<String, String>);

impl ConfigEnv {
    /// Capture the environment and the operating system's home-directory fallback.
    pub(crate) fn current() -> Self {
        let mut env = Self(
            std::env::vars_os()
                .filter_map(|(key, value)| {
                    Some((key.into_string().ok()?, value.into_string().ok()?))
                })
                .collect(),
        );
        #[cfg(windows)]
        let home_key = "USERPROFILE";
        #[cfg(not(windows))]
        let home_key = "HOME";
        if env.get(home_key).is_none_or(str::is_empty)
            && let Some(home) =
                std::env::home_dir().and_then(|path| path.into_os_string().into_string().ok())
        {
            env.0.insert(home_key.into(), home);
        }
        env
    }

    /// Match the operating system's environment-variable name rules.
    pub(crate) fn get(&self, name: &str) -> Option<&str> {
        #[cfg(windows)]
        let value = self.0.get(name).or_else(|| {
            self.0
                .iter()
                .find(|(key, _)| key.eq_ignore_ascii_case(name))
                .map(|(_, value)| value)
        });
        #[cfg(not(windows))]
        let value = self.0.get(name);
        value.map(String::as_str)
    }

    /// Read npm's case-insensitive configuration environment variables.
    pub(crate) fn npm(&self, name: &str) -> Option<&str> {
        let name = format!("npm_config_{}", name.replace('-', "_"));
        self.get(&name)
            .or_else(|| self.get(&name.to_ascii_uppercase()))
            .or_else(|| {
                self.0
                    .iter()
                    .find(|(key, _)| key.replace('-', "_").eq_ignore_ascii_case(&name))
                    .map(|(_, value)| value.as_str())
            })
            .filter(|value| !value.is_empty())
    }

    /// Read modern pnpm overrides before npm-compatible legacy overrides.
    pub(crate) fn pnpm(&self, name: &str) -> Option<&str> {
        let key = format!("pnpm_config_{}", name.replace('-', "_"));
        self.get(&key)
            .or_else(|| self.get(&key.to_ascii_uppercase()))
            .filter(|value| !value.is_empty())
            .or_else(|| self.npm(name))
    }

    /// Resolve the OS home directory, as Cargo and pnpm do.
    pub(crate) fn user_home(&self) -> Option<PathBuf> {
        #[cfg(windows)]
        let key = "USERPROFILE";
        #[cfg(not(windows))]
        let key = "HOME";
        self.get(key)
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    /// Resolve npm/Bun's explicit HOME override before the OS home directory.
    pub(crate) fn home(&self) -> Option<PathBuf> {
        self.get("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .or_else(|| self.user_home())
    }

    /// Resolve an optional config path, warning and skipping invalid references.
    pub(crate) fn path(&self, value: &str, root: &Path) -> Option<PathBuf> {
        let result = (|| {
            let value = self.expand(value)?;
            let value = self.expand_windows_path(&value)?;
            let path = if let Some(relative) = value
                .strip_prefix("~/")
                .or_else(|| value.strip_prefix("~\\"))
            {
                self.home()
                    .ok_or("cannot expand config path without a home directory")?
                    .join(relative)
            } else if value == "~" {
                self.home()
                    .ok_or("cannot expand config path without a home directory")?
            } else {
                PathBuf::from(value)
            };
            Ok::<_, String>(root.join(path))
        })();
        result
            .inspect_err(|error| log::warn!("ignoring invalid configuration path: {error}"))
            .ok()
    }

    fn expand_windows_path(&self, value: &str) -> Result<String, String> {
        #[cfg(not(windows))]
        {
            Ok(value.into())
        }
        #[cfg(windows)]
        {
            let mut value = value.to_string();
            for _ in 0..32 {
                let Some(start) = value.find('%') else {
                    return Ok(value);
                };
                let Some(end) = value[start + 1..].find('%').map(|end| end + start + 1) else {
                    return Ok(value);
                };
                let key = &value[start + 1..end];
                let replacement = self
                    .get(key)
                    .ok_or_else(|| format!("unset environment variable %{key}% in config path"))?;
                let next = format!("{}{}{}", &value[..start], replacement, &value[end + 1..]);
                if next == value || next.len() > 32767 {
                    break;
                }
                value = next;
            }
            Err("recursive environment variable in config path".into())
        }
    }

    /// Expand npm/pnpm-style references, including optional variables and defaults.
    pub(crate) fn expand(&self, value: &str) -> Result<String, String> {
        let mut output = String::new();
        let mut rest = value;
        while let Some(start) = rest.find("${") {
            output.push_str(&rest[..start]);
            let end = rest[start + 2..]
                .find('}')
                .ok_or("unterminated environment variable in publish-age setting")?
                + start
                + 2;
            let expression = &rest[start + 2..end];
            let replacement = if let Some((name, fallback)) = expression.split_once(":-") {
                self.get(name)
                    .filter(|value| !value.is_empty())
                    .unwrap_or(fallback)
            } else if let Some((name, fallback)) = expression.split_once('-') {
                self.get(name).unwrap_or(fallback)
            } else if let Some(name) = expression.strip_suffix('?') {
                self.get(name).unwrap_or("")
            } else {
                self.get(expression).ok_or_else(|| {
                    format!("unset environment variable {expression} in publish-age setting")
                })?
            };
            output.push_str(replacement);
            rest = &rest[end + 1..];
        }
        output.push_str(rest);
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_custom_paths_from_command_root() {
        let dir = tempfile::tempdir().unwrap();
        let mut env = ConfigEnv::default();
        env.0.insert("CONFIG_DIR".into(), "configs".into());
        env.0
            .insert("HOME".into(), dir.path().to_string_lossy().into());
        assert_eq!(
            env.path("${CONFIG_DIR}/npmrc", dir.path()).unwrap(),
            dir.path().join("configs/npmrc")
        );
        assert_eq!(
            env.path("~\\npmrc", dir.path()).unwrap(),
            dir.path().join("npmrc")
        );
        #[cfg(windows)]
        {
            assert_eq!(
                env.path("%config_dir%/npmrc", dir.path()).unwrap(),
                dir.path().join("configs/npmrc")
            );
            env.0.insert("CONFIG_DIR".into(), "%CONFIG_DIR%".into());
            assert!(env.path("%CONFIG_DIR%/npmrc", dir.path()).is_none());
        }
        assert_eq!(env.expand("${UNSET:-7} ${UNSET?}").unwrap(), "7 ");
    }
}
