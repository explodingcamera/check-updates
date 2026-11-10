use std::{path::Path, process::Command};

use crate::{Package, Requirement, Usage};

use crate::cli::Args;

pub(crate) mod cargo;
pub mod npm;

pub(crate) use cargo::CargoBackend;
pub(crate) use npm::NpmBackend;

/// A Cargo registry or manifest error.
#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("cargo registry error: {0}")]
    Cargo(#[from] cargo::CargoError),
    #[error("expected a Cargo version requirement")]
    WrongRequirement,
}

disponent::declare!(
    /// A dependency-management backend.
    #[disponent::configure(inherent, from)]
    pub(crate) enum Backend {
        Cargo(CargoBackend),
        Npm(NpmBackend),
    }

    /// Backend-specific project operations.
    pub(crate) trait BackendImpl {
        /// Apply version requirements to manifests.
        fn update_versions<'a>(
            &self,
            edits: impl IntoIterator<Item = (&'a Usage, &'a Package, Requirement)>,
        ) -> Result<(), String>;

        /// Build the dependency upgrade command.
        fn upgrade_command(&self, root: &Path, args: &Args) -> Result<Command, String>;

        /// Show a header for a single update group.
        fn show_headers(&self) -> bool {
            false
        }
    }
);
