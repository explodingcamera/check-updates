use std::path::Path;

use granit_parser::{Event, Parser};

/// Workspace members and publication-age settings from pnpm-workspace.yaml.
#[derive(Default)]
pub(crate) struct PnpmWorkspace {
    pub packages: Vec<String>,
    /// Native age value in minutes, possibly containing an environment reference.
    pub minimum_release_age: Option<String>,
    /// Package or exact-version exemptions, replacing lower-priority lists.
    pub minimum_release_age_exclude: Option<Vec<String>>,
    /// Whether undated versions bypass the age filter.
    pub minimum_release_age_ignore_missing_time: Option<String>,
}

impl PnpmWorkspace {
    /// Read workspace settings from the given project root.
    pub(crate) fn from_root(root: &Path) -> Result<Self, String> {
        let path = root.join("pnpm-workspace.yaml");
        if !path.is_file() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        Self::parse(&contents).map_err(|error| format!("pnpm-workspace.yaml: {error}"))
    }

    /// Read workspace patterns and age settings while skipping unrelated YAML values.
    pub(crate) fn parse(contents: &str) -> Result<Self, String> {
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
        let mut settings = Self::default();
        while let Some(event) = events.next().transpose()? {
            let key = match event {
                Event::Scalar(key, ..) => key,
                Event::MappingEnd => {
                    if events.next().transpose()?.is_some() {
                        return Err("expected a single workspace document".into());
                    }
                    return Ok(Self {
                        packages: packages.unwrap_or_default(),
                        ..settings
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
            if key == "packages"
                || (key == "minimumReleaseAgeExclude" && matches!(value, Event::SequenceStart(..)))
            {
                let field = if key == "packages" {
                    &mut packages
                } else {
                    &mut settings.minimum_release_age_exclude
                };
                if field.is_some() && key == "packages" {
                    return Err(format!("duplicate workspace {key} field"));
                }
                if !matches!(value, Event::SequenceStart(..)) {
                    return Err(format!(
                        "workspace {key} must be a sequence, not a scalar or alias"
                    ));
                }
                let mut patterns = Vec::new();
                let mut invalid = false;
                let mut depth = 1;
                loop {
                    match events.next().transpose()? {
                        Some(Event::Scalar(pattern, _, _, tag))
                            if depth == 1
                                && tag
                                    .as_ref()
                                    .is_none_or(|tag| tag.is_yaml_core_schema_tag("str")) =>
                        {
                            patterns.push(pattern.into_owned());
                        }
                        Some(Event::SequenceEnd) if depth == 1 => break,
                        Some(event) if key != "packages" => {
                            invalid = true;
                            match event {
                                Event::MappingStart(..) | Event::SequenceStart(..) => depth += 1,
                                Event::MappingEnd | Event::SequenceEnd => depth -= 1,
                                _ => {}
                            }
                        }
                        _ => {
                            return Err(
                                "expected scalar workspace patterns, not collections or aliases"
                                    .into(),
                            );
                        }
                    }
                }
                if invalid {
                    log::warn!(
                        "ignoring invalid minimumReleaseAgeExclude, expected a list of strings"
                    );
                } else {
                    *field = Some(patterns);
                }
            } else if (key == "minimumReleaseAge" || key == "minimumReleaseAgeIgnoreMissingTime")
                && matches!(value, Event::Scalar(..))
            {
                let field = if key == "minimumReleaseAge" {
                    &mut settings.minimum_release_age
                } else {
                    &mut settings.minimum_release_age_ignore_missing_time
                };
                let Event::Scalar(value, ..) = value else {
                    return Err(format!("workspace {key} must be a scalar"));
                };
                *field = Some(value.into_owned());
            } else {
                if matches!(
                    key.as_ref(),
                    "minimumReleaseAge"
                        | "minimumReleaseAgeExclude"
                        | "minimumReleaseAgeIgnoreMissingTime"
                ) {
                    log::warn!("ignoring invalid {key}");
                }
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
            "packages: [packages/*, '!packages/skip']\nminimumReleaseAge: [invalid]\nminimumReleaseAgeExclude: [valid, {bad: value}]\n",
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
