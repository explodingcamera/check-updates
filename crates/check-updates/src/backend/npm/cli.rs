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
            crate::npm::PackageManager::detect(root)?;
        }
        let backend = Self::try_new()?;
        let NpmPackages { projects, packages } = backend
            .packages(
                root,
                &NpmOptions {
                    packages: &args.package,
                    min_publish_age: args.min_publish_age,
                    ignore_publish_age: args.ignore_publish_age,
                },
            )
            .await?;
        Ok(Some(Project {
            root: root.to_path_buf(),
            names: projects,
            packages,
            backend: backend.into(),
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
        crate::npm::PackageManager::detect(root)?.upgrade_command(root, args)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upgrade_commands() {
        let dir = tempfile::tempdir().unwrap();
        let backend = NpmBackend::try_new().unwrap();
        for (manager, args, lockfile_arg, age_args, zero_args) in [
            (
                "npm",
                &["update", "--ignore-scripts"][..],
                "--package-lock-only",
                &["--min-release-age=1"][..],
                &["--min-release-age=0"][..],
            ),
            (
                "pnpm",
                &["update", "--no-save", "--ignore-scripts"][..],
                "--lockfile-only",
                &["--config.minimum-release-age=1440"][..],
                &["--config.minimum-release-age=0"][..],
            ),
            (
                "bun",
                &["install", "--ignore-scripts"][..],
                "--lockfile-only",
                &["--minimum-release-age", "86400"][..],
                &["--minimum-release-age", "0"][..],
            ),
        ] {
            std::fs::write(
                dir.path().join("package.json"),
                format!(r#"{{"packageManager":"{manager}@1.0.0"}}"#),
            )
            .unwrap();
            for lockfile_only in [false, true] {
                for (flags, extra) in [
                    (&[][..], &[][..]),
                    (&["--min-publish-age", "1d"][..], age_args),
                    (&["--min-publish-age", "0"][..], zero_args),
                    (
                        &["--min-publish-age", "1d", "--ignore-publish-age"][..],
                        zero_args,
                    ),
                ] {
                    let mut options = crate::cli::args().run_inner(flags).unwrap();
                    options.lockfile_only = lockfile_only;
                    let command = backend.upgrade_command(dir.path(), &options).unwrap();
                    let mut expected = args.to_vec();
                    if lockfile_only {
                        expected.push(lockfile_arg);
                    }
                    expected.extend_from_slice(extra);
                    assert_eq!(command.get_program(), manager);
                    assert_eq!(command.get_current_dir(), Some(dir.path()));
                    assert_eq!(command.get_args().collect::<Vec<_>>(), expected);
                }
            }
        }
    }
}
