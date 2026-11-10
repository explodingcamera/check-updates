use std::path::{Path, PathBuf};

use crate::Packages;

use crate::backend::{Backend, CargoBackend, NpmBackend};
use crate::{cli::Args, update};

/// A discovered project and its dependencies.
pub(crate) struct Project {
    pub(crate) root: PathBuf,
    pub(crate) names: Vec<String>,
    pub(crate) packages: Packages,
    pub(crate) backend: Backend,
}

impl Project {
    /// Load the projects in `root`.
    pub(super) async fn discover(root: &Path, args: &Args) -> Result<Vec<Self>, String> {
        let mut projects = Vec::new();
        if let Some(project) = CargoBackend::discover(root, args).await? {
            projects.push(project);
        }
        if let Some(project) = NpmBackend::discover(root, args).await? {
            projects.push(project);
        }
        if projects.is_empty() {
            return Err(format!(
                "No Cargo.toml or package.json found in {}",
                root.display()
            ));
        }
        for name in &args.package {
            if !projects.iter().any(|project| project.names.contains(name)) {
                return Err(format!("workspace package '{name}' not found"));
            }
        }
        Ok(projects)
    }

    /// Resolve dependency updates.
    pub(super) fn updates(&self, args: &Args, include_breaking: bool) -> update::Updates<'_> {
        update::resolve_updates(&self.packages, &args.version_strategy(), include_breaking)
    }

    /// Apply selected version requirements.
    pub(super) fn update_versions(&self, selected: &[&update::Update<'_>]) -> Result<(), String> {
        let edits = selected
            .iter()
            .filter(|entry| self.packages.contains_key(&entry.usage.unit))
            .map(|entry| (entry.usage, entry.package, entry.new_req.clone()));
        self.backend.update_versions(edits)
    }

    /// Update installed dependencies and the lockfile.
    pub(super) fn upgrade_dependencies(&self, args: &Args) -> Result<(), String> {
        let mut command = self.backend.upgrade_command(&self.root, args)?;
        let executable = command.get_program().to_string_lossy().into_owned();
        let status = command
            .status()
            .map_err(|error| format!("failed to run {executable}: {error}"))?;
        if !status.success() {
            return Err(format!("{executable} upgrade failed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        CheckUpdates, DepKind, Package, PackageVersion, Requirement, Unit, Usage, Version,
    };

    use super::*;

    #[test]
    fn applies_only_owned_updates() {
        let dir = tempfile::tempdir().unwrap();
        let mut projects = Vec::new();
        for (ecosystem, filename, contents) in [
            (
                "cargo",
                "Cargo.toml",
                "[dependencies]\nexample = \"^1.0.0\"\n",
            ),
            (
                "npm",
                "package.json",
                r#"{"dependencies":{"example":"^1.0.0"}}"#,
            ),
        ] {
            let manifest = dir.path().join(filename);
            std::fs::write(&manifest, contents).unwrap();
            let req = if ecosystem == "cargo" {
                Requirement::from_cargo("^1.0.0".parse().unwrap())
            } else {
                Requirement::from_node("^1.0.0").unwrap()
            };
            let unit = Unit::Project {
                manifest,
                name: "test".into(),
            };
            let package = Package {
                purl: format!("pkg:{ecosystem}/example").parse().unwrap(),
                usages: vec![Usage {
                    unit: unit.clone(),
                    req: req.clone(),
                    kind: DepKind::Normal,
                    rename: None,
                    supported_toolchain_version: None,
                }],
                versions: vec![PackageVersion {
                    version: Version::parse("1.1.0").unwrap(),
                    yanked: false,
                    features: Default::default(),
                    required_toolchain_version: None,
                }],
                repository: None,
                homepage: None,
            };
            projects.push(Project {
                root: dir.path().to_path_buf(),
                names: Vec::new(),
                packages: Packages::from([(unit, vec![(req, DepKind::Normal, package)])]),
                backend: if ecosystem == "cargo" {
                    CheckUpdates::new(Some(dir.path().to_path_buf()))
                        .cargo
                        .into()
                } else {
                    NpmBackend.into()
                },
            });
        }
        let args = crate::cli::args().run_inner(&[]).unwrap();
        let updates: Vec<_> = projects
            .iter()
            .map(|project| project.updates(&args, false))
            .collect();
        let entries: Vec<_> = updates
            .iter()
            .flat_map(|updates| updates.values().flatten())
            .collect();

        for project in &projects {
            project.update_versions(&[entries[1]]).unwrap();
        }
        let cargo = dir.path().join("Cargo.toml");
        let npm = dir.path().join("package.json");
        assert!(std::fs::read_to_string(&cargo).unwrap().contains("^1.0.0"));
        assert!(std::fs::read_to_string(&npm).unwrap().contains("^1.1.0"));

        for project in &projects {
            project.update_versions(&[entries[0]]).unwrap();
        }
        assert!(std::fs::read_to_string(cargo).unwrap().contains("^1.1.0"));
        assert!(std::fs::read_to_string(npm).unwrap().contains("^1.1.0"));
    }
}
