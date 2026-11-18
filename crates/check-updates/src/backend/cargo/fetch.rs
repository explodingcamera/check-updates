use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use http::{Request, Response, StatusCode};
use tame_index::IndexKrate;
use tame_index::index::{FileLock, IndexLocation, IndexUrl, SparseIndex};
use tokio::task::JoinSet;

use super::{CargoError, resolve::versions_from_krate};
use crate::PackageVersion;
use crate::backend::http::read_body;
use crate::publish_age::PublishAge;

const MAX_CONCURRENT_FETCHES: usize = 16;

/// The sparse registry and its reusable HTTP client.
#[derive(Clone)]
pub(super) struct Registry {
    index: Arc<SparseIndex>,
    client: reqwest::Client,
}

impl Registry {
    /// Open Cargo's local crates.io index.
    pub(super) fn try_new(client: reqwest::Client) -> Result<Self, CargoError> {
        Ok(Self {
            index: Arc::new(SparseIndex::new(IndexLocation::new(
                IndexUrl::CratesIoSparse,
            ))?),
            client,
        })
    }

    /// Fetch and parse entries as they arrive instead of retaining all response bodies.
    pub(super) async fn versions(
        &self,
        names: &HashSet<String>,
        age: PublishAge,
    ) -> Result<HashMap<String, Arc<[PackageVersion]>>, CargoError> {
        let mut tasks = JoinSet::new();
        let age = Arc::new(age);
        let mut versions = HashMap::with_capacity(names.len());
        for name in names {
            if tasks.len() == MAX_CONCURRENT_FETCHES
                && let Some(result) = tasks.join_next().await
            {
                let (name, available) = result??;
                versions.insert(name, available);
            }
            let registry = self.clone();
            let name = name.clone();
            let age = age.clone();
            tasks.spawn(async move {
                let versions: Arc<[PackageVersion]> = match registry.krate(&name).await? {
                    Some(krate) => versions_from_krate(&krate, &age).into(),
                    None => {
                        log::warn!("skipping '{name}': crate not found in the crates.io index");
                        Arc::default()
                    }
                };
                Ok::<_, CargoError>((name, versions))
            });
        }
        while let Some(result) = tasks.join_next().await {
            let (name, available) = result??;
            versions.insert(name, available);
        }
        Ok(versions)
    }

    async fn krate(&self, name: &str) -> Result<Option<IndexKrate>, CargoError> {
        let name = name.try_into()?;
        let lock = FileLock::unlocked();
        let request: Request<()> = self.index.make_remote_request(name, None, &lock)?.into();
        let mut response = self.fetch(request).await?;
        let mut result = self
            .index
            .parse_remote_response(name, &response, true, &lock)
            .and_then(|entry| entry.map(|entry| entry.deserialize()).transpose());
        if response.status() == StatusCode::NOT_MODIFIED
            && !result.as_ref().is_ok_and(|entry| entry.is_some())
        {
            log::debug!("refetching '{name}' after missing or unreadable cache entry");
            let mut request: Request<()> =
                self.index.make_remote_request(name, None, &lock)?.into();
            request.headers_mut().remove(http::header::IF_NONE_MATCH);
            request
                .headers_mut()
                .remove(http::header::IF_MODIFIED_SINCE);
            drop(response);
            response = self.fetch(request).await?;
            result = self
                .index
                .parse_remote_response(name, &response, true, &lock)
                .and_then(|entry| entry.map(|entry| entry.deserialize()).transpose());
        }
        result.map_err(|source| CargoError::RegistryEntry {
            name: name.to_string(),
            source,
        })
    }

    async fn fetch(&self, request: Request<()>) -> Result<Response<Vec<u8>>, CargoError> {
        let (parts, ()) = request.into_parts();
        let mut response = self
            .client
            .request(parts.method, parts.uri.to_string())
            .headers(parts.headers)
            .send()
            .await?;
        let body = read_body(&mut response)
            .await
            .map_err(|error| CargoError::Body(format!("{}: {error}", response.url())))?;
        let mut builder = Response::builder()
            .status(response.status())
            .version(response.version());
        if let Some(headers) = builder.headers_mut() {
            *headers = std::mem::take(response.headers_mut());
        }
        Ok(builder.body(body)?)
    }
}
