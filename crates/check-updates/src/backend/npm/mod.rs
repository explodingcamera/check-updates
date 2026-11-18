use jiff::{Span, Timestamp};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;

use crate::{Package, Packages, Purl, Usage};

mod cli;
mod fetch;
mod manifest;
mod npmrc;
mod pm;
mod project;
mod update;

pub use pm::PackageManager;
use project::Dependency;
pub use update::update_packages;

/// JavaScript dependency management.
#[derive(Clone)]
pub(crate) struct NpmBackend {
    client: reqwest::Client,
    registry: reqwest::Url,
    limiter: Arc<bottlecap::Limiter>,
}

/// Options for discovering npm packages.
#[derive(Debug, Default)]
pub struct NpmOptions<'a> {
    pub packages: &'a [String],
    /// Override the publication age inherited from package-manager configuration, resolved in UTC.
    pub min_publish_age: Option<Span>,
    /// Ignore publication-age settings, including an explicit minimum.
    pub ignore_publish_age: bool,
}

/// Discovered npm projects and their package versions.
#[derive(Debug)]
pub struct NpmPackages {
    pub projects: Vec<String>,
    pub packages: Packages,
}

/// Discover direct npm dependencies and their published versions without modifying project files.
pub async fn packages(root: &Path, options: &NpmOptions<'_>) -> Result<NpmPackages, String> {
    let project_root = project::root(root)?.ok_or_else(|| {
        format!(
            "No package.json found in {} or its ancestors",
            root.display()
        )
    })?;
    NpmBackend::try_new()?
        .packages(&project_root, options)
        .await
}

impl NpmBackend {
    /// Create a backend with a reusable registry client and rate limiter.
    pub(crate) fn try_new() -> Result<Self, String> {
        Ok(Self {
            client: crate::backend::http::client_builder()
                .build()
                .map_err(|error| error.to_string())?,
            registry: reqwest::Url::parse("https://registry.npmjs.org/")
                .expect("valid public npm registry URL"),
            limiter: Arc::new(bottlecap::Limiter::new(bottlecap::Quota::per_second(10))),
        })
    }

    /// Load workspace projects and their published dependency versions.
    async fn packages(&self, root: &Path, options: &NpmOptions<'_>) -> Result<NpmPackages, String> {
        // Manifest-only checks also work for unsupported or ambiguous package managers.
        let publish_age = match PackageManager::detect(root) {
            Ok(manager) => crate::publish_age::PublishAge::try_from_npm_config(
                root,
                manager,
                options.min_publish_age,
                options.ignore_publish_age,
                &crate::config::ConfigEnv::current(),
            )?,
            Err(error) => {
                if !options.ignore_publish_age
                    && !options.min_publish_age.is_some_and(|age| age.is_zero())
                {
                    log::warn!("cannot load inherited publication-age settings: {error}");
                }
                crate::publish_age::PublishAge::try_new(
                    if options.ignore_publish_age {
                        None
                    } else {
                        options.min_publish_age
                    },
                    Timestamp::now(),
                )?
            }
        };
        let projects = project::discover(root, options.packages)?;
        let names: BTreeSet<_> = projects
            .iter()
            .flat_map(|project| &project.dependencies)
            .map(|dependency| dependency.name.clone())
            .collect();
        let versions = self.versions(names, publish_age).await?;

        let mut project_names = Vec::with_capacity(projects.len());
        let mut packages = Packages::new();
        for project in projects {
            project_names.push(project.name);
            let unit = project.unit;
            for Dependency {
                alias,
                name,
                requirement,
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
                    versions: Arc::clone(available),
                    repository: None,
                    homepage: None,
                };
                packages.entry(unit.clone()).or_default().push((
                    requirement,
                    kind,
                    Arc::new(package),
                ));
            }
        }
        Ok(NpmPackages {
            projects: project_names,
            packages,
        })
    }
}
