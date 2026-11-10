use std::path::Path;
use std::process::Command;

use crate::npm::{NpmOptions, NpmPackages, PackageManager};
use crate::{Package, Requirement, Usage};

use crate::backend::BackendImpl;
use crate::{cli::Args, cmd::project::Project};

use super::NpmBackend;

impl NpmBackend {
    /// Find and load a JavaScript project.
    pub(crate) async fn discover(root: &Path, args: &Args) -> Result<Option<Project>, String> {
        if !root.join("package.json").is_file() {
            return Ok(None);
        }
        if args.upgrade {
            crate::npm::package_manager(root)?;
        }
        let NpmPackages { projects, packages } = crate::npm::packages(
            root,
            &NpmOptions {
                packages: &args.package,
            },
        )
        .await?;
        Ok(Some(Project {
            root: root.to_path_buf(),
            names: projects,
            packages,
            backend: Self.into(),
        }))
    }
}

impl BackendImpl for NpmBackend {
    fn update_versions<'a>(
        &self,
        edits: impl IntoIterator<Item = (&'a Usage, &'a Package, Requirement)>,
    ) -> Result<(), String> {
        crate::npm::update_packages(edits)
    }

    fn upgrade_command(&self, root: &Path, args: &Args) -> Result<Command, String> {
        let manager = crate::npm::package_manager(root)?;
        let (executable, operation, lockfile_arg) = match manager {
            PackageManager::Npm => ("npm", "update", "--package-lock-only"),
            PackageManager::Pnpm => ("pnpm", "update", "--lockfile-only"),
            // Bun update rewrites package.json, so apply our edits with install instead.
            PackageManager::Bun => ("bun", "install", "--lockfile-only"),
        };
        let mut command = Command::new(executable);
        command.arg(operation).current_dir(root);
        if manager == PackageManager::Pnpm {
            command.arg("--no-save");
        }
        command.arg("--ignore-scripts");
        if args.lockfile_only {
            command.arg(lockfile_arg);
        }
        Ok(command)
    }

    fn show_headers(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_commands() {
        let dir = tempfile::tempdir().unwrap();
        for (manager, args, lockfile_arg) in [
            (
                "npm",
                &["update", "--ignore-scripts"][..],
                "--package-lock-only",
            ),
            (
                "pnpm",
                &["update", "--no-save", "--ignore-scripts"][..],
                "--lockfile-only",
            ),
            (
                "bun",
                &["install", "--ignore-scripts"][..],
                "--lockfile-only",
            ),
        ] {
            std::fs::write(
                dir.path().join("package.json"),
                format!(r#"{{"packageManager":"{manager}@1.0.0"}}"#),
            )
            .unwrap();
            for lockfile_only in [false, true] {
                let mut options = crate::cli::args().run_inner(&[]).unwrap();
                options.lockfile_only = lockfile_only;
                let command = NpmBackend.upgrade_command(dir.path(), &options).unwrap();
                let mut expected = args.to_vec();
                if lockfile_only {
                    expected.push(lockfile_arg);
                }
                assert_eq!(command.get_program(), manager);
                assert_eq!(command.get_current_dir(), Some(dir.path()));
                assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
            }
        }
    }
}
