use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::{DepKind, Package, PackageVersion, Packages, Purl, Requirement, Unit, Usage, Version};

mod manifest;

use manifest::{PackageJson, PnpmWorkspace};

/// Package manager selected from a lockfile or `packageManager` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Bun,
}

/// Detect the tool that owns this project's lockfile, if one exists.
pub fn package_manager(root: &Path) -> Result<PackageManager, String> {
    let lockfiles = [
        ("package-lock.json", PackageManager::Npm),
        ("npm-shrinkwrap.json", PackageManager::Npm),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
    ];
    for unsupported in ["yarn.lock", "aube.lock"] {
        if root.join(unsupported).is_file() {
            return Err(format!("unsupported lockfile '{unsupported}'"));
        }
    }
    let detected: Vec<_> = lockfiles
        .iter()
        .filter(|(file, _)| root.join(file).is_file())
        .collect();
    if detected.len() > 1 {
        return Err(format!("multiple lockfiles found in {}", root.display()));
    }
    if let Some((_, manager)) = detected.first() {
        return Ok(*manager);
    }
    let manifest = PackageJson::from_path(&root.join("package.json"))?;
    let manager = manifest
        .package_manager
        .as_deref()
        .map(|value| value.split_once('@').map_or(value, |(name, _)| name));
    match manager {
        Some("pnpm") => Ok(PackageManager::Pnpm),
        Some("bun") => Ok(PackageManager::Bun),
        Some("npm") | None => Ok(PackageManager::Npm),
        Some(other) => Err(format!("unsupported package manager '{other}'")),
    }
}

/// Options for discovering npm packages.
#[derive(Debug, Default)]
pub struct NpmOptions<'a> {
    pub packages: &'a [String],
}

/// Discovered npm projects and their package versions.
#[derive(Debug)]
pub struct NpmPackages {
    pub projects: Vec<String>,
    pub packages: Packages,
}

struct Dependency {
    alias: String,
    name: String,
    declared: String,
    section: &'static str,
}

struct Project {
    manifest: PathBuf,
    name: String,
    dependencies: Vec<Dependency>,
}

