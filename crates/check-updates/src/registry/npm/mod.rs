use std::rc::Rc;

use thiserror::Error;

use crate::{
    Purl, State,
    package::{Packages, Unit},
    registry::RegistryError,
};

#[derive(Debug, Error)]
pub enum NpmError {
    #[error("Unimplemented: {0}")]
    Unimplemented(String),
}

pub struct NpmRegistry {
    packages: Packages,
}

impl super::RegistryImpl for NpmRegistry {
    const TYPE: &'static str = "npm";

    fn initialize(_state: Rc<State>) -> Result<Self, RegistryError> {
        Err(
            NpmError::Unimplemented("npm registry support is not yet implemented".to_string())
                .into(),
        )
    }

    fn packages(&self) -> &Packages {
        &self.packages
    }

    fn update_versions(&self, _unit: &Unit, _packages: &[&Purl]) {
        todo!()
    }
}
