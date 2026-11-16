use std::path::Path;
use std::process::Command;
use std::time::Duration;

use super::manifest::PackageJson;
use crate::cli::Args;
use crate::config::ConfigEnv;
use crate::publish_age::PublishAge;

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

#[derive(Default)]
struct PublishAgeConfig {
    minimum: Option<Duration>,
    excludes: Vec<String>,
    ignore_missing: Option<bool>,
    patterns: bool,
}

disponent::declare!(
    #[disponent::configure(inherent, from)]
    enum Manager {
        Npm(Npm),
        Pnpm(Pnpm),
        Bun(Bun),
    }

    trait ManagerImpl {
        fn resolve_config(&self, root: &Path, env: &ConfigEnv) -> PublishAgeConfig;
        fn upgrade_command(&self, lockfile_only: bool, minimum: Option<Duration>) -> Command;
    }
);

/// Resolve the selected manager's settings and apply explicit publication-age overrides.
pub(super) fn publish_age(
    root: &Path,
    manager: PackageManager,
    minimum: Option<Duration>,
    ignore: bool,
    env: &ConfigEnv,
) -> PublishAge {
    if ignore || minimum.is_some_and(|minimum| minimum.is_zero()) {
        return PublishAge::new(None);
    }
    let config = Manager::from(manager).resolve_config(root, env);
    let mut policy = PublishAge::new(minimum.or(config.minimum));
    policy.excludes = config.excludes;
    policy.patterns = config.patterns;
    policy.ignore_missing_time = config.ignore_missing.unwrap_or(true);
    policy
}

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
    /// Build a native command using the selected manager's flags and age units.
    pub(super) fn upgrade_command(self, root: &Path, args: &Args) -> Command {
        let minimum = if args.ignore_publish_age {
            Some(Duration::ZERO)
        } else {
            args.min_publish_age
        };
        let mut command = Manager::from(self).upgrade_command(args.lockfile_only, minimum);
        command.current_dir(root);
        command
    }
}

#[cfg(test)]
mod tests {
    use super::super::npmrc::last;
    use super::*;

    #[test]
    fn resolves_npmrc_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir(root.join("config")).unwrap();
        std::fs::write(
            root.join(".npmrc"),
            "userconfig=config/user\nglobalconfig=config/global\n",
        )
        .unwrap();
        std::fs::write(root.join("config/user"), "min-release-age=2\n").unwrap();
        std::fs::write(root.join("config/global"), "min-release-age=7\n").unwrap();
        let mut env = ConfigEnv::default();
        let [global, user, _] = npm::npmrc_sources(root, &env);
        assert_eq!(last(&global, "min-release-age"), Some("7"));
        assert_eq!(last(&user, "min-release-age"), Some("2"));
        env.0
            .insert("NPM_CONFIG_GLOBALCONFIG".into(), "config/user".into());
        assert_eq!(
            last(&npm::npmrc_sources(root, &env)[0], "min-release-age"),
            Some("2")
        );
        env.0
            .insert("NPM_CONFIG_USERCONFIG".into(), "config/global".into());
        env.0
            .insert("PNPM_CONFIG_USERCONFIG".into(), "config/user".into());
        let [_, user, _] = pnpm::npmrc_sources(root, &env);
        assert_eq!(last(&user, "min-release-age"), Some("2"));
        env.0.insert(
            "NPM_CONFIG_GLOBALCONFIG".into(),
            "${UNSET_CONFIG}/npmrc".into(),
        );
        let [global, user, project] = pnpm::npmrc_sources(root, &env);
        assert!(global.is_empty());
        assert_eq!(last(&user, "min-release-age"), Some("2"));
        assert_eq!(last(&project, "userconfig"), Some("config/user"));
    }
}
