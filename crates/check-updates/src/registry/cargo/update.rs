use std::path::Path;

use semver::VersionReq;

use super::CargoError;

pub(super) struct ManifestEdit {
    /// The TOML section path, e.g. "dependencies" or "workspace.dependencies".
    pub section: String,
    /// The key within that section (alias name or crate name).
    pub toml_key: String,
    /// The new version requirement to write.
    pub new_req: VersionReq,
    /// Whether to strip the ^ prefix for bare version requirements.
    pub preserve_bare: bool,
}

/// Apply a batch of version edits to a TOML document string, preserving
/// formatting and comments. Returns the modified document string.
pub(super) fn edit_manifest(contents: &str, edits: &[ManifestEdit]) -> Result<String, CargoError> {
    let mut doc = contents
        .parse::<toml_edit::DocumentMut>()
        .map_err(|e| CargoError::Metadata(format!("failed to parse manifest: {e}")))?;

    for edit in edits {
        let mut version_str = edit.new_req.to_string();

        // Navigate to the section (handles dotted paths like "workspace.dependencies").
        let section = edit
            .section
            .split('.')
            .try_fold(doc.as_item_mut(), |item, key| item.get_mut(key));

        let Some(section) = section.and_then(|s| s.as_table_like_mut()) else {
            log::warn!(
                "section [{}] not found; skipping '{}'",
                edit.section,
                edit.toml_key
            );
            continue;
        };

        let Some(entry) = section.get_mut(&edit.toml_key) else {
            log::warn!(
                "'{}' not found in [{}]; skipping",
                edit.toml_key,
                edit.section,
            );
            continue;
        };

        // Determine the original version string and whether it was bare
        let original_version = if entry.is_str() {
            entry.as_str()
        } else if let Some(table) = entry.as_table_like_mut() {
            table.get("version").and_then(|v| v.as_str())
        } else {
            None
        };

        let was_bare = edit.preserve_bare
            && original_version
                .map(|s| s.trim().starts_with(|c: char| c.is_ascii_digit()))
                .unwrap_or(false);

        if was_bare {
            version_str = version_str
                .strip_prefix('^')
                .unwrap_or(&version_str)
                .to_string();
        }

        if entry.is_str() {
            *entry = toml_edit::value(version_str);
        } else if let Some(table) = entry.as_table_like_mut() {
            if let Some(v) = table.get_mut("version") {
                *v = toml_edit::value(version_str);
            } else {
                log::warn!(
                    "'{}' in [{}] has no 'version' key; skipping",
                    edit.toml_key,
                    edit.section,
                );
            }
        } else {
            log::warn!(
                "unexpected value type for '{}' in [{}]; skipping",
                edit.toml_key,
                edit.section,
            );
        }
    }

    Ok(doc.to_string())
}

