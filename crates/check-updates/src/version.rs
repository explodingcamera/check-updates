use crate::Version;
use owo_colors::{OwoColorize, Stream, Style};

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
