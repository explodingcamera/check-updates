use std::path::Path;
use std::process::Command;

use super::manifest::PackageJson;
use crate::cli::Args;

mod bun;
mod npm;
pub(super) mod pnpm;

use bun::Bun;
use npm::Npm;
use pnpm::Pnpm;

/// Package manager selected from a lockfile or `packageManager` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Npm,
    Pnpm,
    Bun,
}

/// Detect the tool that owns this project's lockfile, if one exists.
pub fn package_manager(root: &Path) -> Result<PackageManager, String> {
    if root.join("yarn.lock").is_file() {
        return Err("unsupported lockfile 'yarn.lock'".into());
    }
    let lockfiles = [
        ("package-lock.json", PackageManager::Npm),
        ("npm-shrinkwrap.json", PackageManager::Npm),
        ("pnpm-lock.yaml", PackageManager::Pnpm),
        ("bun.lock", PackageManager::Bun),
        ("bun.lockb", PackageManager::Bun),
    ];
    let detected: Vec<_> = lockfiles
        .iter()
        .filter(|(file, _)| root.join(file).is_file())
        .collect();
    if detected.len() > 1 {
        return Err(format!("multiple lockfiles found in {}", root.display()));
    }
    if let Some((_, manager)) = detected.first() {
        return Ok(*manager);
    }
    let manifest = PackageJson::from_path(&root.join("package.json"))?;
    let manager = manifest
        .package_manager
        .as_deref()
        .map(|value| value.split_once('@').map_or(value, |(name, _)| name));
    match manager {
        Some("pnpm") => Ok(PackageManager::Pnpm),
        Some("bun") => Ok(PackageManager::Bun),
        Some("npm") => Ok(PackageManager::Npm),
        None if root.join("pnpm-workspace.yaml").is_file() => Ok(PackageManager::Pnpm),
        None => Ok(PackageManager::Npm),
        Some(other) => Err(format!("unsupported package manager '{other}'")),
    }
}

disponent::declare!(
    #[disponent::configure(inherent, from)]
    enum Manager {
        Npm(Npm),
        Pnpm(Pnpm),
        Bun(Bun),
    }

    trait ManagerImpl {
        fn upgrade_command(&self, lockfile_only: bool) -> Command;
    }
);

impl From<PackageManager> for Manager {
    fn from(manager: PackageManager) -> Self {
        match manager {
            PackageManager::Npm => Npm.into(),
            PackageManager::Pnpm => Pnpm.into(),
            PackageManager::Bun => Bun.into(),
        }
    }
}

impl PackageManager {
    /// Build a native command using the selected manager's flags.
    pub(super) fn upgrade_command(self, root: &Path, args: &Args) -> Command {
        let mut command = Manager::from(self).upgrade_command(args.lockfile_only);
        command.current_dir(root);
        command
    }
}