/// Discover direct npm dependencies and their published versions without modifying project files.
pub async fn packages(root: &Path, options: &NpmOptions<'_>) -> Result<NpmPackages, String> {
    let root_manifest = PackageJson::from_path(&root.join("package.json"))?;
    let workspace = PnpmWorkspace::from_root(root)?;
    let mut paths = BTreeSet::from([root.join("package.json")]);
    let mut excluded = BTreeSet::new();
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let patterns = root_manifest
        .workspaces
        .as_ref()
        .map(|workspaces| workspaces.patterns().to_vec())
        .unwrap_or_default();
    for pattern in patterns.iter().chain(&workspace.packages) {
        let (exclude, pattern) = match pattern.strip_prefix('!') {
            Some(pattern) => (true, pattern),
            None => (false, pattern.as_str()),
        };
        if Path::new(pattern).is_absolute() || pattern.split('/').any(|part| part == "..") {
            continue;
        }
        let pattern = root.join(pattern).join("package.json");
        let pattern = pattern.to_string_lossy();
        for path in glob::glob(&pattern).map_err(|error| error.to_string())? {
            let path = path.map_err(|error| error.to_string())?;
            if path
                .components()
                .any(|part| part.as_os_str() == "node_modules")
                || !path
                    .canonicalize()
                    .map_err(|error| error.to_string())?
                    .starts_with(&canonical_root)
            {
                continue;
            }
            if exclude {
                excluded.insert(path);
            } else {
                paths.insert(path);
            }
        }
    }

    paths.retain(|path| !excluded.contains(path));
    paths.remove(&root.join("package.json"));
    let mut projects = Vec::new();
    let mut names = BTreeSet::new();
    for path in std::iter::once(root.join("package.json")).chain(paths) {
        let manifest = if path == root.join("package.json") {
            root_manifest.clone()
        } else {
            PackageJson::from_path(&path)?
        };
        let name = manifest
            .name
            .clone()
            .unwrap_or_else(|| path.parent().unwrap_or(root).display().to_string());
        if !options.packages.is_empty() && !options.packages.contains(&name) {
            continue;
        }
        let mut dependencies = Vec::new();
        for (section, entries) in [
            ("dependencies", &manifest.dependencies),
            ("devDependencies", &manifest.dev_dependencies),
            ("optionalDependencies", &manifest.optional_dependencies),
            ("peerDependencies", &manifest.peer_dependencies),
        ] {
            for (alias, spec) in entries {
                let (package, range) = if let Some(alias_spec) = spec.strip_prefix("npm:") {
                    match alias_spec.rsplit_once('@') {
                        Some((package, range)) => (package, range),
                        None => continue,
                    }
                } else {
                    (alias.as_str(), spec.as_str())
                };
                let Ok(requirement) = Requirement::from_node(spec) else {
                    log::debug!("skipping non-semver dependency {alias}: {spec}");
                    continue;
                };
                if range == "*"
                    || !requirement
                        .current_version()
                        .is_some_and(|version| requirement.with_version(&version).is_some())
                {
                    continue;
                }
                names.insert(package.to_string());
                dependencies.push(Dependency {
                    alias: alias.to_string(),
                    name: package.to_string(),
                    declared: spec.to_string(),
                    section,
                });
            }
        }
        projects.push(Project {
            manifest: path,
            name,
            dependencies,
        });
    }

    let client = reqwest::Client::builder()
        .user_agent(concat!("check-updates/", env!("CARGO_PKG_VERSION")))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let registry =
        reqwest::Url::parse("https://registry.npmjs.org/").expect("valid public npm registry URL");
    let mut versions = BTreeMap::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let mut tasks = tokio::task::JoinSet::new();
    for name in names {
        let client = client.clone();
        let registry = registry.clone();
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| error.to_string())?;
        tasks.spawn(async move {
            let _permit = permit;
            let mut url = registry;
            url.path_segments_mut()
                .map_err(|_| "invalid registry URL".to_string())?
                .pop_if_empty()
                .push(&name);
            let mut response = client
                .get(url.clone())
                .header("Accept", "application/vnd.npm.install-v1+json")
                .send()
                .await
                .map_err(|error| error.to_string())?;
            for attempt in 0..2 {
                if response.status().as_u16() != 429 && !response.status().is_server_error() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                response = client
                    .get(url.clone())
                    .header("Accept", "application/vnd.npm.install-v1+json")
                    .send()
                    .await
                    .map_err(|error| error.to_string())?;
            }
            if response.status() == reqwest::StatusCode::NOT_FOUND {
                log::debug!("skipping {name}: not found on the public npm registry");
                return Ok::<_, String>((name, Vec::new()));
            }
            let mut response = response
                .error_for_status()
                .map_err(|error| error.to_string())?;
            const MAX_METADATA_BYTES: usize = 32 * 1024 * 1024;
            let mut bytes = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(|error| error.to_string())? {
                if bytes.len().saturating_add(chunk.len()) > MAX_METADATA_BYTES {
                    return Err(format!("registry metadata for {name} exceeds 32 MiB"));
                }
                bytes.extend_from_slice(&chunk);
            }
            let body: Value = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
            let available = body["versions"]
                .as_object()
                .ok_or_else(|| format!("missing versions for {name}"))?
                .iter()
                .filter(|(_, data)| data.get("deprecated").is_none())
                .filter_map(|(version, _)| Version::parse(version).ok())
                .collect::<Vec<_>>();
            Ok::<_, String>((name, available))
        });
    }
    while let Some(result) = tasks.join_next().await {
        let (name, available) = result.map_err(|error| error.to_string())??;
        versions.insert(name, available);
    }

    let project_names = projects
        .iter()
        .map(|project| project.name.clone())
        .collect();
    let mut packages = Packages::new();
    for project in projects {
        for dependency in project.dependencies {
            let Ok(requirement) = Requirement::from_node(&dependency.declared) else {
                continue;
            };
            let Some(current) = requirement.current_version() else {
                continue;
            };
            let Some(available) = versions.get(&dependency.name) else {
                continue;
            };
            let kind = match dependency.section {
                "dependencies" => DepKind::Normal,
                "devDependencies" => DepKind::Dev,
                "optionalDependencies" => DepKind::Optional,
                "peerDependencies" => DepKind::Peer,
                _ => unreachable!("known dependency section"),
            };
            let unit = Unit::Project {
                manifest: project.manifest.clone(),
                name: project.name.clone(),
            };
            let usage = Usage {
                unit: unit.clone(),
                req: requirement.clone(),
                kind,
                rename: None,
                supported_toolchain_version: None,
            };
            let package = Package {
                purl: Purl::new("npm".to_string(), dependency.alias.clone())
                    .map_err(|error| error.to_string())?,
                usages: vec![usage],
                versions: available
                    .iter()
                    .filter(|version| *version > &current)
                    .cloned()
                    .map(|version| PackageVersion {
                        version,
                        yanked: false,
                        features: Default::default(),
                        required_toolchain_version: None,
                    })
                    .collect(),
                repository: None,
                homepage: None,
            };
            packages
                .entry(unit)
                .or_default()
                .push((requirement.clone(), kind, package));
        }
    }
    Ok(NpmPackages {
        projects: project_names,
        packages,
    })
}

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
        let Unit::Project { manifest, .. } = &usage.unit else {
            return Err("expected an npm project manifest".into());
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
        let section_key = serde_json::to_string(section).map_err(|error| error.to_string())?;
        let key = serde_json::to_string(name).map_err(|error| error.to_string())?;
        let old_value = serde_json::to_string(old).map_err(|error| error.to_string())?;
        let new_value = serde_json::to_string(new).map_err(|error| error.to_string())?;
        let start = text
            .find(&section_key)
            .ok_or_else(|| format!("missing {section} in {}", path.display()))?;
        let section_text = &text[start + section_key.len()..];
        let open = section_text
            .find('{')
            .ok_or_else(|| format!("invalid {section} in {}", path.display()))?;
        let close = section_text[open..]
            .find('}')
            .ok_or_else(|| format!("invalid {section} in {}", path.display()))?
            + open;
        let body = &section_text[open..close];
        let value_pos = body
            .match_indices(&key)
            .find_map(|(key_pos, _)| {
                let tail = &body[key_pos + key.len()..];
                let spaces = tail.len() - tail.trim_start().len();
                let tail = tail.get(spaces..)?.strip_prefix(':')?;
                let value_spaces = tail.len() - tail.trim_start().len();
                tail.get(value_spaces..)?
                    .starts_with(&old_value)
                    .then_some(key_pos + key.len() + spaces + 1 + value_spaces)
            })
            .ok_or_else(|| format!("missing {name} in {}", path.display()))?;
        let offset = start + section_key.len() + open + value_pos;
        text.replace_range(offset..offset + old_value.len(), &new_value);
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
    fn edits_only_requested_section() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        let input = "{\n  \"dependencies\": { \"foo\": \"^1.0.0\" },\n  \"devDependencies\": { \"foo\": \"^1.0.0\" }\n}\n";
        std::fs::write(&path, input).unwrap();
        edit_manifest(
            &path,
            &[(
                "devDependencies",
                "foo".into(),
                "^1.0.0".into(),
                "^2.0.0".into(),
            )],
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            input.replacen(
                "\"devDependencies\": { \"foo\": \"^1.0.0\"",
                "\"devDependencies\": { \"foo\": \"^2.0.0\"",
                1
            )
        );
    }

    #[test]
    fn applies_selected_package_requirement() {
        let dir = tempfile::tempdir().unwrap();
        let manifest = dir.path().join("package.json");
        std::fs::write(
            &manifest,
            r#"{"dependencies":{"foo":"^1.0.0"},"devDependencies":{"foo":"^1.0.0"}}"#,
        )
        .unwrap();
        let req = Requirement::from_node("^1.0.0").unwrap();
        let unit = Unit::Project {
            manifest: manifest.clone(),
            name: "app".into(),
        };
        let usage = Usage {
            unit,
            req,
            kind: DepKind::Dev,
            rename: None,
            supported_toolchain_version: None,
        };
        let package = Package {
            purl: Purl::new("npm".to_string(), "foo").unwrap(),
            usages: vec![usage.clone()],
            versions: Vec::new(),
            repository: None,
            homepage: None,
        };
        update_packages([(&usage, &package, Requirement::from_node("^2.0.0").unwrap())]).unwrap();
        let contents = std::fs::read_to_string(manifest).unwrap();
        assert!(contents.contains(r#""dependencies":{"foo":"^1.0.0"}"#));
        assert!(contents.contains(r#""devDependencies":{"foo":"^2.0.0"}"#));
    }

    #[test]
    fn refuses_to_edit_a_nested_section_instead() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("package.json");
        let input =
            r#"{"config":{"dependencies":{"foo":"^1.0.0"}},"dependencies":{"foo":"^1.0.0"}}"#;
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
            .is_err()
        );
        assert_eq!(std::fs::read_to_string(path).unwrap(), input);
    }

    #[tokio::test]
    async fn discovers_bun_workspace_from_package_json() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let app = root.join("apps/app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","packageManager":"bun@1.2.0","workspaces":{"packages":["apps/*"]}}"#,
        )
        .unwrap();
        std::fs::write(app.join("package.json"), r#"{"name":"app"}"#).unwrap();
        std::fs::write(root.join("bun.lock"), "").unwrap();
        assert_eq!(package_manager(root).unwrap(), PackageManager::Bun);

        let found = packages(root, &NpmOptions::default()).await.unwrap();
        assert_eq!(found.projects, ["root", "app"]);
    }

    #[test]
    fn detects_lockfile_before_package_manager_field() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::write(root.join("package.json"), r#"{"packageManager":"npm@10"}"#).unwrap();
        assert_eq!(package_manager(root).unwrap(), PackageManager::Npm);
        std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
        assert_eq!(package_manager(root).unwrap(), PackageManager::Pnpm);
        std::fs::write(root.join("bun.lock"), "").unwrap();
        assert!(package_manager(root).is_err());
        std::fs::remove_file(root.join("bun.lock")).unwrap();
        std::fs::remove_file(root.join("pnpm-lock.yaml")).unwrap();
        std::fs::write(root.join("yarn.lock"), "").unwrap();
        assert!(package_manager(root).is_err());
    }

    #[tokio::test]
    async fn discovers_pnpm_workspace_without_package_json_workspaces() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let app = root.join("packages/app");
        std::fs::create_dir_all(&app).unwrap();
        std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
        std::fs::write(
            root.join("pnpm-workspace.yaml"),
            "packages:\n  - 'packages/*'\n",
        )
        .unwrap();
        std::fs::write(app.join("package.json"), r#"{"name":"app"}"#).unwrap();
        let found = packages(root, &NpmOptions::default()).await.unwrap();
        assert_eq!(found.projects, ["root", "app"]);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn excludes_workspace_patterns_and_external_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let external = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("apps/app")).unwrap();
        std::fs::create_dir_all(root.join("apps/skip")).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","workspaces":["apps/*","!apps/skip"]}"#,
        )
        .unwrap();
        for (folder, name) in [("apps/app", "app"), ("apps/skip", "skip")] {
            std::fs::write(
                root.join(folder).join("package.json"),
                format!(r#"{{"name":"{name}"}}"#),
            )
            .unwrap();
        }
        std::fs::write(
            external.path().join("package.json"),
            r#"{"name":"external"}"#,
        )
        .unwrap();
        std::os::unix::fs::symlink(external.path(), root.join("apps/external")).unwrap();

        let found = packages(root, &NpmOptions::default()).await.unwrap();
        assert_eq!(found.projects, ["root", "app"]);
    }
}
