use jiff::{Span, Timestamp};
use std::path::{Path, PathBuf};

use crate::config::{ConfigEnv, read_toml};
use crate::publish_age::{PublishAge, parse_span};

#[derive(Default)]
struct Settings {
    global: Option<Span>,
    registry: Option<Span>,
    allow: bool,
}

/// Resolve the publication cutoff from Cargo settings, environment, and explicit overrides.
pub(super) fn load(
    root: &Path,
    minimum: Option<Span>,
    ignore: bool,
    env: &ConfigEnv,
) -> Result<PublishAge, String> {
    if ignore || minimum.is_some() {
        return PublishAge::try_new(if ignore { None } else { minimum }, Timestamp::now());
    }
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let home = env
        .get("CARGO_HOME")
        .filter(|value| !value.is_empty())
        .and_then(|value| env.path(value, &root))
        .or_else(|| env.user_home().map(|home| home.join(".cargo")));
    let mut directories = Vec::new();
    if let Some(home) = home {
        directories.push(home);
    }
    for parent in root.ancestors().collect::<Vec<_>>().into_iter().rev() {
        let directory = parent.join(".cargo");
        // Cargo home is read once, with the lowest precedence.
        if !directories.contains(&directory) {
            directories.push(directory);
        }
    }
    let mut settings = Settings::default();
    for directory in directories {
        let legacy = directory.join("config");
        let path = if legacy.is_file() {
            legacy
        } else {
            directory.join("config.toml")
        };
        if let Err(error) = settings.read(&path, &mut Vec::new()) {
            log::warn!("ignoring invalid Cargo configuration: {error}");
        }
    }
    for (key, minimum) in [
        (
            "CARGO_REGISTRY_GLOBAL_MIN_PUBLISH_AGE",
            &mut settings.global,
        ),
        (
            "CARGO_REGISTRIES_CRATES_IO_MIN_PUBLISH_AGE",
            &mut settings.registry,
        ),
    ] {
        if let Some(value) = env.get(key) {
            match parse_span(value) {
                Ok(age) => *minimum = Some(age),
                Err(error) => log::warn!("ignoring invalid {key}: {error}"),
            }
        }
    }
    if let Some(value) = env.get("CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE") {
        match parse_behavior(value) {
            Ok(allow) => settings.allow = allow,
            Err(error) => {
                log::warn!("ignoring invalid CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE: {error}")
            }
        }
    }
    PublishAge::try_new(
        if settings.allow {
            None
        } else {
            settings.registry.or(settings.global)
        },
        Timestamp::now(),
    )
}

impl Settings {
    fn read(&mut self, path: &Path, stack: &mut Vec<PathBuf>) -> Result<(), String> {
        let Some(doc) = read_toml(path)? else {
            return Ok(());
        };
        let absolute = path.canonicalize().map_err(|error| error.to_string())?;
        if stack.contains(&absolute) || stack.len() >= 64 {
            return Err(format!(
                "{}: recursive Cargo config include",
                path.display()
            ));
        }
        stack.push(absolute);
        let result = (|| {
            if let Some(includes) = doc.get("include") {
                let includes = includes
                    .as_array()
                    .ok_or("Cargo config include must be an array")?;
                for include in includes {
                    let (include, optional) = if let Some(path) = include.as_str() {
                        (path, false)
                    } else {
                        let table = include
                            .as_inline_table()
                            .ok_or("invalid Cargo config include")?;
                        (
                            table
                                .get("path")
                                .and_then(|value| value.as_str())
                                .ok_or("missing Cargo config include path")?,
                            table
                                .get("optional")
                                .and_then(|value| value.as_bool())
                                .unwrap_or(false),
                        )
                    };
                    let include = path.parent().unwrap_or(Path::new(".")).join(include);
                    if !include.is_file() && !optional {
                        return Err(format!(
                            "missing Cargo config include {}",
                            include.display()
                        ));
                    }
                    self.read(&include, stack)?;
                }
            }
            for (table, key, minimum) in [
                (
                    doc.get("registry"),
                    "global-min-publish-age",
                    &mut self.global,
                ),
                (
                    doc.get("registries")
                        .and_then(|value| value.get("crates-io")),
                    "min-publish-age",
                    &mut self.registry,
                ),
            ] {
                if let Some(value) = table.and_then(|table| table.get(key)) {
                    match value
                        .as_str()
                        .ok_or_else(|| format!("{key} must be a string"))
                        .and_then(parse_span)
                    {
                        Ok(age) => *minimum = Some(age),
                        Err(error) => {
                            log::warn!("{}: ignoring invalid {key}: {error}", path.display())
                        }
                    }
                }
            }
            if let Some(value) = doc
                .get("resolver")
                .and_then(|v| v.get("incompatible-publish-age"))
            {
                match value
                    .as_str()
                    .ok_or_else(|| "incompatible-publish-age must be a string".to_string())
                    .and_then(parse_behavior)
                {
                    Ok(allow) => self.allow = allow,
                    Err(error) => log::warn!(
                        "{}: ignoring invalid incompatible-publish-age: {error}",
                        path.display()
                    ),
                }
            }
            Ok(())
        })();
        stack.pop();
        result.map_err(|error: String| format!("{}: {error}", path.display()))
    }
}

