use std::rc::Rc;

use curl::multi::Multi;

use crate::{
    package::{Packages, Unit},
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
    registries: Vec<Registry>,
}

impl CheckUpdates {
    pub fn new() -> Result<Self, Error> {
        let state = Rc::new(State::new());
        let registries = vec![Registry::Cargo(CargoRegistry::initialize(state)?)];
        Ok(Self { registries })
    }

    pub fn packages(&self) -> impl IntoIterator<Item = (&'static str, &Packages)> {
        self.registries.iter().map(|registry| match registry {
            Registry::Cargo(cargo) => (CargoRegistry::TYPE, cargo.packages()),
            // Registry::Npm(npm) => (NpmRegistry::TYPE, npm.packages()),
        })
    }

    /// Update the locally installed versions of the given packages to the one specified
    pub fn update_versions<P>(
        &self,
        registry_type: &'static str,
        unit: &Unit,
        packages: &P,
    ) -> Result<(), Error>
    where
        for<'a> &'a P: IntoIterator<Item = &'a Purl>,
    {
        for registry in &self.registries {
            match registry {
                Registry::Cargo(cargo) if CargoRegistry::TYPE == registry_type => {
                    cargo.update_versions(unit, packages)?;
                }
                // Registry::Npm(npm) if NpmRegistry::TYPE == registry_type => {
                //     npm.update_versions(unit, packages);
                // }
                _ => {}
            }
        }

        Ok(())
    }
}
