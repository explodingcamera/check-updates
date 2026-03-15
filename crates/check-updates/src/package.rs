use crate::Purl;
use semver::VersionReq;
use std::{
    borrow::Cow,
    collections::HashMap,
    fmt,
    path::{Path, PathBuf},
};

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
            (Unit::Workspace { .. }, Unit::Workspace { .. }) => std::cmp::Ordering::Equal,
            (Unit::Workspace { .. }, _) => std::cmp::Ordering::Less,
            (_, Unit::Workspace { .. }) => std::cmp::Ordering::Greater,
            (Unit::Global, Unit::Global) => std::cmp::Ordering::Equal,
            (Unit::Global, _) => std::cmp::Ordering::Greater,
            (_, Unit::Global) => std::cmp::Ordering::Less,
            _ => self.name().cmp(&other.name()),
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
    pub rust_version: Option<semver::Version>,
}
