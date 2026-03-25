use std::path::Path;

use semver::VersionReq;

use super::CargoError;

#[derive(Clone)]
pub(super) struct ManifestEdit {
    /// The TOML section path segments, e.g. ["dependencies"] or
    /// ["workspace", "dependencies"].
    pub section: Vec<String>,
    /// The key within that section (alias name or crate name).
    pub toml_key: String,
    /// The new version requirement to write.
    pub new_req: VersionReq,
    /// The currently expected version requirement to match before editing.
    pub old_req: VersionReq,
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
        let mut section_paths = vec![edit.section.clone()];
        if let [dep_kind] = edit.section.as_slice()
            && matches!(
                dep_kind.as_str(),
                "dependencies" | "dev-dependencies" | "build-dependencies"
            )
            && let Some(target) = doc.get("target").and_then(|v| v.as_table())
        {
            for (target_name, target_item) in target {
                let Some(target_table) = target_item.as_table() else {
                    continue;
                };

                let Some(dep_table) = target_table
                    .get(dep_kind.as_str())
                    .and_then(|v| v.as_table())
                else {
                    continue;
                };

                if dep_table.contains_key(&edit.toml_key) {
                    section_paths.push(vec![
                        "target".to_string(),
                        target_name.to_string(),
                        dep_kind.clone(),
                    ]);
                }
            }
        }

        let mut touched_any = false;

        for section_path in section_paths {
            if apply_single_edit(&mut doc, &section_path, edit) {
                touched_any = true;
            }
        }

        if !touched_any {
            log::warn!(
                "'{}' not found in [{}]; skipping",
                edit.toml_key,
                edit.section.join("."),
            );
        }
    }

    Ok(doc.to_string())
}

fn apply_single_edit(
    doc: &mut toml_edit::DocumentMut,
    section: &[String],
    edit: &ManifestEdit,
) -> bool {
    let mut version_str = edit.new_req.to_string();
    let section_name = section.join(".");

    let section_item = section
        .iter()
        .try_fold(doc.as_item_mut(), |item, key| item.get_mut(key));

    let Some(section_table) = section_item.and_then(|s| s.as_table_like_mut()) else {
        return false;
    };

    let Some(entry) = section_table.get_mut(&edit.toml_key) else {
        return false;
    };

    if entry
        .as_table_like()
        .and_then(|table| table.get("workspace"))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return false;
    }

    let original_version = if entry.is_str() {
        entry.as_str()
    } else if let Some(table) = entry.as_table_like_mut() {
        table.get("version").and_then(|v| v.as_str())
    } else {
        None
    };

    let Some(original_req) = original_version.and_then(|v| v.parse::<VersionReq>().ok()) else {
        return false;
    };

    if original_req != edit.old_req {
        return false;
    }

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
        return true;
    }

    if let Some(table) = entry.as_table_like_mut() {
        if let Some(v) = table.get_mut("version") {
            *v = toml_edit::value(version_str);
            return true;
        }

        log::warn!(
            "'{}' in [{}] has no 'version' key; skipping",
            edit.toml_key,
            section_name,
        );
        return false;
    }

    log::warn!(
        "unexpected value type for '{}' in [{}]; skipping",
        edit.toml_key,
        section_name,
    );
    false
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
            section: section.split('.').map(ToString::to_string).collect(),
            toml_key: key.to_string(),
            new_req: req.parse().unwrap(),
            old_req: "^1.0".parse().unwrap(),
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

    #[test]
    fn edit_target_dependencies() {
        let input = r#"
[target.'cfg(unix)'.dependencies]
serde = "1.0"
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(
            doc["target"]["cfg(unix)"]["dependencies"]["serde"]
                .as_str()
                .unwrap(),
            "2.0"
        );
    }

    #[test]
    fn edit_target_expanded_table_dependency() {
        let input = r#"
[target.'cfg(unix)'.dependencies.serde]
version = "1.0"
features = ["derive"]
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();
        assert_eq!(
            doc["target"]["cfg(unix)"]["dependencies"]["serde"]["version"]
                .as_str()
                .unwrap(),
            "2.0"
        );
        assert!(
            doc["target"]["cfg(unix)"]["dependencies"]["serde"]["features"]
                .as_array()
                .is_some()
        );
    }

    #[test]
    fn edit_target_dependencies_only_matching_current_req() {
        let input = r#"
[dependencies]
serde = "1.0"

[target.'cfg(unix)'.dependencies]
serde = "1.0"

[target.'cfg(windows)'.dependencies]
serde = "2.0"
"#;
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^3.0")]).unwrap();
        let doc: toml_edit::DocumentMut = result.parse().unwrap();

        assert_eq!(doc["dependencies"]["serde"].as_str().unwrap(), "3.0");
        assert_eq!(
            doc["target"]["cfg(unix)"]["dependencies"]["serde"]
                .as_str()
                .unwrap(),
            "3.0"
        );
        // Different current requirement should not be rewritten.
        assert_eq!(
            doc["target"]["cfg(windows)"]["dependencies"]["serde"]
                .as_str()
                .unwrap(),
            "2.0"
        );
    }
}
