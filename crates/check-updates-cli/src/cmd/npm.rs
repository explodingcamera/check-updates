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

pub fn upgrade_dependencies(root: &Path, lockfile_only: bool) -> Result<(), String> {
    let manager = check_updates::npm::package_manager(root)?;
    let executable = match manager {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Bun => "bun",
    };
    let mut command = std::process::Command::new(executable);
    let status = command
        .args(upgrade_args(manager, lockfile_only))
        .current_dir(root)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("{executable} upgrade failed"));
    }
    Ok(())
}

fn upgrade_args(manager: PackageManager, lockfile_only: bool) -> Vec<&'static str> {
    // For Bun, use `install` rather than `update`: update rewrites package.json, and
    // `--no-save` also prevents saving the lockfile. See https://github.com/oven-sh/bun/issues/30407.
    let mut args = if manager == PackageManager::Bun {
        vec!["install"]
    } else {
        vec!["update"]
    };
    if manager == PackageManager::Pnpm {
        args.push("--no-save");
    }
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
    fn upgrade_arguments() {
        assert_eq!(
            upgrade_args(PackageManager::Npm, false),
            ["update", "--ignore-scripts"]
        );
        assert_eq!(
            upgrade_args(PackageManager::Npm, true),
            ["update", "--package-lock-only", "--ignore-scripts"]
        );
        assert_eq!(
            upgrade_args(PackageManager::Pnpm, false),
            ["update", "--no-save", "--ignore-scripts"]
        );
        assert_eq!(
            upgrade_args(PackageManager::Pnpm, true),
            ["update", "--no-save", "--lockfile-only", "--ignore-scripts"]
        );
        assert_eq!(
            upgrade_args(PackageManager::Bun, false),
            ["install", "--ignore-scripts"]
        );
        assert_eq!(
            upgrade_args(PackageManager::Bun, true),
            ["install", "--lockfile-only", "--ignore-scripts"]
        );
    }
}