/// Apply a batch of version edits to a single Cargo.toml, preserving
/// formatting and comments.
pub(super) fn apply_manifest_edits(
    manifest: &Path,
    edits: &[ManifestEdit],
) -> Result<(), CargoError> {
    let metadata = std::fs::metadata(manifest)
        .map_err(|e| CargoError::Metadata(format!("failed to read {}: {e}", manifest.display())))?;

    if metadata.permissions().readonly() {
        return Err(CargoError::ReadOnly(manifest.to_path_buf()));
    }

    let contents = std::fs::read_to_string(manifest)
        .map_err(|e| CargoError::Metadata(format!("failed to read {}: {e}", manifest.display())))?;

    let result = edit_manifest(&contents, edits)?;

    std::fs::write(manifest, result).map_err(|e| {
        CargoError::Metadata(format!("failed to write {}: {e}", manifest.display()))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_edit(section: &str, key: &str, req: &str) -> ManifestEdit {
        ManifestEdit {
            section: section.to_string(),
            toml_key: key.to_string(),
            new_req: req.parse().unwrap(),
            preserve_bare: true,
        }
    }

    #[test]
    fn edit_simple_version_string() {
        let input = r#"
[dependencies]
serde = "1.0"
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        // Bare version gets ^ stripped
        assert_eq!(doc["dependencies"]["serde"].as_str().unwrap(), "2.0");
    }

    #[test]
    fn edit_inline_table_version() {
        let input = r#"
[dependencies]
serde = { version = "1.0", features = ["derive"] }
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let table = doc["dependencies"]["serde"].as_inline_table().unwrap();
        assert_eq!(table.get("version").unwrap().as_str().unwrap(), "2.0");
        // features should be preserved
        assert!(table.get("features").is_some());
    }

    #[test]
    fn edit_expanded_table_version() {
        let input = r#"
[dependencies.serde]
version = "1.0"
features = ["derive"]
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(
            doc["dependencies"]["serde"]["version"].as_str().unwrap(),
            "2.0"
        );
        // features should be preserved
        assert!(
            doc["dependencies"]["serde"]["features"]
                .as_array()
                .is_some()
        );
    }

    #[test]
    fn edit_workspace_dependencies() {
        let input = r#"
[workspace.dependencies]
tokio = "1.0"
serde = { version = "1.0", features = ["derive"] }
"#;
        let result = edit_manifest(
            input,
            &[
                make_edit("workspace.dependencies", "tokio", "^2.0"),
                make_edit("workspace.dependencies", "serde", "^2.0"),
            ],
        )
        .unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(
            doc["workspace"]["dependencies"]["tokio"].as_str().unwrap(),
            "2.0"
        );
        assert_eq!(
            doc["workspace"]["dependencies"]["serde"]
                .as_inline_table()
                .unwrap()
                .get("version")
                .unwrap()
                .as_str()
                .unwrap(),
            "2.0"
        );
    }

    #[test]
    fn edit_dev_and_build_deps() {
        let input = r#"
[dev-dependencies]
proptest = "1.0"

[build-dependencies]
cc = { version = "1.0" }
"#;
        let result = edit_manifest(
            input,
            &[
                make_edit("dev-dependencies", "proptest", "^2.0"),
                make_edit("build-dependencies", "cc", "^2.0"),
            ],
        )
        .unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(doc["dev-dependencies"]["proptest"].as_str().unwrap(), "2.0");
        assert_eq!(
            doc["build-dependencies"]["cc"]
                .as_inline_table()
                .unwrap()
                .get("version")
                .unwrap()
                .as_str()
                .unwrap(),
            "2.0"
        );
    }

    #[test]
    fn edit_renamed_package() {
        // When a dep is renamed, the TOML key is the alias, not the crate name.
        let input = r#"
[dependencies]
my_serde = { package = "serde", version = "1.0" }
"#;
        let result =
            edit_manifest(input, &[make_edit("dependencies", "my_serde", "^2.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        let table = doc["dependencies"]["my_serde"].as_inline_table().unwrap();
        assert_eq!(table.get("version").unwrap().as_str().unwrap(), "2.0");
        // package field should be preserved
        assert_eq!(table.get("package").unwrap().as_str().unwrap(), "serde");
    }

    #[test]
    fn edit_preserves_formatting() {
        let input = r#"[package]
name = "my-crate"

# important comment
[dependencies]
serde = "1.0"
tokio = "1.0"
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap();
        // Comment and other deps should be preserved
        assert!(result.contains("# important comment"));
        assert!(result.contains("name = \"my-crate\""));
        assert!(result.contains("tokio"));
    }

    #[test]
    fn edit_missing_section_is_skipped() {
        let input = r#"
[dependencies]
serde = "1.0"
"#;
        // Editing a section that doesn't exist should not error, just skip.
        let result = edit_manifest(input, &[make_edit("dev-dependencies", "foo", "^1.0")]).unwrap();
        // Document should be unchanged (modulo whitespace normalization by toml_edit)
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(doc["dependencies"]["serde"].as_str().unwrap(), "1.0");
    }

    #[test]
    fn edit_missing_key_is_skipped() {
        let input = r#"
[dependencies]
serde = "1.0"
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "tokio", "^1.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(doc["dependencies"]["serde"].as_str().unwrap(), "1.0");
        assert!(doc["dependencies"].get("tokio").is_none());
    }

    #[test]
    fn edit_multiple_deps_same_section() {
        let input = r#"
[dependencies]
serde = "1.0"
tokio = { version = "1.0", features = ["full"] }
"#;
        let result = edit_manifest(
            input,
            &[
                make_edit("dependencies", "serde", "^2.0"),
                make_edit("dependencies", "tokio", "^2.0"),
            ],
        )
        .unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        // Bare versions get ^ stripped
        assert_eq!(doc["dependencies"]["serde"].as_str().unwrap(), "2.0");
        assert_eq!(
            doc["dependencies"]["tokio"]
                .as_inline_table()
                .unwrap()
                .get("version")
                .unwrap()
                .as_str()
                .unwrap(),
            "2.0"
        );
    }

    #[test]
    fn edit_preserves_caret_prefix() {
        let input = r#"
[dependencies]
serde = "^1.0"
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        // Non-bare versions keep the ^
        assert_eq!(doc["dependencies"]["serde"].as_str().unwrap(), "^2.0");
    }
}
