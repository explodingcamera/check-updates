use jiff::{Span, Timestamp, tz::TimeZone};

/// Parse an age with an explicit unit, or zero to disable the filter.
pub(crate) fn parse_span(value: &str) -> Result<Span, String> {
    let value = value.trim();
    if value == "0" {
        return Ok(Span::new());
    }
    let span: Span = value.parse().map_err(|error| {
        format!("expected a publish-age span, such as 7d or '12 hours': {error}")
    })?;
    if span.is_negative() {
        return Err("publish age must be non-negative".into());
    }
    Ok(span)
}

/// A resolved, per-run publication cutoff and package exclusions.
#[derive(Clone, Debug, Default)]
pub(crate) struct PublishAge {
    cutoff: Option<Timestamp>,
    /// Package names or exact-version selectors exempt from the cutoff.
    pub(crate) excludes: Vec<String>,
    /// Allow versions whose publication timestamp is absent or invalid.
    pub(crate) ignore_missing_time: bool,
    /// Interpret exclusion names as globs, as npm and pnpm do.
    pub(crate) patterns: bool,
}

impl PublishAge {
    /// Capture a cutoff once, rather than recomputing the age for each version.
    pub(crate) fn try_new(age: Option<Span>, now: Timestamp) -> Result<Self, String> {
        let cutoff = age
            .filter(|age| !age.is_zero())
            .map(|age| {
                if age.is_negative() {
                    return Err("publish age must be non-negative".into());
                }
                now.to_zoned_datetime(TimeZone::UTC)
                    .checked_sub(age)
                    .map(|cutoff| cutoff.timestamp())
                    .map_err(|error| format!("invalid publish age: {error}"))
            })
            .transpose()?;
        Ok(Self {
            cutoff,
            ignore_missing_time: true,
            ..Self::default()
        })
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
            Some(time) => time <= cutoff,
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
            ("1d 12h", 129600),
            ("1.5h", 5400),
        ] {
            assert_eq!(
                parse_span(input)
                    .unwrap()
                    .to_duration(jiff::SpanRelativeTo::days_are_24_hours())
                    .unwrap()
                    .as_secs(),
                seconds
            );
        }
        for input in ["", "7", "-1d", "1.5d", "NaN", "18446744073709551615w"] {
            assert!(parse_span(input).is_err(), "{input}");
        }
    }

    #[test]
    fn filters_at_a_fixed_cutoff() {
        let now = "2026-01-01T00:00:00Z".parse().unwrap();
        let mut policy = PublishAge::try_new(Some(Span::new().days(1)), now).unwrap();
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
        assert!(
            PublishAge::try_new(Some(Span::new()), now)
                .unwrap()
                .allows("pkg", "3.0.0", None)
        );
        let now = "2024-03-01T00:00:00Z".parse().unwrap();
        let policy = PublishAge::try_new(Some(Span::new().months(1)), now).unwrap();
        assert!(policy.allows("pkg", "1.0.0", Some("2024-02-01T00:00:00Z")));
        assert!(!policy.allows("pkg", "1.0.0", Some("2024-02-01T00:00:00.001Z")));
        assert!(PublishAge::try_new(Some(Span::new().days(-1)), now).is_err());
    }
}
