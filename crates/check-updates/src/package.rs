use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use crate::{Purl, Requirement, Version};

/// A unit of package management, such as a project, a workspace, or a global environment
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Unit {
    /// A single project manifest (e.g. `crates/foo/Cargo.toml` `[dependencies]`)
    Project { manifest: PathBuf, name: String },
    /// A workspace root manifest (e.g. Cargo `[workspace.dependencies]` or root `package.json` dependencies)
    Workspace { manifest: PathBuf },
    /// A globally installed package
    Global,
}

impl Ord for Unit {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        match (self, other) {
            (Unit::Workspace { manifest: a }, Unit::Workspace { manifest: b }) => a.cmp(b),
            (Unit::Workspace { .. }, _) => Ordering::Less,
            (_, Unit::Workspace { .. }) => Ordering::Greater,
            (
                Unit::Project {
                    name: a,
                    manifest: am,
                },
                Unit::Project {
                    name: b,
                    manifest: bm,
                },
            ) => a.cmp(b).then_with(|| am.cmp(bm)),
            (Unit::Project { .. }, Unit::Global) => Ordering::Less,
            (Unit::Global, Unit::Project { .. }) => Ordering::Greater,
            (Unit::Global, Unit::Global) => Ordering::Equal,
        }
    }
}

impl PartialOrd for Unit {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Unit {
    /// Returns the path to the manifest file, if this unit has one.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Unit::Project { manifest, .. } | Unit::Workspace { manifest, .. } => Some(manifest),
            Unit::Global => None,
        }
    }

    /// Returns the name of the unit, if it has one.
    pub fn name(&self) -> Cow<'_, str> {
        match self {
            Unit::Project { name, .. } => name.into(),
            Unit::Workspace { manifest, .. } => manifest
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(|n| format!("{n} (workspace)"))
                .unwrap_or("workspace".into())
                .into(),
            Unit::Global => "global".into(),
        }
    }
}

/// The kind of dependency
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepKind {
    Normal,
    Dev,
    Build,
    Optional,
    Peer,
}

impl fmt::Display for DepKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DepKind::Normal => write!(f, "normal"),
            DepKind::Dev => write!(f, "dev"),
            DepKind::Build => write!(f, "build"),
            DepKind::Optional => write!(f, "optional"),
            DepKind::Peer => write!(f, "peer"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub unit: Unit,
    pub req: Requirement,
    pub kind: DepKind,
    pub rename: Option<String>,
    pub supported_toolchain_version: Option<Version>,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub purl: Purl,
    pub usages: Vec<Usage>,
    pub versions: Vec<PackageVersion>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
}

/// Options for selecting a published version of a package.
#[derive(Debug, Clone, Copy, Default)]
pub struct VersionStrategy {
    pub compatible: bool,
    pub pre: bool,
    pub ignore_toolchain_version: bool,
}

impl Package {
    /// Whether a published version has been yanked.
    pub fn is_version_yanked(&self, version: &Version) -> bool {
        self.versions
            .iter()
            .find(|entry| &entry.version == version)
            .is_some_and(|entry| entry.yanked)
    }

    /// Select the latest non-yanked version using a requirement and strategy.
    pub fn latest(
        &self,
        requirement: &Requirement,
        strategy: &VersionStrategy,
        supported_toolchain_version: Option<&Version>,
    ) -> Option<&Version> {
        strategy.select(
            &self.versions,
            requirement,
            requirement.current_version().as_ref(),
            supported_toolchain_version,
        )
    }
}

impl VersionStrategy {
    /// Select a published version using this strategy.
    pub fn select<'a>(
        &self,
        versions: &'a [PackageVersion],
        requirement: &Requirement,
        current: Option<&Version>,
        supported_toolchain_version: Option<&Version>,
    ) -> Option<&'a Version> {
        versions
            .iter()
            .filter(|v| !v.yanked)
            .filter(|v| self.allows_prerelease(&v.version, current))
            .filter(|v| !self.compatible || requirement.matches(&v.version))
            .filter(|v| {
                self.ignore_toolchain_version
                    || supported_toolchain_version.is_none_or(|supported| {
                        v.required_toolchain_version
                            .as_ref()
                            .is_none_or(|required| required <= supported)
                    })
            })
            .map(|v| &v.version)
            .max()
    }

    /// Prefer stable releases while allowing updates within the current prerelease.
    pub fn stable() -> Self {
        Self::default()
    }

    /// Include prerelease versions, regardless of the requirement's compatibility range.
    pub fn latest() -> Self {
        Self {
            pre: true,
            ..Self::default()
        }
    }

    pub(crate) fn allows_prerelease(&self, version: &Version, current: Option<&Version>) -> bool {
        self.pre
            || version.pre.is_empty()
            || current
                .filter(|current| !current.pre.is_empty())
                .is_some_and(|current| {
                    version.major == current.major
                        && version.minor == current.minor
                        && version.patch == current.patch
                })
    }
}

pub type Packages = HashMap<Unit, Vec<(Requirement, DepKind, Package)>>;

#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub version: Version,
    pub yanked: bool,
    pub features: HashMap<String, Vec<String>>,
    pub required_toolchain_version: Option<Version>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_ordering_is_consistent_with_equality() {
        let units = [
            Unit::Workspace {
                manifest: PathBuf::from("a/Cargo.toml"),
            },
            Unit::Workspace {
                manifest: PathBuf::from("b/Cargo.toml"),
            },
            Unit::Project {
                manifest: PathBuf::from("a/crates/app/Cargo.toml"),
                name: "app".to_string(),
            },
            Unit::Project {
                manifest: PathBuf::from("b/crates/app/Cargo.toml"),
                name: "app".to_string(),
            },
            Unit::Global,
        ];

        for left in &units {
            for right in &units {
                assert_eq!(left.cmp(right) == Ordering::Equal, left == right);
            }
        }
    }

    #[test]
    fn latest_respects_strategy_and_yanked_versions() {
        let mut package = Package {
            purl: Purl::new("cargo".to_string(), "example").unwrap(),
            usages: Vec::new(),
            versions: ["1.0.0", "2.0.0-alpha.1", "3.0.0", "1.2.0"]
                .into_iter()
                .map(|version| PackageVersion {
                    version: Version::parse(version).unwrap(),
                    yanked: version == "3.0.0",
                    features: HashMap::new(),
                    required_toolchain_version: None,
                })
                .collect(),
            repository: None,
            homepage: None,
        };
        assert!(package.is_version_yanked(&Version::parse("3.0.0").unwrap()));
        assert!(!package.is_version_yanked(&Version::parse("1.2.0").unwrap()));
        assert!(!package.is_version_yanked(&Version::parse("4.0.0").unwrap()));
        #[cfg(feature = "cargo")]
        {
            let req = Requirement::from_cargo("^1.0.0".parse().unwrap());
            assert_eq!(
                package.latest(&req, &VersionStrategy::stable(), None),
                Some(&Version::parse("1.2.0").unwrap())
            );
            assert_eq!(
                package.latest(&req, &VersionStrategy::latest(), None),
                Some(&Version::parse("2.0.0-alpha.1").unwrap())
            );
        }
        package
            .versions
            .iter_mut()
            .for_each(|version| version.yanked = true);
        #[cfg(feature = "cargo")]
        assert!(
            package
                .latest(
                    &Requirement::from_cargo("^1".parse().unwrap()),
                    &VersionStrategy::stable(),
                    None
                )
                .is_none()
        );
    }
}
