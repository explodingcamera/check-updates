use std::path::Path;

use check_updates::npm::{NpmOptions, NpmPackages, PackageManager};

use crate::cli::Args;

pub async fn fetch(root: &Path, args: &Args) -> Result<NpmPackages, String> {
    check_updates::npm::packages(
        root,
        &NpmOptions {
            packages: &args.package,
        },
    )
    .await
}

pub fn install_updates(root: &Path, lockfile_only: bool) -> Result<(), String> {
    let manager = check_updates::npm::package_manager(root)?;
    let executable = match manager {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Bun => "bun",
    };
    let mut command = std::process::Command::new(executable);
    let status = command
        .args(install_args(manager, lockfile_only))
        .current_dir(root)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("{executable} install failed"));
    }
    Ok(())
}

fn install_args(manager: PackageManager, lockfile_only: bool) -> Vec<&'static str> {
    let mut args = vec!["install"];
    if lockfile_only {
        args.push(if manager == PackageManager::Npm {
            "--package-lock-only"
        } else {
            "--lockfile-only"
        });
    }
    args.push("--ignore-scripts");
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_arguments() {
        for manager in [
            PackageManager::Npm,
            PackageManager::Pnpm,
            PackageManager::Bun,
        ] {
            assert_eq!(
                install_args(manager, false),
                ["install", "--ignore-scripts"]
            );
            assert_eq!(
                install_args(manager, true),
                [
                    "install",
                    if manager == PackageManager::Npm {
                        "--package-lock-only"
                    } else {
                        "--lockfile-only"
                    },
                    "--ignore-scripts"
                ]
            );
        }
    }
}
