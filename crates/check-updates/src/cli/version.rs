use crate::Version;
use jiff::SignedDuration;
use owo_colors::{OwoColorize, Stream, Style};

/// Format an age warning for a release published less than three days ago.
pub(crate) fn format_publish_age(age: SignedDuration, stream: Stream) -> Option<String> {
    let seconds = age.as_secs().max(0);
    if seconds >= 3 * 86400 {
        return None;
    }
    let (whole, remainder, unit, subunit) = if seconds >= 86400 {
        (seconds / 86400, seconds % 86400 / 3600, "d", "h")
    } else if seconds >= 3600 {
        (seconds / 3600, seconds % 3600 / 60, "h", "m")
    } else {
        (seconds / 60, 0, "m", "")
    };
    let badge = if whole == 0 {
        "[<1m]".to_string()
    } else if remainder == 0 {
        format!("[{whole}{unit}]")
    } else {
        format!("[{whole}{unit}{remainder}{subunit}]")
    };
    let style = if seconds < 86400 {
        Style::new().red()
    } else {
        Style::new().yellow()
    };
    Some(
        badge
            .if_supports_color(stream, |text| text.style(style))
            .to_string(),
    )
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

pub fn bump_style(bump: VersionBump) -> Style {
    match bump {
        VersionBump::Major => Style::new().red(),
        VersionBump::Minor => Style::new().cyan(),
        VersionBump::Patch => Style::new().green(),
    }
}

/// Color the changed part of the version requirement based on the bump level.
pub fn colorize_req(
    curr_req_str: &str,
    new_req_str: &str,
    bump: VersionBump,
    stream: Stream,
) -> String {
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

    format!(
        "{prefix}{same}{}",
        changed.if_supports_color(stream, |text| text.style(color))
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_recent_release_ages() {
        for (seconds, expected) in [
            (0, Some("[<1m]")),
            (60, Some("[1m]")),
            (3600, Some("[1h]")),
            (86399, Some("[23h59m]")),
            (86400, Some("[1d]")),
            (29 * 3600, Some("[1d5h]")),
            (3 * 86400 - 1, Some("[2d23h]")),
            (3 * 86400, None),
        ] {
            let badge = format_publish_age(SignedDuration::from_secs(seconds), Stream::Stdout);
            match expected {
                Some(expected) => assert!(badge.unwrap().contains(expected)),
                None => assert!(badge.is_none()),
            }
        }
    }

    #[test]
    fn classifies_version_bumps() {
        for (from, to, expected) in [
            ("1.2.3", "2.0.0", VersionBump::Major),
            ("0.1.0", "0.2.0", VersionBump::Major),
            ("1.2.3", "1.3.0", VersionBump::Minor),
            ("1.2.3", "1.2.5", VersionBump::Patch),
            ("0.2.3", "0.2.5", VersionBump::Patch),
            ("0.0.1", "0.0.2", VersionBump::Patch),
            ("1.0.0-alpha.1", "1.0.0-beta.1", VersionBump::Major),
            ("1.0.0-alpha.1", "1.0.0", VersionBump::Major),
        ] {
            assert_eq!(
                version_bump(&Version::parse(from).unwrap(), &Version::parse(to).unwrap()),
                expected,
                "{from} -> {to}"
            );
        }
    }
}