fn parse_behavior(value: &str) -> Result<bool, String> {
    match value {
        "allow" => Ok(true),
        "deny" => Ok(false),
        _ => Err("incompatible-publish-age must be allow or deny".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_layers_and_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let root = dir.path().join("project/member");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(root.join(".cargo")).unwrap();
        std::fs::create_dir_all(dir.path().join("project/.cargo")).unwrap();
        // A registry-specific value takes precedence even over a project global value.
        std::fs::write(home.join("config.toml"), "[registry]\nglobal-min-publish-age = '1 day'\n[registries.crates-io]\nmin-publish-age = '0'\n").unwrap();
        std::fs::write(
            dir.path().join("project/.cargo/config.toml"),
            "[registry]\nglobal-min-publish-age = '7 days'\n",
        )
        .unwrap();
        let mut env = ConfigEnv::default();
        env.0.insert("CARGO_HOME".into(), "../../home".into());
        assert!(!load(&root, None, false, &env).unwrap().enabled());
        env.0.insert(
            "CARGO_REGISTRIES_CRATES_IO_MIN_PUBLISH_AGE".into(),
            "7 days".into(),
        );
        assert!(load(&root, None, false, &env).unwrap().enabled());
        env.0.insert(
            "CARGO_REGISTRIES_CRATES_IO_MIN_PUBLISH_AGE".into(),
            "invalid".into(),
        );
        assert!(!load(&root, None, false, &env).unwrap().enabled());
        assert!(
            !load(&root, Some(Span::new()), false, &env)
                .unwrap()
                .enabled()
        );
        std::fs::write(
            root.join(".cargo/config"),
            "[resolver]\nincompatible-publish-age = 'allow'\n",
        )
        .unwrap();
        assert!(!load(&root, None, false, &env).unwrap().enabled());
        assert!(
            load(&root, Some(Span::new().minutes(1)), false, &env)
                .unwrap()
                .enabled()
        );
        assert!(
            !load(&root, Some(Span::new().minutes(1)), true, &env)
                .unwrap()
                .enabled()
        );
    }

    #[test]
    fn reads_includes_and_rejects_cycles() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(
            dir.path().join("shared.toml"),
            "[registry]\nglobal-min-publish-age = '7 days'\n",
        )
        .unwrap();
        std::fs::write(
            &path,
            "include = ['shared.toml', {path = 'missing.toml', optional = true}]\n",
        )
        .unwrap();
        let mut settings = Settings::default();
        settings.read(&path, &mut Vec::new()).unwrap();
        assert_eq!(settings.global.unwrap().get_days(), 7);
        std::fs::write(
            dir.path().join("shared.toml"),
            "include = ['config.toml']\n",
        )
        .unwrap();
        assert!(
            settings
                .read(&path, &mut Vec::new())
                .unwrap_err()
                .contains("recursive")
        );
    }
}
