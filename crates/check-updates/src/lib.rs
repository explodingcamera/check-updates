#[cfg(not(any(feature = "cargo", feature = "npm")))]
compile_error!("enable at least one backend feature: cargo or npm");

#[cfg(feature = "cargo")]
use std::collections::HashSet;
use std::path::PathBuf;
#[cfg(feature = "cargo")]
use std::rc::Rc;
use std::time::Duration;

use reqwest::Client;

#[cfg(feature = "cargo")]
use crate::registry::*;

#[cfg(any(feature = "cargo", feature = "npm"))]
mod package;
mod registry;
#[cfg(any(feature = "cargo", feature = "npm"))]
mod requirement;

#[cfg(any(feature = "cargo", feature = "npm"))]
pub use package::{DepKind, Package, PackageVersion, Packages, Unit, Usage, VersionStrategy};
#[cfg(feature = "npm")]
pub use registry::npm;
#[cfg(feature = "npm")]
pub use requirement::NpmRequirementError;
#[cfg(any(feature = "cargo", feature = "npm"))]
pub use requirement::Requirement;
/// Concrete published version shared by all supported registries.
pub use semver::Version;

#[cfg(any(feature = "cargo", feature = "npm"))]
type Purl = purl::GenericPurl<String>;

const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegistryCachePolicy {
    PreferLocal,
    #[default]
    Refresh,
    NoCache,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub registry_cache_policy: RegistryCachePolicy,
}

pub struct State {
    client: Client,
    root: Option<PathBuf>,
    registry_cache_policy: RegistryCachePolicy,
}

impl State {
    pub fn new(root: Option<PathBuf>, options: Options) -> Self {
        let client = Client::builder()
            .http2_adaptive_window(true)
            .timeout(HTTP_TIMEOUT)
            .user_agent(concat!(
                env!("CARGO_PKG_NAME"),
                "/",
                env!("CARGO_PKG_VERSION"),
                " (+",
                env!("CARGO_PKG_REPOSITORY"),
                ")"
            ))
            .build()
            .expect("failed to initialize HTTP client");
        Self {
            client,
            root,
            registry_cache_policy: options.registry_cache_policy,
        }
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn root(&self) -> Option<&PathBuf> {
        self.root.as_ref()
    }

    pub fn registry_cache_policy(&self) -> RegistryCachePolicy {
        self.registry_cache_policy
    }
}

#[cfg(feature = "cargo")]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),
}

#[cfg(feature = "cargo")]
pub struct CheckUpdates {
    cargo: Registry,
}

#[cfg(feature = "cargo")]
impl CheckUpdates {
    pub fn new(root: Option<PathBuf>) -> Self {
        Self::with_options(root, Options::default())
    }

    pub fn with_options(root: Option<PathBuf>, options: Options) -> Self {
        let state = Rc::new(State::new(root, options));
        let cargo = CargoRegistry::new(state.clone());

        Self {
            cargo: cargo.into(),
        }
    }

    pub async fn packages(&self) -> Result<Packages, Error> {
        let mut res: Packages = Default::default();
        // Track (unit, package_name, req, kind) to deduplicate while still
        // preserving distinct dep sections in output.
        let mut seen: HashSet<(Unit, String, String, DepKind)> = HashSet::new();
        for package in self.cargo.packages().await? {
            for usage in &package.usages {
                // Wildcard requirements have nothing to update.
                if usage.req.is_wildcard() {
                    continue;
                }
                let key = (
                    usage.unit.clone(),
                    package.purl.name().to_string(),
                    usage.req.to_string(),
                    usage.kind,
                );
                if !seen.insert(key) {
                    continue;
                }
                res.entry(usage.unit.clone()).or_default().push((
                    usage.req.clone(),
                    usage.kind,
                    package.clone(),
                ));
            }
        }
        Ok(res)
    }

    /// Update the locally installed versions of the given packages to the one specified
    pub fn update_versions<'a>(
        &self,
        packages: impl IntoIterator<Item = (&'a Usage, &'a Package, Requirement)>,
    ) -> Result<(), Error> {
        self.cargo.update_versions(packages)?;
        Ok(())
    }
}
