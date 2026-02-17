use std::rc::Rc;

use thiserror::Error;

use crate::{
    Purl, State,
    package::{Packages, Unit},
};

pub(crate) mod cargo;
pub(crate) use cargo::CargoRegistry;
// pub(crate) mod npm;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("cargo registry error: {0}")]
    Cargo(#[from] cargo::CargoError),
    // Npm(#[from] npm::NpmError),
}

pub(crate) enum Registry {
    Cargo(cargo::CargoRegistry),
    // Npm(npm::NpmRegistry),
}

pub(crate) trait RegistryImpl {
    const TYPE: &'static str;

    /// Initialize the registry by scanning the local environment for installed packages
    fn initialize(state: Rc<State>) -> Result<Self, RegistryError>
    where
        Self: Sized;

    /// Get the locally installed packages for this registry
    fn packages(&self) -> &Packages;

    /// Update the locally installed versions of the given packages to the one specified
    fn update_versions<'a>(
        &self,
        unit: &Unit,
        packages: impl IntoIterator<Item = &'a Purl>,
    ) -> Result<(), RegistryError>;
}
