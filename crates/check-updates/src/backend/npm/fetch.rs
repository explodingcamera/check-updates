use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use serde::Deserialize;
use serde_json::Value;
use tokio::task::JoinSet;

use super::NpmBackend;
use crate::publish_age::PublishAge;
use crate::{PackageVersion, Version};

const MAX_CONCURRENT_FETCHES: usize = 8;

#[derive(Deserialize)]
struct RegistryMetadata {
    versions: BTreeMap<String, RegistryVersion>,
    time: Option<BTreeMap<String, Value>>,
}

#[derive(Deserialize)]
struct RegistryVersion {
    deprecated: Option<Value>,
}

impl NpmBackend {
    /// Fetch published versions
    pub(super) async fn versions(
        &self,
        names: BTreeSet<String>,
        publish_age: PublishAge,
    ) -> Result<BTreeMap<String, Arc<[PackageVersion]>>, String> {
        let mut versions = BTreeMap::new();
        let publish_age = Arc::new(publish_age);
        let mut tasks = JoinSet::new();
        for name in names {
            if tasks.len() == MAX_CONCURRENT_FETCHES
                && let Some(result) = tasks.join_next().await
            {
                let (name, available) = result.map_err(|error| error.to_string())??;
                versions.insert(name, available);
            }
            let backend = self.clone();
            let publish_age = publish_age.clone();
            tasks.spawn(async move {
                let available: Arc<[PackageVersion]> = backend
                    .metadata(&name)
                    .await?
                    .map(|body| available_versions(&name, &body, &publish_age))
                    .unwrap_or_default()
                    .into();
                Ok::<_, String>((name, available))
            });
        }
        while let Some(result) = tasks.join_next().await {
            let (name, available) = result.map_err(|error| error.to_string())??;
            versions.insert(name, available);
        }
        Ok(versions)
    }

    async fn metadata(&self, name: &str) -> Result<Option<RegistryMetadata>, String> {
        let mut url = self.registry.clone();
        url.path_segments_mut()
            .map_err(|_| "invalid registry URL".to_string())?
            .pop_if_empty()
            .push(name);
        let mut retries = 0;
        let mut response = loop {
            let response = self
                .client
                .get(url.clone())
                .header("Accept", "application/json")
                .send()
                .await
                .map_err(|error| error.to_string())?;
            let retryable = response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                || response.status().is_server_error();
            if !retryable || retries == 2 {
                break response;
            }
            let delay = retry_after(response.headers(), jiff::Timestamp::now())
                .unwrap_or(Duration::from_millis(500 * (1 << retries)));
            // Do not retry early or allow a server to stall a check indefinitely.
            if delay > Duration::from_secs(30) {
                return Err(format!(
                    "registry requested a retry delay over 30 seconds for {name}"
                ));
            }
            drop(response);
            tokio::time::sleep(delay).await;
            retries += 1;
        };
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            log::debug!("skipping {name}: not found on the public npm registry");
            return Ok(None);
        }
        response = response
            .error_for_status()
            .map_err(|error| error.to_string())?;
        let bytes = crate::backend::http::read_body(&mut response)
            .await
            .map_err(|error| format!("failed to read registry metadata for {name}: {error}"))?;
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("invalid registry metadata for {name}: {error}"))
    }
}

fn retry_after(headers: &http::HeaderMap, now: jiff::Timestamp) -> Option<Duration> {
    let value = headers
        .get(http::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = jiff::fmt::rfc2822::DateTimeParser::new()
        .parse_timestamp(value)
        .ok()?;
    Some(
        date.duration_since(now)
            .try_into()
            .unwrap_or(Duration::ZERO),
    )
}

fn available_versions(
    name: &str,
    body: &RegistryMetadata,
    age: &PublishAge,
) -> Vec<PackageVersion> {
    body.versions
        .iter()
        .filter(|(_, data)| {
            data.deprecated
                .as_ref()
                .and_then(Value::as_str)
                .is_none_or(str::is_empty)
        })
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
