use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use curl::multi::Multi;
use semver::VersionReq;

use crate::registry::*;

mod package;
mod registry;

pub use package::{DepKind, Package, PackageVersion, Packages, Unit, Usage};

type Purl = purl::GenericPurl<String>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RegistryCachePolicy {
    #[default]
    PreferLocal,
    Refresh,
    NoCache,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Options {
    pub registry_cache_policy: RegistryCachePolicy,
}

pub struct State {
    multi: Multi,
    root: Option<PathBuf>,
    registry_cache_policy: RegistryCachePolicy,
}

impl State {
    pub fn new(root: Option<PathBuf>, options: Options) -> Self {
        let mut multi = Multi::new();
        multi.pipelining(false, true).ok();
        Self {
            multi,
            root,
            registry_cache_policy: options.registry_cache_policy,
        }
    }

    pub fn multi(&self) -> &Multi {
        &self.multi
    }

    pub fn root(&self) -> Option<&PathBuf> {
        self.root.as_ref()
    }

    pub fn registry_cache_policy(&self) -> RegistryCachePolicy {
        self.registry_cache_policy
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("registry error: {0}")]
    Registry(#[from] registry::RegistryError),
}

pub struct CheckUpdates {
    cargo: Registry,
}

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

    pub fn packages(&self) -> Result<Packages, Error> {
        let mut res: Packages = Default::default();
        // Track (unit, package_name, req) to deduplicate entries where the same
        // package appears in multiple dep-kind sections with the same version.
        let mut seen: HashSet<(Unit, String, String)> = HashSet::new();
        for package in self.cargo.packages()? {
            for usage in &package.usages {
                // Wildcard requirements have nothing to update.
                if usage.req == VersionReq::STAR {
                    continue;
                }
                let key = (
                    usage.unit.clone(),
                    package.purl.name().to_string(),
                    usage.req.to_string(),
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
        packages: impl IntoIterator<Item = (&'a Usage, &'a Package, VersionReq)>,
    ) -> Result<(), Error> {
        self.cargo.update_versions(packages)?;
        Ok(())
    }
}
