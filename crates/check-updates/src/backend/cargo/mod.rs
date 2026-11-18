use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use thiserror::Error;

use crate::{
    Requirement, State,
    backend::RegistryError,
    package::{DepKind, Package, Unit, Usage},
};

mod cli;
mod config;
mod fetch;
mod metadata;
mod resolve;
mod update;

use resolve::*;
use update::*;

#[derive(Debug, Error)]
pub enum CargoError {
    #[error("invalid publish-age configuration: {0}")]
    Configuration(String),
    #[error("failed to get cargo metadata: {0}")]
    Metadata(String),
    #[error("failed to access crates.io index: {0}")]
    Index(#[from] tame_index::Error),
    #[error("failed to read registry entry for '{name}': {source}")]
    RegistryEntry {
        name: String,
        #[source]
        source: tame_index::Error,
    },
    #[error("HTTP request failed: {0}")]
    Http(#[from] http::Error),
    #[error("request error: {0}")]
    Reqwest(#[from] reqwest::Error),
    #[error("failed to read registry response: {0}")]
    Body(String),
    #[error("fetch task failed: {0}")]
    FetchTask(#[from] tokio::task::JoinError),
    #[error("manifest is not writable: {0}")]
    ReadOnly(PathBuf),
}

/// Cargo dependency management.
pub(crate) struct CargoBackend {
    state: Rc<State>,
}

impl CargoBackend {
    /// Create a Cargo backend.
    pub(crate) fn new(state: Rc<State>) -> Self {
        Self { state }
    }

    /// Load the workspace root and dependency packages.
    pub(crate) async fn packages(&self) -> Result<(PathBuf, Vec<Package>), RegistryError> {
        let total_start = Instant::now();

        let metadata_start = Instant::now();
        let metadata = metadata::workspace(self.state.root().map(|root| root.as_path()))?;
        log::debug!(
            "cargo.packages: workspace metadata took {:?}",
            metadata_start.elapsed()
        );
        let root = metadata.workspace_root.as_std_path().to_path_buf();
        let publish_age = config::load(
            &root,
            self.state.options.min_publish_age,
            self.state.options.ignore_publish_age,
            &crate::config::ConfigEnv::current(),
        )
        .map_err(CargoError::Configuration)?;

        let registry_start = Instant::now();
        let members = workspace_members(&metadata);
        let names = collect_crates_io_deps(&members);

        let registry = fetch::Registry::try_new(self.state.client().clone())?;
        let versions = registry.versions(&names, publish_age).await?;
        log::debug!(
            "cargo.packages: checked {} registry entries in {:?}",
            names.len(),
            registry_start.elapsed()
        );

        let workspace_root_manifest: PathBuf = metadata.workspace_root.join("Cargo.toml").into();

        let crate_meta = tame_index::utils::cargo_home()
            .map(|home| {
                metadata::cached_crate_meta(home.join("registry/src").as_std_path(), &names)
            })
            .unwrap_or_default();

        let inherited_deps: HashMap<PathBuf, InheritedDeps> = members
            .iter()
            .map(|member| {
                let path: PathBuf = member.manifest_path.clone().into();
                let inherited = workspace_inherited_deps(&path);
                (path, inherited)
            })
            .collect();

        let packages = build_packages(
            &members,
            versions,
            &workspace_root_manifest,
            &crate_meta,
            &inherited_deps,
        );
        log::debug!(
            "cargo.packages: total resolution took {:?}",
            total_start.elapsed()
        );

        Ok((root, packages))
    }

    /// Apply version requirements to manifests.
    pub(crate) fn update_versions<'a>(
        &self,
        edits: impl IntoIterator<Item = (&'a Usage, &'a Package, Requirement)>,
    ) -> Result<(), RegistryError> {
        // Group all edits by manifest path so we only read/write each file once.
        let mut manifest_edits: HashMap<PathBuf, Vec<ManifestEdit>> = HashMap::new();

        for (usage, package, new_req) in edits {
            let new_req = new_req
                .into_cargo()
                .ok_or(RegistryError::WrongRequirement)?;
            // Fan out to every usage of the same section that shares the same
            // unit + req. Other dep-kind sections are separate updates.
            let matching_usages = package
                .usages
                .iter()
                .filter(|u| u.unit == usage.unit && u.req == usage.req && u.kind == usage.kind);

            for u in matching_usages {
                let manifest = match u.unit.path() {
                    Some(p) => p.to_path_buf(),
                    None => {
                        log::warn!(
                            "skipping update for '{}': unit has no manifest path",
                            package.purl.name()
                        );
                        continue;
                    }
                };

                // The TOML key is the rename (alias) if present, otherwise the
                // real crate name.
                let toml_key = u
                    .rename
                    .as_deref()
                    .unwrap_or_else(|| package.purl.name())
                    .to_string();

                let section = match &u.unit {
                    Unit::Workspace { .. } => {
                        vec!["workspace".to_string(), "dependencies".to_string()]
                    }
                    _ => {
                        let section = match u.kind {
                            DepKind::Normal => "dependencies",
                            DepKind::Dev => "dev-dependencies",
                            DepKind::Build => "build-dependencies",
                            DepKind::Optional | DepKind::Peer => continue,
                        };
                        vec![section.to_string()]
                    }
                };

                manifest_edits
                    .entry(manifest)
                    .or_default()
                    .push(ManifestEdit {
                        section,
                        toml_key,
                        new_req: new_req.clone(),
                        old_req: u
                            .req
                            .clone()
                            .into_cargo()
                            .ok_or(RegistryError::WrongRequirement)?,
                        preserve_bare: true,
                    });
            }
        }

        // Apply all edits, one manifest at a time.
        for (manifest, mut file_edits) in manifest_edits {
            let mut seen = HashSet::new();
            file_edits.retain(|edit| {
                seen.insert((
                    edit.section.clone(),
                    edit.toml_key.clone(),
                    edit.old_req.to_string(),
                    edit.new_req.to_string(),
                ))
            });

            apply_manifest_edits(&manifest, &file_edits)?;
        }

        Ok(())
    }
}
