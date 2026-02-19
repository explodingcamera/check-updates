use semver::VersionReq;
use thiserror::Error;

use crate::package::{Package, Usage};

pub(crate) mod cargo;
pub(crate) use cargo::CargoRegistry;

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("cargo registry error: {0}")]
    Cargo(#[from] cargo::CargoError),
}

pub(crate) trait RegistryImpl {
    /// Get the locally installed packages for this registry
    fn packages(&self) -> Result<impl IntoIterator<Item = Package>, RegistryError>;

    /// Update the locally installed versions of the given packages to the one specified
    fn update_versions<'a>(
        &self,
        packages: impl IntoIterator<Item = (&'a Usage, &'a Package, VersionReq)>,
    ) -> Result<(), RegistryError>;
}
