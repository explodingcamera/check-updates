use check_updates::Version;
use console::Style;

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
    use check_updates::{PackageVersion, Requirement, VersionStrategy};

    fn cargo_requirement(text: &str) -> Requirement {
        Requirement::from_cargo(text.parse::<semver::VersionReq>().unwrap())
    }

    fn package_version(version: &str, required_toolchain_version: Option<&str>) -> PackageVersion {
        PackageVersion {
            version: Version::parse(version).unwrap(),
            yanked: false,
            features: Default::default(),
            required_toolchain_version: required_toolchain_version
                .map(|version| Version::parse(version).unwrap()),
        }
    }

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
        let req = cargo_requirement("^1.2.3");
        assert_eq!(
            req.current_version(),
            Some(Version::parse("1.2.3").unwrap())
        );
    }

    #[test]
    fn test_current_version_tilde() {
        let req = cargo_requirement("~0.4.0");
        assert_eq!(
            req.current_version(),
            Some(Version::parse("0.4.0").unwrap())
        );
    }

    #[test]
    fn test_current_version_gte() {
        let req = cargo_requirement(">=1.0.0");
        assert_eq!(
            req.current_version(),
            Some(Version::parse("1.0.0").unwrap())
        );
    }

    #[test]
    fn test_build_new_req_caret() {
        let old = cargo_requirement("^1.2.3");
        let new_ver = Version::parse("2.0.0").unwrap();
        let result = old.with_version(&new_ver).unwrap();
        assert_eq!(result.to_string(), "^2.0.0");
    }

    #[test]
    fn test_build_new_req_tilde() {
        let old = cargo_requirement("~1.2.3");
        let new_ver = Version::parse("1.3.0").unwrap();
        let result = old.with_version(&new_ver).unwrap();
        assert_eq!(result.to_string(), "~1.3.0");
    }

    #[test]
    fn test_build_new_req_bare_major() {
        let old = cargo_requirement("1");
        let new_ver = Version::parse("2.3.4").unwrap();
        let result = old.with_version(&new_ver).unwrap();
        assert_eq!(result.to_string(), "^2");
    }

    #[test]
    fn test_build_new_req_bare_major_minor() {
        let old = cargo_requirement("1.2");
        let new_ver = Version::parse("2.3.4").unwrap();
        let result = old.with_version(&new_ver).unwrap();
        assert_eq!(result.to_string(), "^2.3");
    }

    #[test]
    fn test_resolve_version_latest() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("3.0.0-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
        ];
        let req = cargo_requirement("^1.0.0");
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
            ignore_toolchain_version: false,
        };
        assert_eq!(
            strategy.select(&versions, &req, None, None),
            Some(&Version::parse("2.0.0").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_compatible() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("1.5.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
        ];
        let req = cargo_requirement("^1.0.0");
        let strategy = VersionStrategy {
            compatible: true,
            pre: false,
            ignore_toolchain_version: false,
        };
        assert_eq!(
            strategy.select(&versions, &req, None, None),
            Some(&Version::parse("1.5.0").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_skips_yanked() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0").unwrap(),
                yanked: true,
                features: Default::default(),
                required_toolchain_version: None,
            },
        ];
        let req = cargo_requirement("^1.0.0");
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
            ignore_toolchain_version: false,
        };
        assert_eq!(
            strategy.select(&versions, &req, None, None),
            Some(&Version::parse("1.0.0").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_with_pre() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("2.0.0-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
        ];
        let req = cargo_requirement("^1.0.0");
        let strategy = VersionStrategy {
            compatible: false,
            pre: true,
            ignore_toolchain_version: false,
        };
        assert_eq!(
            strategy.select(&versions, &req, None, None),
            Some(&Version::parse("2.0.0-alpha.1").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_current_prerelease_without_pre_flag() {
        let versions = vec![
            PackageVersion {
                version: Version::parse("1.0.0-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("1.0.0-alpha.2").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
            PackageVersion {
                version: Version::parse("1.0.1-alpha.1").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            },
        ];
        let req = cargo_requirement("^1.0.0-alpha.1");
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
            ignore_toolchain_version: false,
        };

        assert_eq!(
            strategy.select(
                &versions,
                &req,
                Some(&Version::parse("1.0.0-alpha.1").unwrap()),
                None,
            ),
            Some(&Version::parse("1.0.0-alpha.2").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_filters_by_toolchain_version() {
        let versions = vec![
            package_version("1.0.0", None),
            package_version("2.0.0", Some("1.70.0")),
            package_version("3.0.0", Some("1.80.0")),
        ];
        let req = cargo_requirement("^1.0.0");
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
            ignore_toolchain_version: false,
        };

        assert_eq!(
            strategy.select(
                &versions,
                &req,
                None,
                Some(&Version::parse("1.70.0").unwrap()),
            ),
            Some(&Version::parse("2.0.0").unwrap())
        );
    }

    #[test]
    fn test_resolve_version_can_ignore_toolchain_version() {
        let versions = vec![
            package_version("1.0.0", None),
            package_version("2.0.0", Some("1.70.0")),
            package_version("3.0.0", Some("1.80.0")),
        ];
        let req = cargo_requirement("^1.0.0");
        let strategy = VersionStrategy {
            compatible: false,
            pre: false,
            ignore_toolchain_version: true,
        };

        assert_eq!(
            strategy.select(
                &versions,
                &req,
                None,
                Some(&Version::parse("1.70.0").unwrap()),
            ),
            Some(&Version::parse("3.0.0").unwrap())
        );
    }

    #[test]
    fn test_build_new_req_keeps_full_prerelease() {
        let old = cargo_requirement("^1.0.0-alpha.1");
        let new_ver = Version::parse("1.0.0-beta.2").unwrap();
        let result = old.with_version(&new_ver).unwrap();
        assert_eq!(result.to_string(), "^1.0.0-beta.2");

        let old = cargo_requirement("=1.0.0-alpha.1");
        let result = old.with_version(&new_ver).unwrap();
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
        let req = cargo_requirement("^1");
        assert_eq!(req.to_string(), "^1");
        assert!(req.matches(&Version::parse("1.3.1").unwrap()));
        assert!(!req.matches(&Version::parse("0.2.5").unwrap()));
    }

    #[test]
    fn test_bare_version_req() {
        // Bare "1" gets normalized to "^1" by semver crate
        let req = cargo_requirement("1");
        assert_eq!(req.to_string(), "^1");

        // Bare "1.2" gets normalized to "^1.2"
        let req = cargo_requirement("1.2");
        assert_eq!(req.to_string(), "^1.2");
    }

    #[test]
    fn test_build_new_req_complex_chain_is_unchanged() {
        let old = cargo_requirement(">=1.0, <2.0");
        let new_ver = Version::parse("3.4.5").unwrap();
        let result = old.with_version(&new_ver).unwrap();
        assert_eq!(result.to_string(), old.to_string());
    }
}
