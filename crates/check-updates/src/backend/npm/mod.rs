use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;

use crate::{DepKind, Package, PackageVersion, Packages, Purl, Requirement, Unit, Usage, Version};

mod cli;
mod manifest;
mod npmrc;
mod pm;
mod project;
mod update;

use manifest::PackageJson;
use pm::pnpm::PnpmWorkspace;
pub use pm::{PackageManager, package_manager};
pub use update::update_packages;

/// JavaScript dependency management.
pub(crate) struct NpmBackend;

/// Options for discovering npm packages.
#[derive(Debug, Default)]
pub struct NpmOptions<'a> {
    pub packages: &'a [String],
    /// Override the minimum publication age inherited from package-manager configuration.
    pub min_publish_age: Option<Duration>,
    /// Ignore publication-age settings, including an explicit minimum.
    pub ignore_publish_age: bool,
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

#[derive(Deserialize)]
struct RegistryMetadata {
    versions: BTreeMap<String, RegistryVersion>,
    time: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
struct RegistryVersion {
    deprecated: Option<Value>,
}

/// Discover direct npm dependencies and their published versions without modifying project files.
pub async fn packages(root: &Path, options: &NpmOptions<'_>) -> Result<NpmPackages, String> {
    let project_root = project::root(root)?.ok_or_else(|| {
        format!(
            "No package.json found in {} or its ancestors",
            root.display()
        )
    })?;
    let root = project_root.as_path();
    // Manifest-only checks also work for unsupported or ambiguous package managers.
    let publish_age = match package_manager(root) {
        Ok(manager) => pm::publish_age(
            root,
            manager,
            options.min_publish_age,
            options.ignore_publish_age,
            &crate::config::ConfigEnv::current(),
        ),
        Err(error) => {
            if !options.ignore_publish_age && options.min_publish_age != Some(Duration::ZERO) {
                log::warn!("cannot load inherited publication-age settings: {error}");
            }
            crate::publish_age::PublishAge::new(if options.ignore_publish_age {
                None
            } else {
                options.min_publish_age
            })
        }
    };
    let root_path = root.join("package.json");
    let root_manifest = PackageJson::from_path(&root_path)?;
    let workspace = PnpmWorkspace::from_root(root)?;
    let has_workspace = root_manifest.workspaces.is_some() || !workspace.packages.is_empty();
    let mut paths = BTreeSet::new();
    let mut excluded = BTreeSet::new();
    let canonical_root = root.canonicalize().map_err(|error| error.to_string())?;
    // Escape directory names, not Windows drive/UNC prefixes such as `\\?\C:\`.
    let root_pattern: PathBuf = root
        .components()
        .map(|component| match component {
            std::path::Component::Normal(name) => {
                glob::Pattern::escape(&name.to_string_lossy()).into()
            }
            _ => component.as_os_str().to_owned(),
        })
        .collect();
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
        let pattern = root_pattern.join(pattern).join("package.json");
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
        .user_agent(crate::USER_AGENT)
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|error| error.to_string())?;
    let registry =
        reqwest::Url::parse("https://registry.npmjs.org/").expect("valid public npm registry URL");
    let mut versions = BTreeMap::new();
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let limiter = std::sync::Arc::new(bottlecap::Limiter::new(
        bottlecap::Quota::per_second(10),
    ));
    let mut tasks = tokio::task::JoinSet::new();
    for name in names {
        let client = client.clone();
        let limiter = limiter.clone();
        let publish_age = publish_age.clone();
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
            limiter
                .wait(tokio::time::sleep)
                .await
                .map_err(|error| error.to_string())?;
            let mut response = client
                .get(url.clone())
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|error| error.to_string())?;
            for attempt in 0..2 {
                if response.status().as_u16() != 429 && !response.status().is_server_error() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(500 * (1 << attempt))).await;
                limiter
                    .wait(tokio::time::sleep)
                    .await
                    .map_err(|error| error.to_string())?;
                response = client
                    .get(url.clone())
                    .header("Accept", "application/json")
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
            // Full metadata can contain tens of MiB of unused readmes and manifests.
            // Deserialize only the fields needed to select versions.
            let body: RegistryMetadata = serde_json::from_slice(&bytes)
                .map_err(|error| format!("invalid registry metadata for {name}: {error}"))?;
            let available = available_versions(&name, &body, &publish_age);
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
                    .filter(|entry| entry.version > current)
                    .cloned()
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

fn available_versions(
    name: &str,
    body: &RegistryMetadata,
    age: &crate::publish_age::PublishAge,
) -> Vec<PackageVersion> {
    body.versions
        .iter()
        .filter(|(_, data)| data.deprecated.is_none())
        .filter_map(|(version, _)| {
            let published = body
                .time
                .as_ref()
                .and_then(|time| time.get(version))
                .and_then(Value::as_str);
            if !age.allows(name, version, published) {
                return None;
            }
            Some(PackageVersion {
                version: Version::parse(version).ok()?,
                published_at: published.and_then(|value| value.parse().ok()),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::publish_age::PublishAge;

    #[test]
    fn skips_recent_versions_and_keeps_older_updates() {
        let body = serde_json::from_value(serde_json::json!({
            "versions": {"1.0.0": {}, "1.1.0": {}, "1.2.0": {}, "1.3.0": {"deprecated": "bad"}},
            "time": {"1.0.0": "2000-01-01T00:00:00Z", "1.1.0": "2001-01-01T00:00:00Z", "1.2.0": "2099-01-01T00:00:00Z", "1.3.0": "2001-01-01T00:00:00Z"}
        })).unwrap();
        let mut age = PublishAge::new(Some(Duration::from_secs(86400)));
        let versions = available_versions("real-package", &body, &age);
        assert_eq!(
            versions
                .iter()
                .map(|entry| &entry.version)
                .max()
                .unwrap()
                .to_string(),
            "1.1.0"
        );
        assert!(versions.iter().all(|entry| entry.published_at.is_some()));
        age.excludes = vec!["real-package".into()];
        assert_eq!(
            available_versions("real-package", &body, &age)
                .iter()
                .map(|entry| &entry.version)
                .max()
                .unwrap()
                .to_string(),
            "1.2.0"
        );
        assert_eq!(
            available_versions("real-package", &body, &PublishAge::default()).len(),
            3
        );
    }
}
