use std::borrow::Cow;
use std::cmp::Ordering;
use std::collections::HashMap;
use std::fmt;
use std::path::{Path, PathBuf};

use semver::VersionReq;

use crate::Purl;

/// A unit of package management, such as a project, a workspace, or a global environment
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Unit {
    /// A single project manifest (e.g. `crates/foo/Cargo.toml` `[dependencies]`)
    Project { manifest: PathBuf, name: String },
    /// The workspace root manifest (e.g. `Cargo.toml` `[workspace.dependencies]`)
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
}

impl fmt::Display for DepKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DepKind::Normal => write!(f, "dependencies"),
            DepKind::Dev => write!(f, "dev-dependencies"),
            DepKind::Build => write!(f, "build-dependencies"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Usage {
    pub unit: Unit,
    pub req: VersionReq,
    pub kind: DepKind,
    pub rename: Option<String>,
    pub supported_toolchain_version: Option<semver::Version>,
}

#[derive(Debug, Clone)]
pub struct Package {
    pub purl: Purl,
    pub usages: Vec<Usage>,
    pub versions: Vec<PackageVersion>,
    pub repository: Option<String>,
    pub homepage: Option<String>,
}

pub type Packages = HashMap<Unit, Vec<(VersionReq, DepKind, Package)>>;

#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub version: semver::Version,
    pub yanked: bool,
    pub features: HashMap<String, Vec<String>>,
    pub required_toolchain_version: Option<semver::Version>,
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
}
