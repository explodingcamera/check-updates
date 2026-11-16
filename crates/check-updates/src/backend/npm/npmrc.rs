use std::collections::BTreeMap;
use std::path::Path;

use crate::config::{ConfigEnv, read_optional};

/// Parsed npmrc values, retaining repeated array entries.
pub(super) type NpmrcValues = BTreeMap<String, Vec<String>>;

/// Parse comma-separated exclusions used in npmrc and environment overrides.
pub(super) fn split_excludes(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(String::from)
        .collect()
}

/// Read the last value assigned to an npmrc key.
pub(super) fn last<'a>(values: &'a NpmrcValues, key: &str) -> Option<&'a str> {
    values
        .get(key)
        .and_then(|values| values.last())
        .map(String::as_str)
}

/// Read relevant npmrc values without interpreting package-manager policy.
pub(super) fn read_npmrc(path: &Path, env: &ConfigEnv) -> NpmrcValues {
    let mut values = NpmrcValues::new();
    let Some(text) = read_optional(path).unwrap_or_else(|error| {
        log::warn!("ignoring unreadable npmrc: {error}");
        None
    }) else {
        return values;
    };
    let mut section = false;
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            section = true;
        }
        if section || line.starts_with(['#', ';']) {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let (key, array) = key
            .strip_suffix("[]")
            .map(|key| (key, true))
            .unwrap_or((key, false));
        // Never expand or report unrelated credentials or registry settings.
        if !matches!(
            key,
            "min-release-age"
                | "min-release-age-exclude"
                | "minimum-release-age"
                | "minimum-release-age-exclude"
                | "minimum-release-age-ignore-missing-time"
                | "userconfig"
                | "globalconfig"
                | "prefix"
        ) {
            continue;
        }
        let mut quote = None;
        let end = value
            .char_indices()
            .find_map(|(index, ch)| {
                if matches!(ch, '\'' | '"') {
                    if quote == Some(ch) {
                        quote = None;
                    } else if quote.is_none() {
                        quote = Some(ch);
                    }
                }
                (quote.is_none() && matches!(ch, '#' | ';')).then_some(index)
            })
            .unwrap_or(value.len());
        let value = value[..end].trim();
        let value = if value.len() >= 2
            && ((value.starts_with('"') && value.ends_with('"'))
                || (value.starts_with('\'') && value.ends_with('\'')))
        {
            &value[1..value.len() - 1]
        } else {
            value
        };
        let value = match env.expand(value) {
            Ok(value) => value,
            Err(error) => {
                log::warn!("{}: ignoring invalid {key}: {error}", path.display());
                continue;
            }
        };
        if array {
            values.entry(key.into()).or_default().push(value);
        } else {
            values.insert(key.into(), vec![value]);
        }
    }
    values
}
