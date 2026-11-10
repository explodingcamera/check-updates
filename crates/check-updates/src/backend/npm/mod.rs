use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::Value;

use crate::{DepKind, Package, PackageVersion, Packages, Purl, Requirement, Unit, Usage, Version};

mod cli;
mod manifest;
mod update;

use manifest::{PackageJson, PnpmWorkspace};
pub use update::update_packages;

/// JavaScript dependency management.
pub(crate) struct NpmBackend;

/// Package manager selected from a lockfile or `packageManager` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Bun,
}

/// Detect the tool that owns this project's lockfile, if one exists.
pub fn package_manager(root: &Path) -> Result<PackageManager, String> {
    if root.join("yarn.lock").is_file() {
        return Err("unsupported lockfile 'yarn.lock'".into());
    }
    let lockfiles = [
        ("package-lock.json", PackageManager::Npm),
        ("npm-shrinkwrap.json", PackageManager::Npm),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
    ];
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
    requirement: Requirement,
    current: Version,
    kind: DepKind,
}

struct Project {
    manifest: PathBuf,
    name: String,
    dependencies: Vec<Dependency>,
}

/// Discover direct npm dependencies and their published versions without modifying project files.
pub async fn packages(root: &Path, options: &NpmOptions<'_>) -> Result<NpmPackages, String> {
    let root_path = root.join("package.json");
    let root_manifest = PackageJson::from_path(&root_path)?;
    let workspace = PnpmWorkspace::from_root(root)?;
    let has_workspace = root_manifest.workspaces.is_some() || !workspace.packages.is_empty();
    let mut paths = BTreeSet::new();
    let mut excluded = BTreeSet::new();
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    let root_pattern = glob::Pattern::escape(&root.to_string_lossy());
    let patterns = root_manifest
        .workspaces
        .as_ref()
        .map(|workspaces| workspaces.patterns())
        .unwrap_or_default();
    for pattern in patterns.iter().chain(&workspace.packages) {
        let (exclude, pattern) = match pattern.strip_prefix('!') {
            Some(pattern) => (true, pattern),
            None => (false, pattern.as_str()),
        };
        if Path::new(pattern).is_absolute() || pattern.split('/').any(|part| part == "..") {
            continue;
        }
        let pattern = Path::new(&root_pattern).join(pattern).join("package.json");
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
    paths.remove(&root_path);
    let mut projects = Vec::new();
    let mut names = BTreeSet::new();
    for path in std::iter::once(root_path.clone()).chain(paths) {
        let manifest = if path == root_path {
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
        for (kind, entries) in [
            (DepKind::Normal, &manifest.dependencies),
            (DepKind::Dev, &manifest.dev_dependencies),
            (DepKind::Optional, &manifest.optional_dependencies),
            (DepKind::Peer, &manifest.peer_dependencies),
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
                let Some(current) = requirement.current_version() else {
                    continue;
                };
                if range == "*" || requirement.with_version(&current).is_none() {
                    continue;
                }
                names.insert(package.to_string());
                dependencies.push(Dependency {
                    alias: alias.to_string(),
                    name: package.to_string(),
                    requirement,
                    current,
                    kind,
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
        let mut url = registry.clone();
        let permit = semaphore
            .clone()
            .acquire_owned()
            .await
            .map_err(|error| error.to_string())?;
        tasks.spawn(async move {
            let _permit = permit;
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

    let mut project_names = Vec::with_capacity(projects.len());
    let mut packages = Packages::new();
    for project in projects {
        project_names.push(project.name.clone());
        let unit = if has_workspace && project.manifest == root_path {
            Unit::Workspace {
                manifest: project.manifest,
            }
        } else {
            Unit::Project {
                manifest: project.manifest,
                name: project.name,
            }
        };
        for Dependency {
            alias,
            name,
            requirement,
            current,
            kind,
        } in project.dependencies
        {
            let Some(available) = versions.get(&name) else {
                continue;
            };
            let usage = Usage {
                unit: unit.clone(),
                req: requirement.clone(),
                kind,
                rename: None,
                supported_toolchain_version: None,
            };
            let package = Package {
                purl: Purl::new("npm".to_string(), alias).map_err(|error| error.to_string())?,
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
                .entry(unit.clone())
                .or_default()
                .push((requirement, kind, package));
        }
    }
    Ok(NpmPackages {
        projects: project_names,
        packages,
    })
}
