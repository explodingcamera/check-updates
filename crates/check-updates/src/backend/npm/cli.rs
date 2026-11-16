use std::path::Path;
use std::process::Command;

use crate::npm::{NpmOptions, NpmPackages};
use crate::{Package, Requirement, Usage};

use crate::backend::BackendImpl;
use crate::cli::{Args, project::Project};

use super::NpmBackend;

impl NpmBackend {
    /// Find and load a JavaScript project.
    pub(crate) async fn discover(root: &Path, args: &Args) -> Result<Option<Project>, String> {
        let Some(root) = super::project::root(root)? else {
            return Ok(None);
        };
        let root = root.as_path();
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
        Ok(crate::npm::package_manager(root)?.upgrade_command(root, args))
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
