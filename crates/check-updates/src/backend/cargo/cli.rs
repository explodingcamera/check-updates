use std::path::Path;
use std::process::Command;

use crate::{CheckUpdates, Options, Package, Requirement, Usage};

use super::CargoBackend;
use crate::backend::BackendImpl;
use crate::cli::{Args, project::Project, update};

impl CargoBackend {
    /// Find and load a Cargo project.
    pub(crate) async fn discover(root: &Path, args: &Args) -> Result<Option<Project>, String> {
        let start = root
            .canonicalize()
            .map_err(|error| format!("{}: {error}", root.display()))?;
        if !start
            .ancestors()
            .any(|directory| directory.join("Cargo.toml").is_file())
        {
            return Ok(None);
        }
        let options = Options {
            min_publish_age: args.min_publish_age,
            ignore_publish_age: args.ignore_publish_age,
        };
        let checker =
            CheckUpdates::try_new(Some(start), options).map_err(|error| error.to_string())?;
        let (root, mut packages) = checker
            .packages_with_root()
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
            root,
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
        if args.ignore_publish_age {
            command.args(["--config", "resolver.incompatible-publish-age=\"allow\""]);
        } else if let Some(age) = args.native_publish_age()? {
            // Cargo accepts only whole seconds. Round up to preserve the minimum.
            let seconds = age.as_secs() + u64::from(age.subsec_nanos() != 0);
            command.args(["--config", "resolver.incompatible-publish-age=\"deny\""]);
            for key in [
                "registry.global-min-publish-age",
                "registries.crates-io.min-publish-age",
            ] {
                command
                    .arg("--config")
                    .arg(format!("{key}=\"{seconds} seconds\""));
            }
        }
        Ok(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forwards_publish_age_overrides() {
        let dir = tempfile::tempdir().unwrap();
        let backend = CheckUpdates::try_new(Some(dir.path().to_path_buf()), Options::default())
            .unwrap()
            .cargo;
        for (flags, behavior, seconds) in [
            (&["--min-publish-age", "1d"][..], "deny", 86400),
            (&["--min-publish-age", "500ms"][..], "deny", 1),
            (&["--min-publish-age", "0"][..], "deny", 0),
            (
                &["--min-publish-age", "1d", "--ignore-publish-age"][..],
                "allow",
                0,
            ),
        ] {
            let args = crate::cli::args().run_inner(flags).unwrap();
            let command = backend.upgrade_command(dir.path(), &args).unwrap();
            let mut expected = vec![
                "update".to_string(),
                "--config".into(),
                format!("resolver.incompatible-publish-age=\"{behavior}\""),
            ];
            if behavior == "deny" {
                expected.extend([
                    "--config".into(),
                    format!("registry.global-min-publish-age=\"{seconds} seconds\""),
                    "--config".into(),
                    format!("registries.crates-io.min-publish-age=\"{seconds} seconds\""),
                ]);
            }
            assert_eq!(
                command
                    .get_args()
                    .map(|arg| arg.to_str().unwrap())
                    .collect::<Vec<_>>(),
                expected
            );
        }
    }
}
