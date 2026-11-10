use std::fmt;

use crate::Version;

/// An invalid npm dependency requirement.
#[derive(Debug, thiserror::Error)]
pub enum NpmRequirementError {
    #[error("invalid npm alias: {0}")]
    InvalidAlias(String),
    #[error("invalid npm version range: {0}")]
    InvalidRange(String),
}

/// A dependency requirement with ecosystem-specific matching and rewriting.
#[derive(Debug, Clone)]
pub enum Requirement {
    Cargo(semver::VersionReq),
    Npm {
        text: String,
        range: nodejs_semver::Range,
    },
}

impl PartialEq for Requirement {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Cargo(a), Self::Cargo(b)) => a == b,
            (Self::Npm { text: a, .. }, Self::Npm { text: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl Eq for Requirement {}

impl Requirement {
    /// Wrap a parsed Cargo registry requirement.
    pub fn from_cargo(req: semver::VersionReq) -> Self {
        Self::Cargo(req)
    }

    /// Whether this requirement allows any version.
    pub fn is_wildcard(&self) -> bool {
        match self {
            Self::Cargo(req) => *req == semver::VersionReq::STAR,
            Self::Npm { text, .. } => text == "*",
        }
    }
    /// Extract the Cargo requirement, if this is a Cargo requirement.
    pub fn into_cargo(self) -> Option<semver::VersionReq> {
        match self {
            Self::Cargo(req) => Some(req),
            Self::Npm { .. } => None,
        }
    }

    /// Extract the npm requirement text, if this is an npm requirement.
    pub fn into_npm(self) -> Option<String> {
        match self {
            Self::Npm { text, .. } => Some(text),
            Self::Cargo(_) => None,
        }
    }

    /// Parse an npm requirement, including an optional `npm:` alias.
    pub fn from_node(text: &str) -> Result<Self, NpmRequirementError> {
        let range = if let Some(alias) = text.strip_prefix("npm:") {
            let (name, range) = alias
                .rsplit_once('@')
                .ok_or_else(|| NpmRequirementError::InvalidAlias(text.to_owned()))?;
            if name.is_empty() || range.is_empty() {
                return Err(NpmRequirementError::InvalidAlias(text.to_owned()));
            }
            range
        } else {
            text
        };
        Ok(Self::Npm {
            text: text.to_owned(),
            range: nodejs_semver::Range::parse(range)
                .map_err(|_| NpmRequirementError::InvalidRange(text.to_owned()))?,
        })
    }

    /// Return the version used to compare updates.
    pub fn current_version(&self) -> Option<Version> {
        match self {
            Self::Cargo(req) => {
                let [comparator] = req.comparators.as_slice() else {
                    return None;
                };
                let mut version = Version::new(
                    comparator.major,
                    comparator.minor.unwrap_or(0),
                    comparator.patch.unwrap_or(0),
                );
                version.pre = comparator.pre.clone();
                Some(version)
            }
            Self::Npm { range, .. } => range
                .min_version()
                .and_then(|version| Version::parse(&version.to_string()).ok()),
        }
    }

    /// Check a published concrete version against this ecosystem's range rules.
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Cargo(req) => req.matches(version),
            Self::Npm { range, .. } => nodejs_semver::Version::parse(version.to_string())
                .is_ok_and(|version| range.satisfies(&version)),
        }
    }

    /// Rewrite a simple requirement for a new version, preserving its operator and alias.
    /// Complex npm ranges are left unchanged rather than rewritten incorrectly.
    pub fn with_version(&self, version: &Version) -> Option<Self> {
        match self {
            Self::Cargo(req) => {
                if req.comparators.len() != 1 {
                    return Some(self.clone());
                }
                let old = req.to_string();
                let digit = old.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
                let prefix = &old[..digit];
                let value = if !version.pre.is_empty() {
                    version.to_string()
                } else {
                    match old[digit..].matches('.').count() + 1 {
                        1 => version.major.to_string(),
                        2 => format!("{}.{}", version.major, version.minor),
                        _ => format!("{}.{}.{}", version.major, version.minor, version.patch),
                    }
                };
                let updated = format!("{prefix}{value}")
                    .parse::<semver::VersionReq>()
                    .ok()?;
                updated.matches(version).then_some(Self::Cargo(updated))
            }
            Self::Npm { text, .. } => {
                let (alias, old) = if let Some(alias) = text.strip_prefix("npm:") {
                    let (name, range) = alias.rsplit_once('@')?;
                    (Some(name), range)
                } else {
                    (None, text.as_str())
                };
                let prefix = if old.starts_with('^') {
                    "^"
                } else if old.starts_with('~') {
                    "~"
                } else if old.starts_with(|c: char| c.is_ascii_digit()) {
                    ""
                } else {
                    return None;
                };
                let old_version = &old[prefix.len()..];
                let numeric = old_version
                    .split_once(['-', '+'])
                    .map_or(old_version, |(numeric, _)| numeric);
                let parts: Vec<_> = numeric.split('.').collect();
                if parts.is_empty()
                    || parts.len() > 3
                    || parts.iter().any(|part| part.parse::<u64>().is_err())
                    || (numeric != old_version && parts.len() != 3)
                    || (parts.len() == 3 && Version::parse(old_version).is_err())
                {
                    return None;
                }
                let value = if !version.pre.is_empty() {
                    version.to_string()
                } else {
                    match parts.len() {
                        1 => version.major.to_string(),
                        2 => format!("{}.{}", version.major, version.minor),
                        _ => version.to_string(),
                    }
                };
                let text = match alias {
                    Some(name) => format!("npm:{name}@{prefix}{value}"),
                    None => format!("{prefix}{value}"),
                };
                Self::from_node(&text).ok()
            }
        }
    }
}

