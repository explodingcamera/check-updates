use std::rc::Rc;

use curl::multi::Multi;
use semver::VersionReq;

use crate::{
    package::{Package, Packages, Usage},
    registry::*,
};
mod package;
mod registry;

type Purl = purl::GenericPurl<String>;

pub struct State {
    multi: Multi,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub fn new() -> Self {
        let mut multi = Multi::new();
        multi.pipelining(false, true).ok();
        Self { multi }
    }

    pub fn multi(&self) -> &Multi {
        &self.multi
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("registry error: {0}")]
    Registry(#[from] registry::RegistryError),
}

pub struct CheckUpdates {
    cargo: CargoRegistry,
}

impl Default for CheckUpdates {
    fn default() -> Self {
        Self::new()
    }
}

impl CheckUpdates {
    pub fn new() -> Self {
        let state = Rc::new(State::new());
        let cargo = CargoRegistry::new(state.clone());
        Self { cargo }
    }

    pub fn packages(&self) -> Result<Packages, Error> {
        let mut res: Packages = Default::default();
        for package in self.cargo.packages()? {
            for usage in &package.usages {
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
