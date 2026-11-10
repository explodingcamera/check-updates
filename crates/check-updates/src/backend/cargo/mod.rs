use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Instant;

use cargo_metadata::{CargoOpt, MetadataCommand};
use tame_index::index::{FileLock, IndexLocation, IndexUrl, SparseIndex};
use thiserror::Error;

use crate::{
    RegistryCachePolicy, Requirement, State,
    backend::RegistryError,
    package::{DepKind, Package, PackageVersion, Unit, Usage},
};

mod cli;
mod fetch;
mod resolve;
mod update;

use resolve::*;
use update::*;

#[derive(Debug, Error)]
pub enum CargoError {
    #[error("failed to get cargo metadata: {0}")]
    Metadata(String),
    #[error("failed to access crates.io index: {0}")]
    Index(#[from] tame_index::Error),
    #[error("HTTP request failed: {0}")]
    Http(#[from] http::Error),
    #[error("request error: {0}")]
    Reqwest(#[from] reqwest::Error),
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

    /// Load dependency packages.
    pub(crate) async fn packages(&self) -> Result<Vec<Package>, RegistryError> {
        let total_start = Instant::now();

        let index = SparseIndex::new(IndexLocation::new(IndexUrl::CratesIoSparse))
            .map_err(CargoError::from)?;
        let lock = FileLock::unlocked();

        let metadata_cmd = || {
            let mut cmd = MetadataCommand::new();
            if let Some(root) = self.state.root() {
                cmd.current_dir(root);
            }
            cmd
        };

        let metadata_start = Instant::now();
        let metadata = metadata_cmd()
            .features(CargoOpt::AllFeatures)
            .exec()
            .or_else(|_| {
                metadata_cmd()
                    .features(CargoOpt::SomeFeatures(vec![]))
                    .exec()
            })
            .or_else(|_| metadata_cmd().exec())
            .map_err(|e| CargoError::Metadata(e.to_string()))?;
        log::debug!(
            "cargo.packages: cargo metadata resolution took {:?}",
            metadata_start.elapsed()
        );

        let registry_start = Instant::now();
        let members = workspace_members(&metadata);
        let names = collect_crates_io_deps(&members);

        let mut versions: HashMap<String, Vec<PackageVersion>> = HashMap::new();
        let mut requests: Vec<(String, http::Request<()>)> = Vec::new();
        let mut cache_hits = 0usize;
        let policy = self.state.registry_cache_policy();
        let use_local_cache = matches!(policy, RegistryCachePolicy::PreferLocal);
        let write_cache_entry = !matches!(policy, RegistryCachePolicy::NoCache);

        // A 304 response makes tame-index read the local cache entry instead, so
        // requests without revalidation drop the conditional headers.
        let revalidate = write_cache_entry;
        let make_request =
            |name: &str, revalidate: bool| -> Result<http::Request<()>, CargoError> {
                let mut request: http::Request<()> = index
                    .make_remote_request(name.try_into()?, None, &lock)?
                    .into();
                if !revalidate {
                    let headers = request.headers_mut();
                    headers.remove(http::header::IF_NONE_MATCH);
                    headers.remove(http::header::IF_MODIFIED_SINCE);
                }
                Ok(request)
            };

        for name in &names {
            if use_local_cache {
                let crate_name = name.as_str().try_into().map_err(CargoError::from)?;
                match index
                    .cached_krate(crate_name, &lock)
                    .and_then(|entry| entry.map(|entry| entry.deserialize()).transpose())
                {
                    Ok(Some(krate)) => {
                        versions.insert(name.clone(), versions_from_krate(&krate));
                        cache_hits += 1;
                        continue;
                    }
                    Ok(None) => {}
                    Err(err) => {
                        log::debug!("cargo.packages: cache miss for '{name}': {err}");
                    }
                }
            }

            match make_request(name, revalidate) {
                Ok(req) => requests.push((name.clone(), req)),
                Err(e) => {
                    log::warn!("failed to build cache request for '{name}': {e}");
                }
            }
        }

        log::debug!(
            "cargo.packages: cache policy {:?}, cache hits {}, network requests {}, registry resolve took {:?}",
            policy,
            cache_hits,
            requests.len(),
            registry_start.elapsed()
        );

        let mut revalidated = revalidate;
        while !requests.is_empty() {
            let responses =
                fetch::fetch_all(self.state.client(), std::mem::take(&mut requests)).await?;

            for (name, response) in responses {
                let crate_name = match name.as_str().try_into() {
                    Ok(name) => name,
                    Err(e) => {
                        log::warn!("failed to parse crate name '{name}': {e}");
                        continue;
                    }
                };

                match index
                    .parse_remote_response(crate_name, &response, write_cache_entry, &lock)
                    .and_then(|entry| entry.map(|entry| entry.deserialize()).transpose())
                {
                    Ok(Some(krate)) => {
                        versions.insert(name, versions_from_krate(&krate));
                    }
                    Ok(None) => {
                        log::debug!("no crate data returned for '{name}'");
                    }
                    // The cache entry can be unreadable, e.g. when cargo wrote it
                    // in a form tame-index rejects. Fetch the full entry instead.
                    Err(e)
                        if revalidated && response.status() == http::StatusCode::NOT_MODIFIED =>
                    {
                        log::debug!("refetching '{name}' after unreadable cache entry: {e}");
                        match make_request(&name, false) {
                            Ok(req) => requests.push((name, req)),
                            Err(e) => log::warn!("failed to build request for '{name}': {e}"),
                        }
                    }
                    Err(e) => {
                        log::warn!("failed to parse versions for '{name}': {e}");
                    }
                }
            }
            revalidated = false;
        }

        let workspace_root_manifest: PathBuf = metadata.workspace_root.join("Cargo.toml").into();

        let crate_meta = crate_meta_from_packages(&metadata.packages);

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
            &versions,
            &workspace_root_manifest,
            &crate_meta,
            &inherited_deps,
        );
        log::debug!(
            "cargo.packages: total resolution took {:?}",
            total_start.elapsed()
        );

        Ok(packages)
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