impl fmt::Display for Requirement {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Cargo(req) => req.fmt(f),
            Self::Npm { text, .. } => text.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cargo_current_version() {
        for (requirement, expected) in [
            ("^1", "1.0.0"),
            ("^1.53", "1.53.0"),
            ("^1.2.3", "1.2.3"),
            ("~0.4", "0.4.0"),
            ("~0.4.0", "0.4.0"),
            ("^0.4.50", "0.4.50"),
            (">=1.0.0", "1.0.0"),
            ("1.*", "1.0.0"),
            ("1.2.*", "1.2.0"),
            ("0.*", "0.0.0"),
            ("=1.0.0-beta.1", "1.0.0-beta.1"),
        ] {
            let req = Requirement::from_cargo(requirement.parse().unwrap());
            assert_eq!(
                req.current_version(),
                Some(Version::parse(expected).unwrap()),
                "{requirement}"
            );
        }
    }

    #[test]
    fn cargo_current_version_skips_complex_ranges() {
        for requirement in ["*", ">=1.0, <2.0"] {
            let req = Requirement::from_cargo(requirement.parse().unwrap());
            assert_eq!(req.current_version(), None, "{requirement}");
        }
    }

    #[test]
    fn rewrites_cargo_requirements() {
        for (requirement, version, expected) in [
            ("^1.2.3", "2.0.0", Some("^2.0.0")),
            ("~1.2.3", "1.3.0", Some("~1.3.0")),
            ("1", "2.3.4", Some("^2")),
            ("1.2", "2.3.4", Some("^2.3")),
            ("^1.0.0-alpha.1", "1.0.0-beta.2", Some("^1.0.0-beta.2")),
            ("=1.0.0-alpha.1", "1.0.0-beta.2", Some("=1.0.0-beta.2")),
            (">=1.0, <2.0", "3.4.5", Some(">=1.0, <2.0")),
            (">1.2.3", "1.3.4", None),
            ("<1.2.3", "1.3.4", None),
            ("<=1.2.3", "1.3.4", Some("<=1.3.4")),
        ] {
            let req = Requirement::from_cargo(requirement.parse().unwrap());
            assert_eq!(
                req.with_version(&Version::parse(version).unwrap())
                    .map(|req| req.to_string()),
                expected.map(str::to_string),
                "{requirement} -> {version}"
            );
        }
    }

    #[test]
    fn npm_alias_preserves_range() {
        let req = Requirement::from_node("npm:@scope/pkg@~1.2.0").unwrap();
        assert!(req.matches(&Version::parse("1.2.3").unwrap()));
        assert!(!req.matches(&Version::parse("2.0.0").unwrap()));
        assert_eq!(
            req.with_version(&Version::parse("2.3.4").unwrap())
                .unwrap()
                .to_string(),
            "npm:@scope/pkg@~2.3.4"
        );
    }

    #[test]
    fn unusual_npm_specs_are_not_rewritten() {
        let next = Version::parse("2.4.3").unwrap();
        for spec in [
            "1.x",
            "^1.2.x",
            "1.*",
            ">=1 <3",
            "1.0.0 || 2.0.0",
            "npm:@scope/pkg@~1.x",
        ] {
            assert!(
                Requirement::from_node(spec)
                    .unwrap()
                    .with_version(&next)
                    .is_none(),
                "{spec}"
            );
        }
        for spec in [
            "latest",
            "next",
            "file:../local",
            "workspace:*",
            "git+https://example.com/repo.git",
        ] {
            assert!(Requirement::from_node(spec).is_err(), "{spec}");
        }
        assert_eq!(
            Requirement::from_node("~1.2")
                .unwrap()
                .with_version(&Version::parse("2.0.0-beta.1").unwrap())
                .unwrap()
                .to_string(),
            "~2.0.0-beta.1"
        );
    }
}
