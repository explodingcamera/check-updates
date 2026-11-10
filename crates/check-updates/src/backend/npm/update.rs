use std::collections::BTreeMap;
use std::path::Path;

use serde_json::{Value, value::RawValue};

use crate::{DepKind, Package, Requirement, Unit, Usage};

/// Apply selected npm requirements from the shared package model.
pub fn update_packages<'a>(
    selected: impl IntoIterator<Item = (&'a Usage, &'a Package, Requirement)>,
) -> Result<(), String> {
    let mut by_manifest: BTreeMap<&Path, Vec<(&str, String, String, String)>> = BTreeMap::new();
    for (usage, package, proposed) in selected {
        if !matches!(usage.req, Requirement::Npm { .. })
            || !matches!(proposed, Requirement::Npm { .. })
            || package.purl.package_type() != "npm"
        {
            return Err("expected an npm dependency update".into());
        }
        let manifest = match &usage.unit {
            Unit::Project { manifest, .. } | Unit::Workspace { manifest } => manifest,
            Unit::Global => return Err("expected an npm project manifest".into()),
        };
        let section = match usage.kind {
            DepKind::Normal => "dependencies",
            DepKind::Dev => "devDependencies",
            DepKind::Optional => "optionalDependencies",
            DepKind::Peer => "peerDependencies",
            DepKind::Build => return Err("unsupported npm dependency section".into()),
        };
        by_manifest.entry(manifest).or_default().push((
            section,
            package.purl.name().to_string(),
            usage.req.to_string(),
            proposed.to_string(),
        ));
    }
    for (path, edits) in by_manifest {
        edit_manifest(path, &edits)?;
    }
    Ok(())
}

