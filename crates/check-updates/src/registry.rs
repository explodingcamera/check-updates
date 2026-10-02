#[cfg(feature = "cargo")]
use thiserror::Error;

#[cfg(feature = "cargo")]
use crate::Requirement;
#[cfg(feature = "cargo")]
use crate::package::{Package, Usage};

#[cfg(feature = "cargo")]
pub(crate) mod cargo;
#[cfg(feature = "cargo")]
pub(crate) use cargo::CargoRegistry;
#[cfg(feature = "npm")]
pub mod npm;

#[cfg(feature = "cargo")]
#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("cargo registry error: {0}")]
    Cargo(#[from] cargo::CargoError),
    #[error("expected a Cargo version requirement")]
    WrongRequirement,
}

#[cfg(feature = "cargo")]
disponent::declare!(
    #[disponent::configure(inherent, from)]
    pub enum Registry {
        Cargo(CargoRegistry),
    }

    pub(crate) trait RegistryImpl {
        /// Get the locally installed packages for this registry
        async fn packages(&self) -> Result<Vec<Package>, RegistryError>;

        /// Update the locally installed versions of the given packages to the one specified
        fn update_versions<'a>(
            &self,
            packages: impl IntoIterator<Item = (&'a Usage, &'a Package, Requirement)>,
        ) -> Result<(), RegistryError>;
    }
);
