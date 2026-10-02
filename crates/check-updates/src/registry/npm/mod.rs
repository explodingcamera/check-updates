use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::{Requirement, Version, VersionStrategy};

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

/// An npm dependency update in a project manifest.
#[derive(Debug, Clone)]
pub struct NpmUpdate {
    pub manifest: PathBuf,
    pub project: String,
    pub name: String,
    pub section: &'static str,
    pub current: String,
    pub proposed: String,
}

/// Options for querying npm updates.
#[derive(Debug, Default)]
pub struct NpmOptions<'a> {
    pub packages: &'a [String],
    pub strategy: VersionStrategy,
    #[cfg(test)]
    registry_url: Option<String>,
}

/// Discovered npm projects and their available updates.
#[derive(Debug)]
pub struct NpmPackages {
    pub projects: Vec<String>,
    pub updates: Vec<NpmUpdate>,
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

/// Resolve direct npm dependency updates without modifying project files.
pub async fn updates(root: &Path, options: &NpmOptions<'_>) -> Result<NpmPackages, String> {
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
    #[cfg(test)]
    let registry = match options.registry_url.as_deref() {
        Some(url) => reqwest::Url::parse(url).map_err(|error| error.to_string())?,
        None => registry,
    };
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
    let mut updates = Vec::new();
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
            let latest = available
                .iter()
                .filter(|version| options.strategy.allows_prerelease(version, Some(&current)))
                .filter(|version| !options.strategy.compatible || requirement.matches(version))
                .max();
            if let Some(latest) = latest
                && latest > &current
                && let Some(new_requirement) = requirement.with_version(latest)
            {
                let new_spec = new_requirement.to_string();
                if new_spec == dependency.declared {
                    continue;
                }
                updates.push(NpmUpdate {
                    manifest: project.manifest.clone(),
                    project: project.name.clone(),
                    name: dependency.alias,
                    section: dependency.section,
                    current: dependency.declared,
                    proposed: new_spec,
                });
            }
        }
    }
    Ok(NpmPackages {
        projects: project_names,
        updates,
    })
}

/// Apply selected npm manifest updates, preserving the file's formatting.
pub fn update_versions(updates: &[NpmUpdate]) -> Result<(), String> {
    let mut by_manifest: BTreeMap<&Path, Vec<(&str, String, String, String)>> = BTreeMap::new();
    for update in updates {
        by_manifest.entry(&update.manifest).or_default().push((
            update.section,
            update.name.clone(),
            update.current.clone(),
            update.proposed.clone(),
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

    use std::io::{Read, Write};
    use std::net::TcpListener;

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
    async fn discovers_workspace_and_deduplicates_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let workspace = root.join("packages/app");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(
            root.join("package.json"),
            r#"{"name":"root","workspaces":["packages/*"],"dependencies":{"foo":"^1.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(
            workspace.join("package.json"),
            r#"{"name":"app","devDependencies":{"foo":"~1.0.0","local":"workspace:*"}}"#,
        )
        .unwrap();
        let registry = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buf = [0; 4096];
            let read = stream.read(&mut buf).unwrap();
            assert!(String::from_utf8_lossy(&buf[..read]).contains("GET /foo "));
            let body = r#"{"versions":{"1.0.0":{},"2.0.0":{},"3.0.0":{"deprecated":"bad"}}}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let found = updates(
            root,
            &NpmOptions {
                registry_url: Some(registry),
                ..NpmOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(found.projects, ["root", "app"]);
        assert_eq!(found.updates.len(), 2);
        assert_eq!(found.updates[0].proposed, "^2.0.0");
        assert_eq!(found.updates[1].proposed, "~2.0.0");
        server.join().unwrap();
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
        std::fs::write(
            app.join("package.json"),
            r#"{"name":"app","dependencies":{"foo":"^1.0.0"}}"#,
        )
        .unwrap();
        std::fs::write(root.join("bun.lock"), "").unwrap();
        assert_eq!(package_manager(root).unwrap(), PackageManager::Bun);

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let registry = format!("http://{}/", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut request = [0; 4096];
            let read = stream.read(&mut request).unwrap();
            assert!(String::from_utf8_lossy(&request[..read]).contains("GET /foo "));
            let body = r#"{"versions":{"1.0.0":{},"2.0.0":{}}}"#;
            write!(stream, "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{}", body.len(), body).unwrap();
        });
        let found = updates(
            root,
            &NpmOptions {
                registry_url: Some(registry),
                ..NpmOptions::default()
            },
        )
        .await
        .unwrap();
        assert_eq!(found.projects, ["root", "app"]);
        assert_eq!(found.updates.len(), 1);
        update_versions(&found.updates).unwrap();
        assert!(
            std::fs::read_to_string(app.join("package.json"))
                .unwrap()
                .contains("\"foo\":\"^2.0.0\"")
        );
        server.join().unwrap();
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
        let found = updates(root, &NpmOptions::default()).await.unwrap();
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

        let found = updates(root, &NpmOptions::default()).await.unwrap();
        assert_eq!(found.projects, ["root", "app"]);
    }
}
