use std::fmt;

use crate::Version;

/// An invalid npm dependency requirement.
#[cfg(feature = "npm")]
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
    #[cfg(feature = "cargo")]
    Cargo(semver::VersionReq),
    #[cfg(feature = "npm")]
    Npm {
        text: String,
        range: nodejs_semver::Range,
    },
}

impl PartialEq for Requirement {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            #[cfg(feature = "cargo")]
            (Self::Cargo(a), Self::Cargo(b)) => a == b,
            #[cfg(feature = "npm")]
            (Self::Npm { text: a, .. }, Self::Npm { text: b, .. }) => a == b,
            #[cfg(all(feature = "cargo", feature = "npm"))]
            _ => false,
        }
    }
}

impl Eq for Requirement {}

impl Requirement {
    /// Wrap a parsed Cargo registry requirement.
    #[cfg(feature = "cargo")]
    pub fn from_cargo(req: semver::VersionReq) -> Self {
        Self::Cargo(req)
    }

    /// Whether this requirement allows any version.
    pub fn is_wildcard(&self) -> bool {
        match self {
            #[cfg(feature = "cargo")]
            Self::Cargo(req) => *req == semver::VersionReq::STAR,
            #[cfg(feature = "npm")]
            Self::Npm { text, .. } => text == "*",
        }
    }
    /// Extract the Cargo requirement, if this is a Cargo requirement.
    #[cfg(feature = "cargo")]
    pub fn into_cargo(self) -> Option<semver::VersionReq> {
        match self {
            Self::Cargo(req) => Some(req),
            #[cfg(feature = "npm")]
            Self::Npm { .. } => None,
        }
    }

    /// Extract the npm requirement text, if this is an npm requirement.
    #[cfg(feature = "npm")]
    pub fn into_npm(self) -> Option<String> {
        match self {
            Self::Npm { text, .. } => Some(text),
            #[cfg(feature = "cargo")]
            Self::Cargo(_) => None,
        }
    }

    /// Parse an npm requirement, including an optional `npm:` alias.
    #[cfg(feature = "npm")]
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

    /// Return the lowest version described by this requirement when available.
    pub fn current_version(&self) -> Option<Version> {
        match self {
            #[cfg(feature = "cargo")]
            Self::Cargo(req) => {
                let text = req.to_string();
                Version::parse(text.trim_start_matches(|c: char| !c.is_ascii_digit())).ok()
            }
            #[cfg(feature = "npm")]
            Self::Npm { range, .. } => range
                .min_version()
                .and_then(|version| Version::parse(&version.to_string()).ok()),
        }
    }

    /// Check a published concrete version against this ecosystem's range rules.
    pub fn matches(&self, version: &Version) -> bool {
        match self {
            #[cfg(feature = "cargo")]
            Self::Cargo(req) => req.matches(version),
            #[cfg(feature = "npm")]
            Self::Npm { range, .. } => nodejs_semver::Version::parse(version.to_string())
                .is_ok_and(|version| range.satisfies(&version)),
        }
    }

    /// Rewrite a simple requirement for a new version, preserving its operator and alias.
    /// Complex npm ranges are left unchanged rather than rewritten incorrectly.
    pub fn with_version(&self, version: &Version) -> Option<Self> {
        match self {
            #[cfg(feature = "cargo")]
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
                format!("{prefix}{value}").parse().ok().map(Self::Cargo)
            }
            #[cfg(feature = "npm")]
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
            #[cfg(feature = "cargo")]
            Self::Cargo(req) => req.fmt(f),
            #[cfg(feature = "npm")]
            Self::Npm { text, .. } => text.fmt(f),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "npm")]
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

    #[cfg(feature = "npm")]
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
