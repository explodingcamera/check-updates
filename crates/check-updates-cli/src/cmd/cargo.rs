use std::path::Path;

use check_updates::{CheckUpdates, Options, Packages, RegistryCachePolicy};

use crate::{cli, update};

pub struct CargoProject {
    pub checker: CheckUpdates,
    pub packages: Packages,
    pub matched_packages: Vec<String>,
}

pub async fn fetch(args: &cli::Args) -> Result<CargoProject, String> {
    let options = Options {
        registry_cache_policy: match args.cache {
            cli::RegistryCacheMode::PreferLocal => RegistryCachePolicy::PreferLocal,
            cli::RegistryCacheMode::Refresh => RegistryCachePolicy::Refresh,
            cli::RegistryCacheMode::NoCache => RegistryCachePolicy::NoCache,
        },
    };
    let checker = CheckUpdates::with_options(args.root.clone(), options);
    let packages = checker
        .packages()
        .await
        .map_err(|error| error.to_string())?;
    let matched_packages = args
        .package
        .iter()
        .filter(|name| {
            packages
                .keys()
                .any(|unit| update::unit_matches_filter(unit, name))
        })
        .cloned()
        .collect();
    Ok(CargoProject {
        checker,
        packages,
        matched_packages,
    })
}

pub fn update_lockfile(root: Option<&Path>, ignore_toolchain_version: bool) -> Result<(), String> {
    let mut command = std::process::Command::new("cargo");
    command.arg("update");
    if ignore_toolchain_version {
        command.arg("--ignore-rust-version");
    }
    if let Some(root) = root {
        command.current_dir(root);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo update: {error}"))?;
    if !status.success() {
        return Err("cargo update failed".into());
    }
    Ok(())
}
