use std::collections::BTreeMap;
use std::path::Path;

use granit_parser::{Event, Parser};
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageJson {
    pub name: Option<String>,
    #[serde(default)]
    pub workspaces: Option<Workspaces>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub peer_dependencies: BTreeMap<String, String>,
    pub package_manager: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum Workspaces {
    Patterns(Vec<String>),
    Config { packages: Vec<String> },
}

impl Workspaces {
    pub fn patterns(&self) -> &[String] {
        match self {
            Self::Patterns(patterns) => patterns,
            Self::Config { packages } => packages,
        }
    }
}

impl PackageJson {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        serde_json::from_str(&contents).map_err(|error| error.to_string())
    }
}

#[derive(Default)]
pub(super) struct PnpmWorkspace {
    pub packages: Vec<String>,
}

impl PnpmWorkspace {
    pub fn from_root(root: &Path) -> Result<Self, String> {
        let path = root.join("pnpm-workspace.yaml");
        if !path.is_file() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(&contents).map_err(|error| format!("pnpm-workspace.yaml: {error}"))
    }

    fn parse(contents: &str) -> Result<Self, String> {
        let mut events = Parser::new_from_str(contents).filter_map(|result| match result {
            Ok((
                Event::StreamStart
                | Event::StreamEnd
                | Event::DocumentStart(..)
                | Event::DocumentEnd,
                _,
            )) => None,
            Ok((event, _)) => Some(Ok(event)),
            Err(error) => Some(Err(format!(
                "invalid YAML at line {}, column {}",
                error.marker().line(),
                error.marker().col() + 1
            ))),
        });
        match events.next().transpose()? {
            Some(Event::MappingStart(..)) => {}
            None => return Ok(Self::default()),
            _ => return Err("expected a workspace mapping".into()),
        }
        let mut packages = None;
        while let Some(event) = events.next().transpose()? {
            let key = match event {
                Event::Scalar(key, ..) => key,
                Event::MappingEnd => {
                    if events.next().transpose()?.is_some() {
                        return Err("expected a single workspace document".into());
                    }
                    return Ok(Self {
                        packages: packages.unwrap_or_default(),
                    });
                }
                _ => return Err("expected a scalar workspace key".into()),
            };
            if key == "<<" {
                return Err("YAML merge keys are not supported in pnpm workspaces".into());
            }
            let value = events
                .next()
                .transpose()?
                .ok_or("missing workspace value")?;
            if key == "packages" {
                if packages.is_some() {
                    return Err("duplicate workspace packages field".into());
                }
                if !matches!(value, Event::SequenceStart(..)) {
                    return Err(
                        "workspace packages must be a sequence, not a scalar or alias".into(),
                    );
                }
                let mut patterns = Vec::new();
                loop {
                    match events.next().transpose()? {
                        Some(Event::Scalar(pattern, _, _, tag))
                            if tag
                                .as_ref()
                                .is_none_or(|tag| tag.is_yaml_core_schema_tag("str")) =>
                        {
                            patterns.push(pattern.into_owned());
                        }
                        Some(Event::SequenceEnd) => break,
                        _ => {
                            return Err(
                                "expected scalar workspace patterns, not collections or aliases"
                                    .into(),
                            );
                        }
                    }
                }
                packages = Some(patterns);
            } else {
                // Ignore unrelated settings, including nested catalogs and overrides.
                let mut depth = usize::from(matches!(
                    value,
                    Event::MappingStart(..) | Event::SequenceStart(..)
                ));
                while depth > 0 {
                    match events.next().transpose()? {
                        Some(Event::MappingStart(..) | Event::SequenceStart(..)) => depth += 1,
                        Some(Event::MappingEnd | Event::SequenceEnd) => depth -= 1,
                        Some(_) => {}
                        None => return Err("unterminated workspace setting".into()),
                    }
                }
            }
        }
        Err("unterminated workspace mapping".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_workspace_patterns() {
        for input in [
            "packages:\n  - packages/*\n  - '!packages/skip'\n",
            "# workspace\npackages: [\"packages/*\", '!packages/skip'] # members\n",
            "catalog:\n  packages: {nested: [ignored]}\npackages: [packages/*, '!packages/skip']\nsettings:\n  packages: [ignored]\n",
            "packages: &patterns [!!str packages/*, '!packages/skip']\nother: *patterns\n",
            "---\n'packages':\n  - \"packages\\u002f*\"\n  - '!packages/skip'\n...\n",
        ] {
            assert_eq!(
                PnpmWorkspace::parse(input).unwrap().packages,
                ["packages/*", "!packages/skip"],
                "{input}"
            );
        }
        for input in [
            "",
            "# empty workspace\n",
            "{}",
            "packages: []",
            "catalog: {react: '^19'}",
        ] {
            assert!(
                PnpmWorkspace::parse(input).unwrap().packages.is_empty(),
                "{input}"
            );
        }
    }

    #[test]
    fn rejects_invalid_workspace_patterns() {
        for input in [
            "packages: [",
            "[packages/*]",
            "packages: packages/*",
            "packages: {app: packages/*}",
            "packages: [[packages/*]]",
            "packages: [{app: packages/*}]",
            "packages: [!!int 123]",
            "patterns: &patterns [packages/*]\npackages: *patterns",
            "pattern: &pattern packages/*\npackages: [*pattern]",
            "packages: [packages/*]\npackages: [apps/*]",
            "defaults: &defaults {packages: [packages/*]}\n<<: *defaults",
            "packages: [packages/*]\n---\npackages: [apps/*]",
            "settings: {nested: [",
        ] {
            assert!(PnpmWorkspace::parse(input).is_err(), "{input}");
        }
    }
}
