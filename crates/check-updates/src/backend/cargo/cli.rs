use std::path::Path;
use std::process::Command;

use crate::{CheckUpdates, Options, Package, RegistryCachePolicy, Requirement, Usage};

use super::CargoBackend;
use crate::backend::BackendImpl;
use crate::{
    cli::{Args, RegistryCacheMode},
    cmd::project::Project,
    update,
};

impl CargoBackend {
    /// Find and load a Cargo project.
    pub(crate) async fn discover(root: &Path, args: &Args) -> Result<Option<Project>, String> {
        if !root.join("Cargo.toml").is_file() {
            return Ok(None);
        }
        let options = Options {
            registry_cache_policy: match args.cache {
                RegistryCacheMode::PreferLocal => RegistryCachePolicy::PreferLocal,
                RegistryCacheMode::Refresh => RegistryCachePolicy::Refresh,
                RegistryCacheMode::NoCache => RegistryCachePolicy::NoCache,
            },
        };
        let checker = CheckUpdates::with_options(Some(root.to_path_buf()), options);
        let mut packages = checker
            .packages()
            .await
            .map_err(|error| error.to_string())?;
        let names = args
            .package
            .iter()
            .filter(|name| {
                packages
                    .keys()
                    .any(|unit| update::unit_matches_filter(unit, name))
            })
            .cloned()
            .collect();
        if !args.package.is_empty() {
            packages.retain(|unit, _| {
                args.package
                    .iter()
                    .any(|name| update::unit_matches_filter(unit, name))
            });
        }
        Ok(Some(Project {
            root: root.to_path_buf(),
            names,
            packages,
            backend: checker.cargo.into(),
        }))
    }
}

impl BackendImpl for CargoBackend {
    fn update_versions<'a>(
        &self,
        edits: impl IntoIterator<Item = (&'a Usage, &'a Package, Requirement)>,
    ) -> Result<(), String> {
        self.update_versions(edits)
            .map_err(|error| error.to_string())
    }

    fn upgrade_command(&self, root: &Path, args: &Args) -> Result<Command, String> {
        let mut command = Command::new("cargo");
        command.arg("update").current_dir(root);
        if args.ignore_toolchain_version {
            command.arg("--ignore-rust-version");
        }
        Ok(command)
    }
}
