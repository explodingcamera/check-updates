use check_updates::PackageVersion;
use console::Style;
use semver::{Version, VersionReq};

use crate::cli::Args;

pub struct VersionStrategy {
    pub compatible: bool,
    pub pre: bool,
    // TODO: use this to filter versions by MSRV once Unit has rust-version
    // #[allow(dead_code)]
    // pub ignore_rust_version: bool,
}

impl VersionStrategy {
    pub fn from_args(args: &Args) -> Self {
        Self {
            compatible: args.compatible,
            pre: args.pre,
            // ignore_rust_version: args.ignore_rust_version,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VersionBump {
    Major,
    Minor,
    Patch,
}

/// Treats 0.0.x as patch instead of major, since 0.y.z versions often follow a different stability scheme.
pub fn version_bump(from: &Version, to: &Version) -> VersionBump {
    if !from.pre.is_empty() && from != to {
        return VersionBump::Major;
    }

    if from.major != to.major {
        return VersionBump::Major;
    }

    if from.major == 0 {
        if from.minor != to.minor {
            VersionBump::Major
        } else {
            VersionBump::Patch
        }
    } else if from.minor != to.minor {
        VersionBump::Minor
    } else {
        VersionBump::Patch
    }
}

/// Pick the best target version from published versions, filtered by strategy.
pub fn resolve_version(
    versions: &[PackageVersion],
    req: &VersionReq,
    strategy: &VersionStrategy,
    current: Option<&Version>,
) -> Option<Version> {
    versions
        .iter()
        .filter(|v| !v.yanked)
        .filter(|v| {
            if strategy.pre {
                return true;
            }

            if v.version.pre.is_empty() {
                return true;
            }

            current
                .filter(|c| !c.pre.is_empty())
                .is_some_and(|c| same_base(&v.version, c))
        })
        // TODO: once Unit has MSRV, use ignore_rust_version flag to control filtering
        .filter(|v| !strategy.compatible || req.matches(&v.version))
        .map(|v| &v.version)
        .max()
        .cloned()
}

fn same_base(a: &Version, b: &Version) -> bool {
    a.major == b.major && a.minor == b.minor && a.patch == b.patch
}

/// Extract the base version from a `VersionReq` (e.g. `^1.2.3` -> `1.2.3`).
pub fn current_version(req: &VersionReq) -> Option<Version> {
    let s = req.to_string();
    let stripped = s.trim_start_matches(|c: char| !c.is_ascii_digit());
    Version::parse(stripped).ok()
}

pub fn is_version_yanked(versions: &[PackageVersion], current: Option<&Version>) -> bool {
    let Some(current) = current else {
        return false;
    };
    versions
        .iter()
        .find(|v| v.version == *current)
        .is_some_and(|v| v.yanked)
}

/// Build a new `VersionReq` preserving the operator prefix and version shape.
pub fn build_new_req(old_req: &VersionReq, new_version: &Version) -> VersionReq {
    if old_req.comparators.len() != 1 {
        return old_req.clone();
    }

    let old_str = old_req.to_string();
    let digit_pos = old_str.find(|c: char| c.is_ascii_digit()).unwrap_or(0);
    let prefix = &old_str[..digit_pos];

    if !new_version.pre.is_empty() {
        return format!("{prefix}{new_version}")
            .parse()
            .expect("valid version req");
    }

    let version_part = &old_str[digit_pos..];

    // Count how many components the original had (major, major.minor, or major.minor.patch)
    let component_count = version_part.matches('.').count() + 1;

    let new_version_str = match component_count {
        1 => new_version.major.to_string(),
        2 => format!("{}.{}", new_version.major, new_version.minor),
        _ => format!(
            "{}.{}.{}",
            new_version.major, new_version.minor, new_version.patch
        ),
    };

    format!("{prefix}{new_version_str}")
        .parse()
        .expect("valid version req")
}

pub fn bump_style(bump: VersionBump) -> Style {
    match bump {
        VersionBump::Major => Style::new().red(),
        VersionBump::Minor => Style::new().cyan(),
        VersionBump::Patch => Style::new().green(),
    }
}

/// Color the changed part of the version requirement based on the bump level.
pub fn colorize_req(curr_req_str: &str, new_req_str: &str, bump: VersionBump) -> String {
    let color = bump_style(bump);

    // find first digit in new string (preserve prefix like ^, ~, >=, etc.)
    let ver_start = new_req_str.find(|c: char| c.is_ascii_digit()).unwrap_or(0);

    let prefix = &new_req_str[..ver_start];
    let new_ver_str = &new_req_str[ver_start..];
    let curr_ver_str = &curr_req_str[ver_start.min(curr_req_str.len())..];

    let parse = |s: &str| {
        let mut parts = s.split('.');
        (
            parts.next().and_then(|p| p.parse::<u64>().ok()),
            parts.next().and_then(|p| p.parse::<u64>().ok()),
            parts.next().and_then(|p| p.parse::<u64>().ok()),
        )
    };

    let (cmaj, cmin, cpat) = parse(curr_ver_str);
    let (nmaj, nmin, npat) = parse(new_ver_str);

    let highlight_from = if cmaj != nmaj {
        0
    } else if cmin != nmin {
        new_ver_str.find('.').map(|i| i + 1).unwrap_or(0)
    } else if cpat != npat {
        new_ver_str
            .match_indices('.')
            .nth(1)
            .map(|(i, _)| i + 1)
            .unwrap_or(0)
    } else {
        return new_req_str.to_string();
    };

    let (same, changed) = new_ver_str.split_at(highlight_from);

    format!("{prefix}{same}{}", color.apply_to(changed))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_version_bump_major() {
        let from = Version::parse("1.2.3").unwrap();
        let to = Version::parse("2.0.0").unwrap();
        assert_eq!(version_bump(&from, &to), VersionBump::Major);
    }

    #[test]
    fn test_version_bump_major_zero() {
        let from = Version::parse("0.1.0").unwrap();
        let to = Version::parse("0.2.0").unwrap();
        assert_eq!(version_bump(&from, &to), VersionBump::Major);
    }

    #[test]
    fn test_version_bump_minor() {
        let from = Version::parse("1.2.3").unwrap();
        let to = Version::parse("1.3.0").unwrap();
        assert_eq!(version_bump(&from, &to), VersionBump::Minor);
    }

    #[test]
    fn test_version_bump_patch() {
        let from = Version::parse("1.2.3").unwrap();
        let to = Version::parse("1.2.5").unwrap();
        assert_eq!(version_bump(&from, &to), VersionBump::Patch);

        let from = Version::parse("0.2.3").unwrap();
        let to = Version::parse("0.2.5").unwrap();
        assert_eq!(version_bump(&from, &to), VersionBump::Patch);
    }

    #[test]
    fn test_current_version_caret() {
        let req: VersionReq = "^1.2.3".parse().unwrap();
        assert_eq!(
            current_version(&req),
            Some(Version::parse("1.2.3").unwrap())
        );
    }

    #[test]
    fn test_current_version_tilde() {
        let req: VersionReq = "~0.4.0".parse().unwrap();
        assert_eq!(
            current_version(&req),
            Some(Version::parse("0.4.0").unwrap())
        );
    }

    #[test]
    fn test_current_version_gte() {
        let req: VersionReq = ">=1.0.0".parse().unwrap();
        assert_eq!(
            current_version(&req),
            Some(Version::parse("1.0.0").unwrap())
        );
    }

    #[test]
    fn test_build_new_req_caret() {
        let old: VersionReq = "^1.2.3".parse().unwrap();
        let new_ver = Version::parse("2.0.0").unwrap();
        let result = build_new_req(&old, &new_ver);
        assert_eq!(result.to_string(), "^2.0.0");
    }

    #[test]
    fn test_build_new_req_tilde() {
        let old: VersionReq = "~1.2.3".parse().unwrap();
        let new_ver = Version::parse("1.3.0").unwrap();
        let result = build_new_req(&old, &new_ver);
        assert_eq!(result.to_string(), "~1.3.0");
    }

    #[test]
    fn test_build_new_req_bare_major() {
        let old: VersionReq = "1".parse().unwrap();
        let new_ver = Version::parse("2.3.4").unwrap();
        let result = build_new_req(&old, &new_ver);
        assert_eq!(result.to_string(), "^2");
    }

    #[test]
    fn test_build_new_req_bare_major_minor() {
        let old: VersionReq = "1.2".parse().unwrap();
        let new_ver = Version::parse("2.3.4").unwrap();
        let result = build_new_req(&old, &new_ver);
        assert_eq!(result.to_string(), "^2.3");
    }

    #[test]
    fn test_is_version_yanked_true() {
        let versions = vec![PackageVersion {
            version: Version::parse("1.2.3").unwrap(),
            yanked: true,
            features: Default::default(),
            rust_version: None,
        }];
        let current = Version::parse("1.2.3").unwrap();
        assert!(is_version_yanked(&versions, Some(&current)));
    }

    #[test]
    fn test_is_version_yanked_false() {
        let versions = vec![PackageVersion {
            version: Version::parse("1.2.3").unwrap(),
            yanked: false,
            features: Default::default(),
            rust_version: None,
        }];
        let current = Version::parse("1.2.3").unwrap();
        assert!(!is_version_yanked(&versions, Some(&current)));
    }

    #[test]
    fn test_resolve_version_latest() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("3.0.0-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
        ];
        let req: VersionReq = "^1.0.0".parse().unwrap();
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
        };
        assert_eq!(
            resolve_version(&versions, &req, &strategy, None),
            Some(Version::parse("2.0.0").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_compatible() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("1.5.0").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
        ];
        let req: VersionReq = "^1.0.0".parse().unwrap();
        let strategy = VersionStrategy {
            compatible: true,
            pre: false,
        };
        assert_eq!(
            resolve_version(&versions, &req, &strategy, None),
            Some(Version::parse("1.5.0").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_skips_yanked() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0").unwrap(),
                yanked: true,
                features: Default::default(),
                rust_version: None,
            },
        ];
        let req: VersionReq = "^1.0.0".parse().unwrap();
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
        };
        assert_eq!(
            resolve_version(&versions, &req, &strategy, None),
            Some(Version::parse("1.0.0").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_with_pre() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
        ];
        let req: VersionReq = "^1.0.0".parse().unwrap();
        let strategy = VersionStrategy {
            compatible: false,
            pre: true,
        };
        assert_eq!(
            resolve_version(&versions, &req, &strategy, None),
            Some(Version::parse("2.0.0-alpha.1").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_current_prerelease_without_pre_flag() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("1.0.0-alpha.2").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
            PackageVersion {
                version: Version::parse("1.0.1-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                rust_version: None,
            },
        ];
        let req: VersionReq = "^1.0.0-alpha.1".parse().unwrap();
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
        };

        assert_eq!(
            resolve_version(
                &versions,
                &req,
                &strategy,
                Some(&Version::parse("1.0.0-alpha.1").unwrap())
            ),
            Some(Version::parse("1.0.0-alpha.2").unwrap())
        );
    }

    #[test]
    fn test_build_new_req_keeps_full_prerelease() {
        let old: VersionReq = "^1.0.0-alpha.1".parse().unwrap();
        let new_ver = Version::parse("1.0.0-beta.2").unwrap();
        let result = build_new_req(&old, &new_ver);
        assert_eq!(result.to_string(), "^1.0.0-beta.2");

        let old: VersionReq = "=1.0.0-alpha.1".parse().unwrap();
        let result = build_new_req(&old, &new_ver);
        assert_eq!(result.to_string(), "=1.0.0-beta.2");
    }

    #[test]
    fn test_version_bump_prerelease_is_breaking() {
        let from = Version::parse("1.0.0-alpha.1").unwrap();
        let to = Version::parse("1.0.0-beta.1").unwrap();
        assert_eq!(version_bump(&from, &to), VersionBump::Major);

        let to = Version::parse("1.0.0").unwrap();
        assert_eq!(version_bump(&from, &to), VersionBump::Major);
    }

    #[test]
    fn test_caret_one_req() {
        let req: VersionReq = "^1".parse().unwrap();
        assert_eq!(req.to_string(), "^1");
        assert!(req.matches(&Version::parse("1.3.1").unwrap()));
        assert!(!req.matches(&Version::parse("0.2.5").unwrap()));
    }

    #[test]
    fn test_bare_version_req() {
        // Bare "1" gets normalized to "^1" by semver crate
        let req: VersionReq = "1".parse().unwrap();
        assert_eq!(req.to_string(), "^1");

        // Bare "1.2" gets normalized to "^1.2"
        let req: VersionReq = "1.2".parse().unwrap();
        assert_eq!(req.to_string(), "^1.2");
    }

    #[test]
    fn test_build_new_req_complex_chain_is_unchanged() {
        let old: VersionReq = ">=1.0, <2.0".parse().unwrap();
        let new_ver = Version::parse("3.4.5").unwrap();
        let result = build_new_req(&old, &new_ver);
        assert_eq!(result.to_string(), old.to_string());
    }
}
