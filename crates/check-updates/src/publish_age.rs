use std::time::Duration;

use jiff::Timestamp;

/// Parse an age with an explicit unit, or zero to disable the filter.
pub(crate) fn parse_duration(value: &str) -> Result<Duration, String> {
    let value = value.trim();
    if value == "0" {
        return Ok(Duration::ZERO);
    }
    let split = value
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(value.len());
    let number = value[..split]
        .parse::<u64>()
        .map_err(|_| "expected a non-negative duration, such as 7d".to_string())?;
    let unit = value[split..].trim();
    let seconds = match unit {
        "s" | "second" | "seconds" => 1,
        "m" | "minute" | "minutes" => 60,
        "h" | "hour" | "hours" => 3600,
        "d" | "day" | "days" => 86400,
        "w" | "week" | "weeks" => 7 * 86400,
        "month" | "months" => 30 * 86400,
        _ => return Err("expected a duration with a unit, such as 7d or '12 hours'".into()),
    };
    number
        .checked_mul(seconds)
        .map(Duration::from_secs)
        .ok_or_else(|| "publish age is too large".into())
}

/// Convert a package manager's numeric age to seconds without accepting negatives or overflow.
pub(crate) fn numeric_duration(value: &str, unit: u64) -> Result<Duration, String> {
    let value = value
        .parse::<f64>()
        .map_err(|_| "expected a non-negative publish age".to_string())?;
    Duration::try_from_secs_f64(value * unit as f64)
        .map_err(|_| "expected a finite, non-negative publish age".into())
}

/// A resolved, per-run publication cutoff and package exclusions.
#[derive(Clone, Debug, Default)]
pub(crate) struct PublishAge {
    cutoff: Option<i128>,
    /// Package names or exact-version selectors exempt from the cutoff.
    pub(crate) excludes: Vec<String>,
    /// Allow versions whose publication timestamp is absent or invalid.
    pub(crate) ignore_missing_time: bool,
    /// Interpret exclusion names as globs, as npm and pnpm do.
    pub(crate) patterns: bool,
}

impl PublishAge {
    /// Capture a cutoff once, rather than recomputing the age for each version.
    pub(crate) fn new(age: Option<Duration>) -> Self {
        Self::at(age, Timestamp::now())
    }

    fn at(age: Option<Duration>, now: Timestamp) -> Self {
        Self {
            cutoff: age
                .filter(|age| !age.is_zero())
                .map(|age| now.as_nanosecond() - age.as_nanos() as i128),
            ignore_missing_time: true,
            ..Self::default()
        }
    }

    /// Whether the age filter is enabled.
    #[cfg(test)]
    pub(crate) fn enabled(&self) -> bool {
        self.cutoff.is_some()
    }

    /// Check a timestamp after applying package and exact-version exemptions.
    pub(crate) fn allows(&self, name: &str, version: &str, published: Option<&str>) -> bool {
        let Some(cutoff) = self.cutoff else {
            return true;
        };
        if self.excludes.iter().any(|entry| {
            let (pattern, versions) = match entry.rsplit_once('@') {
                Some((name, versions)) if !name.is_empty() => (name, Some(versions)),
                _ => (entry.as_str(), None),
            };
            let matches = pattern == name
                || (self.patterns && glob::Pattern::new(pattern).is_ok_and(|p| p.matches(name)));
            matches
                && versions.is_none_or(|versions| versions.split("||").any(|v| v.trim() == version))
        }) {
            return true;
        }
        match published.and_then(|value| value.parse::<jiff::Timestamp>().ok()) {
            Some(time) => time.as_nanosecond() <= cutoff,
            None => self.ignore_missing_time,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_durations() {
        for (input, seconds) in [
            ("0", 0),
            ("7d", 604800),
            ("12 hours", 43200),
            ("90m", 5400),
            ("1 month", 2592000),
        ] {
            assert_eq!(parse_duration(input).unwrap().as_secs(), seconds);
        }
        for input in ["", "7", "-1d", "1.5d", "NaN", "18446744073709551615w"] {
            assert!(parse_duration(input).is_err(), "{input}");
        }
        for input in ["-1", "NaN", "inf", "1e100"] {
            assert!(numeric_duration(input, 60).is_err(), "{input}");
        }
    }

    #[test]
    fn filters_at_a_fixed_cutoff() {
        let now = "2026-01-01T00:00:00Z".parse().unwrap();
        let mut policy = PublishAge::at(Some(Duration::from_secs(86400)), now);
        assert!(policy.allows("pkg", "1.0.0", Some("2025-12-31T00:00:00Z")));
        assert!(!policy.allows("pkg", "1.0.0", Some("2025-12-31T00:00:00.001Z")));
        assert!(!policy.allows("pkg", "1.0.0", Some("2027-01-01T00:00:00Z")));
        assert!(policy.allows("pkg", "1.0.0", None));
        policy.ignore_missing_time = false;
        assert!(!policy.allows("pkg", "1.0.0", Some("invalid")));
        policy.excludes = vec!["@org/*".into(), "pkg@1.0.0 || 2.0.0".into()];
        policy.patterns = true;
        assert!(policy.allows("@org/pkg", "3.0.0", None));
        assert!(policy.allows("pkg", "2.0.0", None));
        assert!(!policy.allows("pkg", "3.0.0", None));
        assert!(PublishAge::new(Some(Duration::ZERO)).allows("pkg", "3.0.0", None));
    }
}