fn edit_manifest(path: &Path, edits: &[(&str, String, String, String)]) -> Result<(), String> {
    let mut text = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
    let mut expected: Value = serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let document: BTreeMap<String, &RawValue> =
        serde_json::from_str(&text).map_err(|error| error.to_string())?;
    let mut sections = BTreeMap::new();
    let mut replacements = BTreeMap::new();
    for (section, name, old, new) in edits {
        let value = expected
            .get_mut(*section)
            .and_then(Value::as_object_mut)
            .and_then(|entries| entries.get_mut(name))
            .ok_or_else(|| format!("missing {name} in {}", path.display()))?;
        if value.as_str() != Some(old.as_str()) {
            return Err(format!("changed {name} in {}", path.display()));
        }
        *value = Value::String(new.clone());
        if !sections.contains_key(*section) {
            let tokens =
                serde_json::from_str::<BTreeMap<String, &RawValue>>(document[*section].get())
                    .map_err(|error| error.to_string())?;
            sections.insert(*section, tokens);
        }
        let token = sections[*section]
            .get(name)
            .ok_or_else(|| format!("missing {name} in {}", path.display()))?
            .get();
        // Borrowed RawValue strings are slices of the original document.
        let start = token.as_ptr().addr() - text.as_ptr().addr();
        let replacement = serde_json::to_string(new).map_err(|error| error.to_string())?;
        if replacements
            .insert(start, (start + token.len(), replacement))
            .is_some()
        {
            return Err(format!("duplicate update for {name} in {}", path.display()));
        }
    }
    // Replace later tokens first so earlier byte offsets remain valid.
    for (start, (end, replacement)) in replacements.into_iter().rev() {
        text.replace_range(start..end, &replacement);
    }
    if serde_json::from_str::<Value>(&text).map_err(|error| error.to_string())? != expected {
        return Err(format!("could not safely edit {}", path.display()));
    }
    std::fs::write(path, text).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_top_level_values_without_changing_formatting() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        for (input, expected) in [
            (
                r#"{"config":{"dependencies":{"foo":"^1.0.0"}},"dependencies":{"foo":"^1.0.0"}}"#,
                r#"{"config":{"dependencies":{"foo":"^1.0.0"}},"dependencies":{"foo":"^2.0.0"}}"#,
            ),
            (
                r#"{"depend\u0065ncies":{"f\u006fo":"\u005e1.0.0"}}"#,
                r#"{"depend\u0065ncies":{"f\u006fo":"^2.0.0"}}"#,
            ),
            (
                r#"{"description":"} \"dependencies\": {\"foo\":\"^1.0.0\"}","dependencies":{"foo":"^1.0.0"}}"#,
                r#"{"description":"} \"dependencies\": {\"foo\":\"^1.0.0\"}","dependencies":{"foo":"^2.0.0"}}"#,
            ),
            (
                r#"{"name":"café🐈","dependencies":{"foo":"^1.0.0"}}"#,
                r#"{"name":"café🐈","dependencies":{"foo":"^2.0.0"}}"#,
            ),
            (
                r#"{"dependencies":{"foo":"^0.0.0","foo":"^1.0.0"}}"#,
                r#"{"dependencies":{"foo":"^0.0.0","foo":"^2.0.0"}}"#,
            ),
            (
                r#"{"dependencies":{"foo":"^0.0.0"},"dependencies":{"foo":"^1.0.0"}}"#,
                r#"{"dependencies":{"foo":"^0.0.0"},"dependencies":{"foo":"^2.0.0"}}"#,
            ),
            (
                "{\r\n  \"dependencies\" : { \"foo\" : \"^1.0.0\" },\r\n  \"devDependencies\": {\"foo\":\"^1.0.0\"}\r\n}\r\n",
                "{\r\n  \"dependencies\" : { \"foo\" : \"^2.0.0\" },\r\n  \"devDependencies\": {\"foo\":\"^1.0.0\"}\r\n}\r\n",
            ),
        ] {
            std::fs::write(&path, input).unwrap();
            edit_manifest(
                &path,
                &[(
                    "dependencies",
                    "foo".into(),
                    "^1.0.0".into(),
                    "^2.0.0".into(),
                )],
            )
            .unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), expected, "{input}");
        }
    }

    #[test]
    fn batches_edits_with_different_lengths() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        let input = "{\n  \"dependencies\": {\"foo\":\"^1.0.0\", \"bar\":\"~1.0.0\"},\n  \"devDependencies\": {\"foo\":\"^1.0.0\"}\n}\n";
        std::fs::write(&path, input).unwrap();
        edit_manifest(
            &path,
            &[
                (
                    "devDependencies",
                    "foo".into(),
                    "^1.0.0".into(),
                    "^123.0.0".into(),
                ),
                ("dependencies", "bar".into(), "~1.0.0".into(), "~12".into()),
                (
                    "dependencies",
                    "foo".into(),
                    "^1.0.0".into(),
                    "^2.0.0".into(),
                ),
            ],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(path).unwrap(),
            "{\n  \"dependencies\": {\"foo\":\"^2.0.0\", \"bar\":\"~12\"},\n  \"devDependencies\": {\"foo\":\"^123.0.0\"}\n}\n"
        );
    }

    #[test]
    fn leaves_manifest_unchanged_on_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        for input in [
            r#"{"dependencies":{"foo":"^9.0.0"}}"#,
            r#"{"dependencies":{"bar":"^1.0.0"}}"#,
            r#"{"config":{"dependencies":{"foo":"^1.0.0"}}}"#,
            r#"{"dependencies":{"foo":123}}"#,
            r#"{"dependencies":[]}"#,
            r#"{"dependencies":{"foo":"^1.0.0"},}"#,
        ] {
            std::fs::write(&path, input).unwrap();
            assert!(
                edit_manifest(
                    &path,
                    &[(
                        "dependencies",
                        "foo".into(),
                        "^1.0.0".into(),
                        "^2.0.0".into()
                    )]
                )
                .is_err(),
                "{input}"
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), input);
        }
        let input = r#"{"dependencies":{"foo":"^1.0.0"}}"#;
        std::fs::write(&path, input).unwrap();
        assert!(
            edit_manifest(
                &path,
                &[
                    (
                        "dependencies",
                        "foo".into(),
                        "^1.0.0".into(),
                        "^2.0.0".into()
                    ),
                    (
                        "dependencies",
                        "foo".into(),
                        "^2.0.0".into(),
                        "^3.0.0".into()
                    ),
                ]
            )
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), input);
    }
}
