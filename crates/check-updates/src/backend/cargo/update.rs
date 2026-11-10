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
    let entry = section
        .iter()
        .try_fold(doc.as_item_mut(), |item, key| {
            item.as_table_like_mut()?.get_mut(key)
        })
        .and_then(|section| section.as_table_like_mut()?.get_mut(&edit.toml_key));
    let Some(entry) = entry else {
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

    let value = if entry.is_str() {
        entry.as_value_mut()
    } else {
        entry
            .as_table_like_mut()
            .and_then(|table| table.get_mut("version"))
            .and_then(|version| version.as_value_mut())
    };
    let Some(value) = value else {
        return false;
    };
    let Some(original_version) = value.as_str() else {
        return false;
    };
    if original_version.parse::<VersionReq>().ok().as_ref() != Some(&edit.old_req) {
        return false;
    }

    let mut version = edit.new_req.to_string();
    if edit.preserve_bare
        && original_version
            .trim()
            .starts_with(|c: char| c.is_ascii_digit())
    {
        version = version.strip_prefix('^').unwrap_or(&version).to_string();
    }
    let decor = value.decor().clone();
    *value = toml_edit::Value::from(version);
    *value.decor_mut() = decor;
    true
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
    fn edits_requirements_without_changing_formatting() {
        for (input, section, key) in [
            ("[dependencies]\nserde = \"1.0\"\n", "dependencies", "serde"),
            (
                "[dependencies]\nserde = \"^1.0\"\n",
                "dependencies",
                "serde",
            ),
            (
                "[dependencies]\nserde={version=\"1.0\",features=[\"derive\"]}\n",
                "dependencies",
                "serde",
            ),
            (
                "[dependencies]\nserde = { version = \"1.0\", features = [\"derive\"] }\n",
                "dependencies",
                "serde",
            ),
            (
                "[dependencies.serde]\nversion = \"1.0\"\nfeatures = [\"derive\"]\n",
                "dependencies",
                "serde",
            ),
            (
                "[dependencies]\nmy_serde = { package = \"serde\", version = \"1.0\" }\n",
                "dependencies",
                "my_serde",
            ),
            (
                "[target.'cfg(unix)'.dependencies]\nserde = \"1.0\"\n",
                "dependencies",
                "serde",
            ),
            (
                "[target.'cfg(unix)'.dependencies.serde]\nversion = \"1.0\"\nfeatures = [\"derive\"]\n",
                "dependencies",
                "serde",
            ),
            ("[dependencies]\nserde=\"1.0\"\n", "dependencies", "serde"),
            (
                "[workspace.dependencies]\nserde = \"1.0\"\n",
                "workspace.dependencies",
                "serde",
            ),
            (
                "[dev-dependencies]\nserde = \"1.0\"\n",
                "dev-dependencies",
                "serde",
            ),
            (
                "[build-dependencies]\nserde = { version = \"1.0\" }\n",
                "build-dependencies",
                "serde",
            ),
            (
                "[package]\nname = \"my-crate\"\n\n# important comment\n[dependencies]\nserde = \"1.0\"\ntokio = \"3.0\"\n",
                "dependencies",
                "serde",
            ),
        ] {
            let result = edit_manifest(input, &[make_edit(section, key, "^2.0")]).unwrap();
            assert_eq!(result, input.replace("1.0", "2.0"), "{input}");
        }
    }

    #[test]
    fn batches_edits() {
        for section in ["dependencies", "workspace.dependencies"] {
            let input = format!(
                "[{section}]\nserde = \"1.0\"\ntokio = {{ version = \"1.0\", features = [\"full\"] }}\n"
            );
            let edits = [
                make_edit(section, "serde", "^2.0"),
                make_edit(section, "tokio", "^2.0"),
            ];
            assert_eq!(
                edit_manifest(&input, &edits).unwrap(),
                input.replace("1.0", "2.0")
            );
        }
    }

    #[test]
    fn skips_missing_or_inherited_requirements() {
        for input in [
            "[dev-dependencies]\nserde = \"1.0\"\n",
            "[dependencies]\ntokio = \"1.0\"\n",
            "[dependencies]\nserde = { workspace = true }\n",
            "[dependencies]\nserde = { path = \"../serde\" }\n",
        ] {
            assert_eq!(
                edit_manifest(input, &[make_edit("dependencies", "serde", "^2.0")]).unwrap(),
                input
            );
        }
    }

    #[test]
    fn edits_only_matching_target_requirements() {
        let input = "[dependencies]\nserde = \"1.0\"\n\n[target.'cfg(unix)'.dependencies]\nserde = \"1.0\"\n\n[target.'cfg(windows)'.dependencies]\nserde = \"2.0\"\n";
        let result = edit_manifest(input, &[make_edit("dependencies", "serde", "^3.0")]).unwrap();
        assert_eq!(result, input.replace("1.0", "3.0"));
    }
}
