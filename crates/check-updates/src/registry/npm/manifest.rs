use std::collections::BTreeMap;
use std::path::Path;

use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct PackageJson {
    pub name: Option<String>,
    #[serde(default)]
    pub workspaces: Option<Workspaces>,
    #[serde(default)]
    pub dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub dev_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub optional_dependencies: BTreeMap<String, String>,
    #[serde(default)]
    pub peer_dependencies: BTreeMap<String, String>,
    pub package_manager: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(untagged)]
pub(super) enum Workspaces {
    Patterns(Vec<String>),
    Config { packages: Vec<String> },
}

impl Workspaces {
    pub fn patterns(&self) -> &[String] {
        match self {
            Self::Patterns(patterns) => patterns,
            Self::Config { packages } => packages,
        }
    }
}

impl PackageJson {
    pub fn from_path(path: &Path) -> Result<Self, String> {
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        serde_json::from_str(&contents).map_err(|error| error.to_string())
    }
}

#[derive(Default, Deserialize)]
pub(super) struct PnpmWorkspace {
    #[serde(default)]
    pub packages: Vec<String>,
}

impl PnpmWorkspace {
    pub fn from_root(root: &Path) -> Result<Self, String> {
        let path = root.join("pnpm-workspace.yaml");
        if !path.is_file() {
            return Ok(Self::default());
        }
        let contents = std::fs::read_to_string(path).map_err(|error| error.to_string())?;
        serde_saphyr::from_str(&contents).map_err(|error| error.to_string())
    }
}
