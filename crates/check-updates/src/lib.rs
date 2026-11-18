use jiff::Span;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

use reqwest::Client;

pub mod cli;
pub use cli::run;

use crate::backend::{CargoBackend, RegistryError};

mod backend;
mod config;
mod package;
mod publish_age;
mod requirement;

pub use backend::npm;
pub use package::{DepKind, Package, PackageVersion, Packages, Unit, Usage, VersionStrategy};
pub use requirement::NpmRequirementError;
pub use requirement::Requirement;
pub use semver::Version;

type Purl = purl::GenericPurl<String>;

const USER_AGENT: &str = concat!(
    env!("CARGO_PKG_NAME"),
    "/",
    env!("CARGO_PKG_VERSION"),
    " (+",
    env!("CARGO_PKG_REPOSITORY"),
    ")"
);

/// Options for checking Cargo dependency versions.
#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    /// Override the publication age inherited from Cargo configuration, resolved in UTC.
    pub min_publish_age: Option<Span>,
    /// Ignore publication-age settings, including an explicit minimum.
    pub ignore_publish_age: bool,
}

pub struct State {
    client: Client,
    root: Option<PathBuf>,
    pub(crate) options: Options,
}

impl State {
    /// Create Cargo state, reporting HTTP client initialization failures.
    pub fn try_new(root: Option<PathBuf>, options: Options) -> Result<Self, reqwest::Error> {
        let client = crate::backend::http::client_builder().build()?;
        Ok(Self {
            client,
            root,
            options,
        })
    }

    pub fn client(&self) -> &Client {
        &self.client
    }

    pub fn root(&self) -> Option<&PathBuf> {
        self.root.as_ref()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("failed to initialize HTTP client: {0}")]
    HttpClient(#[from] reqwest::Error),
    #[error("registry error: {0}")]
    Registry(#[from] RegistryError),
}

pub struct CheckUpdates {
    cargo: CargoBackend,
}

impl CheckUpdates {
    /// Create a checker with the given root and options.
    pub fn try_new(root: Option<PathBuf>, options: Options) -> Result<Self, Error> {
        Ok(Self {
            cargo: CargoBackend::new(Rc::new(State::try_new(root, options)?)),
        })
    }

    pub async fn packages(&self) -> Result<Packages, Error> {
        self.packages_with_root()
            .await
            .map(|(_, packages)| packages)
    }

    /// Load packages and retain Cargo's workspace root for native upgrade commands.
    pub(crate) async fn packages_with_root(&self) -> Result<(PathBuf, Packages), Error> {
        let (root, dependencies) = self.cargo.packages().await?;
        let mut packages = Packages::new();
        // Track (unit, package_name, req, kind) to deduplicate while still
        // preserving distinct dep sections in output.
        let mut seen = HashSet::new();
        for package in dependencies {
            let package = Arc::new(package);
            for usage in &package.usages {
                // Wildcard requirements have nothing to update.
                if usage.req.is_wildcard()
                    || !seen.insert((
                        usage.unit.clone(),
                        package.purl.name().to_string(),
                        usage.req.to_string(),
                        usage.kind,
                    ))
                {
                    continue;
                }
                packages.entry(usage.unit.clone()).or_default().push((
                    usage.req.clone(),
                    usage.kind,
                    Arc::clone(&package),
                ));
            }
        }
        Ok((root, packages))
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
