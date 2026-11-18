use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use cargo_metadata::{Metadata, MetadataCommand};
use semver::Version;

use super::{CargoError, resolve::CrateMeta};

/// Read workspace declarations without resolving or downloading dependencies.
pub(super) fn workspace(root: Option<&Path>) -> Result<Metadata, CargoError> {
    let mut command = MetadataCommand::new();
    if let Some(root) = root {
        command.current_dir(root);
    }
    command
        .no_deps()
        .exec()
        .map_err(|error| CargoError::Metadata(error.to_string()))
}

/// Read links from the newest cached sources for direct crates.io dependencies.
pub(super) fn cached_crate_meta(
    sources: &Path,
    names: &HashSet<String>,
) -> HashMap<String, CrateMeta> {
    if names.is_empty() {
        return HashMap::new();
    }
    let mut latest: HashMap<String, (Version, PathBuf)> = HashMap::new();
    for registry in std::fs::read_dir(sources).into_iter().flatten().flatten() {
        if !registry
            .file_name()
            .to_string_lossy()
            .starts_with("index.crates.io-")
        {
            continue;
        }
        for entry in std::fs::read_dir(registry.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let directory = entry.file_name();
            let Some(directory) = directory.to_str() else {
                continue;
            };
            // Crate names and prerelease versions can both contain hyphens.
            for (separator, _) in directory.match_indices('-') {
                let name = &directory[..separator];
                if !names.contains(name) {
                    continue;
                }
                let Ok(version) = Version::parse(&directory[separator + 1..]) else {
                    continue;
                };
                if latest
                    .get(name)
                    .is_none_or(|(current, _)| *current < version)
                {
                    latest.insert(name.to_string(), (version, entry.path()));
                }
                break;
            }
        }
    }
    latest
        .into_iter()
        .filter_map(|(name, (_, path))| {
            let text = std::fs::read_to_string(path.join("Cargo.toml")).ok()?;
            let document = text.parse::<toml_edit::DocumentMut>().ok()?;
            let package = document.get("package")?;
            Some((
                name,
                CrateMeta {
                    repository: package
                        .get("repository")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                    homepage: package
                        .get("homepage")
                        .and_then(|v| v.as_str())
                        .map(str::to_owned),
                },
            ))
        })
        .collect()
}
