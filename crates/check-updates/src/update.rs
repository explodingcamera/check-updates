use std::collections::BTreeMap;
use std::fmt::Write;

use crate::{DepKind, Package, Packages, Requirement, Unit, Usage, VersionStrategy};
use owo_colors::{OwoColorize, Stream};

use crate::interactive::{UpdateGroup, display_text};
use crate::version::{VersionBump, colorize_req, version_bump};

pub struct Update<'a> {
    pub name: &'a str,
    pub current_req: &'a Requirement,
    pub new_req: Requirement,
    pub bump: VersionBump,
    pub yanked: bool,
    pub usage: &'a Usage,
    pub package: &'a Package,
}

/// Dependency updates grouped by project.
pub type Updates<'a> = BTreeMap<&'a Unit, Vec<Update<'a>>>;

fn display_name(update: &Update<'_>) -> String {
    let name = match update.usage.rename.as_deref() {
        Some(alias) if alias != update.name => {
            format!("{} ({})", display_text(update.name), display_text(alias))
        }
        _ => display_text(update.name),
    };
    name + kind_label(update)
}

/// Marks non-normal dependencies, e.g. ` (dev)`, so entries from different sections are distinguishable.
fn kind_label(update: &Update<'_>) -> &'static str {
    match update.usage.kind {
        DepKind::Normal => "",
        DepKind::Dev => " (dev)",
        DepKind::Build => " (build)",
        DepKind::Optional => " (optional)",
        DepKind::Peer => " (peer)",
    }
}

fn display_requirement(update: &Update<'_>, requirement: &Requirement) -> String {
    let text = requirement.to_string();
    if update.package.purl.package_type() == "npm" {
        display_text(&text)
    } else {
        text
    }
}

pub fn resolve_updates<'a>(
    packages: &'a Packages,
    strategy: &VersionStrategy,
    include_breaking: bool,
) -> Updates<'a> {
    let mut result = Updates::new();

    for (unit, entries) in packages {
        for (req, kind, package) in entries {
            let name = package.purl.name();

            let current = req.current_version();
            let bump_for = |candidate| {
                let bump = current
                    .as_ref()
                    .map(|current| version_bump(current, candidate))
                    .unwrap_or(VersionBump::Major);
                // Existing ranges can already allow a version classified as major.
                if bump == VersionBump::Major
                    && current
                        .as_ref()
                        .is_none_or(|current| current.pre.is_empty())
                    && req.matches(candidate)
                {
                    VersionBump::Patch
                } else {
                    bump
                }
            };
            let Some(usage) = package
                .usages
                .iter()
                .find(|u| u.unit == *unit && u.req == *req && u.kind == *kind)
            else {
                continue;
            };
            let supported_toolchain_version = package
                .usages
                .iter()
                .filter(|u| u.unit == *unit && u.req == *req)
                .filter_map(|u| u.supported_toolchain_version.as_ref())
                .min();
            let latest_for = |strategy: &VersionStrategy, supported| {
                strategy.select(
                    package.versions.iter().filter(|candidate| {
                        current
                            .as_ref()
                            .is_none_or(|current| candidate.version >= *current)
                            && (include_breaking
                                || bump_for(&candidate.version) != VersionBump::Major)
                    }),
                    req,
                    current.as_ref(),
                    supported,
                )
            };
            let latest = latest_for(strategy, supported_toolchain_version);

            if let Some(supported_toolchain_version) = supported_toolchain_version
                && !strategy.ignore_toolchain_version
            {
                let ignored_strategy = VersionStrategy {
                    ignore_toolchain_version: true,
                    ..*strategy
                };
                let unrestricted = latest_for(&ignored_strategy, None);

                if unrestricted > latest {
                    log::info!(
                        "skipping newer {name} versions requiring a newer toolchain than {}",
                        supported_toolchain_version
                    );
                }
            }

            let Some(latest) = latest else {
                continue;
            };
            let Some(new_req) = req.with_version(latest) else {
                continue;
            };

            // Skip if the requirement doesn't need to change
            if new_req == *req {
                continue;
            }

            result.entry(unit).or_default().push(Update {
                name,
                current_req: req,
                new_req,
                bump: bump_for(latest),
                yanked: current
                    .as_ref()
                    .is_some_and(|version| package.is_version_yanked(version)),
                usage,
                package,
            });
        }
    }

    for unit_updates in result.values_mut() {
        unit_updates.sort_unstable_by_key(|u| (kind_label(u), u.name));
    }

    result
}

pub(crate) fn unit_matches_filter(unit: &Unit, filter: &str) -> bool {
    match unit {
        Unit::Project { name, .. } => filter == name,
        Unit::Workspace { manifest } => {
            filter == "workspace"
                || manifest
                    .parent()
                    .and_then(|path| path.file_name())
                    .is_some_and(|name| name == filter)
        }
        Unit::Global => filter == "global",
    }
}

/// Format a single update as an aligned, colored line.
pub fn format_update_line(
    update: &Update,
    name_width: usize,
    cur_width: usize,
    new_width: usize,
    stream: Stream,
) -> String {
    let mut line = String::new();

    let plain_display_name = display_name(update);
    let label = kind_label(update);
    let link_text = plain_display_name
        .strip_suffix(label)
        .unwrap_or(&plain_display_name);
    let name_display = update
        .package
        .repository
        .as_ref()
        .map(|url| normalize_repo_url(url))
        .filter(|url| !url.chars().any(char::is_control))
        .map(|url| hyperlink(&url, link_text))
        .unwrap_or_else(|| link_text.to_string());
    let name_display = format!(
        "{name_display}{}",
        label.if_supports_color(stream, |text| text.dimmed())
    );

    // Pad manually since hyperlink escape codes don't count as visible width
    let padding = name_width.saturating_sub(plain_display_name.len());
    let _ = write!(line, " {}{:>padding$}", name_display, "");

    let cur_req_str = display_requirement(update, update.current_req);
    let cur_display_len = cur_req_str.len() + if update.yanked { 9 } else { 0 };
    let cur_padding = cur_width.saturating_sub(cur_display_len);

    let _ = write!(
        line,
        "  {:>cur_padding$}{}",
        "",
        cur_req_str.if_supports_color(stream, |text| text.white())
    );
    if update.yanked {
        let _ = write!(
            line,
            " {}",
            "(yanked)".if_supports_color(stream, |text| text.yellow())
        );
    }

    let new_req_str = display_requirement(update, &update.new_req);
    let colorized = colorize_req(&cur_req_str, &new_req_str, update.bump, stream);

    let new_padding = new_width.saturating_sub(new_req_str.len());
    let _ = write!(line, "  →  {:>new_padding$}{}", "", colorized);

    line
}

pub fn summary_groups(updates: &Updates<'_>, mixed: bool) -> Vec<UpdateGroup> {
    updates
        .iter()
        .map(|(unit, unit_updates)| {
            let name_w = unit_updates
                .iter()
                .map(|u| display_name(u).len())
                .max()
                .unwrap_or(0);
            let cur_w = unit_updates
                .iter()
                .map(|u| display_requirement(u, u.current_req).len() + if u.yanked { 9 } else { 0 })
                .max()
                .unwrap_or(0);
            let new_w = unit_updates
                .iter()
                .map(|u| display_requirement(u, &u.new_req).len())
                .max()
                .unwrap_or(0);

            UpdateGroup {
                name: group_name(unit, mixed),
                updates: unit_updates
                    .iter()
                    .map(|update| format_update_line(update, name_w, cur_w, new_w, Stream::Stdout))
                    .collect(),
            }
        })
        .collect()
}

fn group_name(unit: &Unit, mixed: bool) -> String {
    let npm = unit
        .path()
        .is_some_and(|path| path.file_name().is_some_and(|name| name == "package.json"));
    let name = display_text(&unit.name());
    if !mixed {
        return name;
    }
    let backend = if npm { "npm" } else { "cargo" };
    match unit {
        Unit::Workspace { .. } => format!(
            "{} ({backend} workspace)",
            name.trim_end_matches(" (workspace)")
        ),
        _ => format!("{name} ({backend})"),
    }
}

pub fn selection_groups(updates: &Updates<'_>, mixed: bool) -> (Vec<UpdateGroup>, Vec<bool>) {
    let (name_w, cur_w, new_w) = updates.values().flatten().fold((0, 0, 0), |widths, u| {
        (
            widths.0.max(display_name(u).len()),
            widths
                .1
                .max(display_requirement(u, u.current_req).len() + if u.yanked { 9 } else { 0 }),
            widths.2.max(display_requirement(u, &u.new_req).len()),
        )
    });
    let target = (console::Term::stderr().size().1 as usize).min(40);
    let name_w = name_w.max(target.saturating_sub(cur_w + new_w + 4));
    let default_selected = updates
        .values()
        .flatten()
        .map(|update| update.bump != VersionBump::Major)
        .collect();
    let groups = updates
        .iter()
        .map(|(unit, entries)| UpdateGroup {
            name: group_name(unit, mixed),
            updates: entries
                .iter()
                .map(|u| format_update_line(u, name_w, cur_w, new_w, Stream::Stderr))
                .collect(),
        })
        .collect();
    (groups, default_selected)
}

fn hyperlink(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

fn normalize_repo_url(url: &str) -> String {
    let url = url.trim();

    // Strip .git suffix
    let url = url.strip_suffix(".git").unwrap_or(url);

    // Convert git@ to https://
    if let Some(rest) = url.strip_prefix("git@")
        // git@github.com:user/repo -> https://github.com/user/repo
        && let Some((host, path)) = rest.split_once(':')
    {
        return format!("https://{host}/{path}");
    }

    // Convert git:// to https://
    if let Some(rest) = url.strip_prefix("git://") {
        return format!("https://{rest}");
    }

    // Ensure https://
    if let Some(rest) = url.strip_prefix("http://") {
        return format!("https://{rest}");
    }

    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PackageVersion, Version};

    fn resolved_update(
        current: &str,
        versions: &[&str],
        include_breaking: bool,
    ) -> Option<(String, VersionBump)> {
        let unit = Unit::Global;
        let req = Requirement::from_cargo(current.parse().unwrap());
        let package = Package {
            purl: "pkg:cargo/example".parse().unwrap(),
            usages: vec![Usage {
                unit: unit.clone(),
                req: req.clone(),
                kind: DepKind::Normal,
                rename: None,
                supported_toolchain_version: None,
            }],
            versions: versions
                .iter()
                .map(|version| PackageVersion {
                    version: Version::parse(version).unwrap(),
                    yanked: false,
                    features: Default::default(),
                    required_toolchain_version: None,
                })
                .collect(),
            repository: None,
            homepage: None,
        };
        let packages = Packages::from([(unit, vec![(req, DepKind::Normal, package)])]);
        resolve_updates(&packages, &VersionStrategy::default(), include_breaking)
            .values()
            .flatten()
            .next()
            .map(|update| (update.new_req.to_string(), update.bump))
    }

    #[test]
    fn uses_latest_nonbreaking_version() {
        assert_eq!(
            resolved_update("^1.2.0", &["1.4.0", "2.0.0"], false),
            Some(("^1.4.0".to_string(), VersionBump::Minor))
        );
        assert_eq!(
            resolved_update("^1.2.0", &["1.4.0", "2.0.0"], true),
            Some(("^2.0.0".to_string(), VersionBump::Major))
        );
        assert_eq!(resolved_update("^1.2.0", &["2.0.0"], false), None);
        assert_eq!(resolved_update("^1.2.0", &["1.0.0", "2.0.0"], false), None);
    }

    #[test]
    fn never_offers_downgrades() {
        for include_breaking in [false, true] {
            for (current, versions) in [
                ("^1.5.0", &["1.4.0"][..]),
                ("^2.0", &["1.9.0"][..]),
                ("^1.5.0-beta.2", &["1.5.0-beta.1"][..]),
                ("1.*", &["0.9.0"][..]),
            ] {
                assert_eq!(
                    resolved_update(current, versions, include_breaking),
                    None,
                    "{current}, include_breaking={include_breaking}"
                );
            }
        }
    }

    #[test]
    fn compatible_wildcards_are_nonbreaking() {
        for include_breaking in [false, true] {
            for (current, latest, expected, bump) in [
                ("1.*", "1.4.0", "^1.4", VersionBump::Minor),
                ("1.2.*", "1.2.5", "^1.2.5", VersionBump::Patch),
                ("0.*", "0.4.0", "^0.4", VersionBump::Patch),
            ] {
                assert_eq!(
                    resolved_update(current, &[latest], include_breaking),
                    Some((expected.to_string(), bump)),
                    "{current}"
                );
            }
        }
        assert_eq!(resolved_update("1.*", &["2.0.0"], false), None);
        assert_eq!(
            resolved_update("1.*", &["2.0.0"], true),
            Some(("^2.0".to_string(), VersionBump::Major))
        );
    }

    #[test]
    fn partial_requirements_are_nonbreaking() {
        assert_eq!(
            resolved_update("^1.53", &["1.54.0", "2.0.0"], false),
            Some(("^1.54".to_string(), VersionBump::Minor))
        );
        assert_eq!(
            resolved_update("^1.53", &["1.54.0", "2.0.0"], true),
            Some(("^2.0".to_string(), VersionBump::Major))
        );
    }

    #[test]
    fn matches_usage_by_dependency_kind() {
        let unit = Unit::Global;
        let req = Requirement::from_cargo("^1.0.0".parse().unwrap());
        let usage = |kind| Usage {
            unit: unit.clone(),
            req: req.clone(),
            kind,
            rename: None,
            supported_toolchain_version: None,
        };
        let package = Package {
            purl: "pkg:cargo/example".parse().unwrap(),
            usages: vec![usage(DepKind::Peer), usage(DepKind::Dev)],
            versions: vec![PackageVersion {
                version: Version::parse("1.1.0").unwrap(),
                yanked: false,
                features: Default::default(),
                required_toolchain_version: None,
            }],
            repository: None,
            homepage: None,
        };
        let packages = Packages::from([(
            unit.clone(),
            vec![
                (req.clone(), DepKind::Dev, package.clone()),
                (req.clone(), DepKind::Peer, package),
            ],
        )]);

        let updates = resolve_updates(&packages, &VersionStrategy::default(), false);
        let mut names: Vec<_> = updates[&unit].iter().map(display_name).collect();
        names.sort();
        assert_eq!(names, ["example (dev)", "example (peer)"]);
    }

    #[test]
    fn sorts_updates_by_kind_then_name() {
        let unit = Unit::Global;
        let req = Requirement::from_cargo("^1.0.0".parse().unwrap());
        let mut entries = Vec::new();
        for kind in [
            DepKind::Peer,
            DepKind::Dev,
            DepKind::Normal,
            DepKind::Optional,
            DepKind::Build,
        ] {
            for name in ["zulu", "alpha"] {
                let package = Package {
                    purl: format!("pkg:cargo/{name}").parse().unwrap(),
                    usages: vec![Usage {
                        unit: unit.clone(),
                        req: req.clone(),
                        kind,
                        rename: None,
                        supported_toolchain_version: None,
                    }],
                    versions: vec![PackageVersion {
                        version: Version::parse("1.1.0").unwrap(),
                        yanked: false,
                        features: Default::default(),
                        required_toolchain_version: None,
                    }],
                    repository: None,
                    homepage: None,
                };
                entries.push((req.clone(), kind, package));
            }
        }
        let packages = Packages::from([(unit.clone(), entries)]);

        let updates = resolve_updates(&packages, &VersionStrategy::default(), false);
        let names: Vec<_> = updates[&unit].iter().map(display_name).collect();

        assert_eq!(
            names,
            [
                "alpha",
                "zulu",
                "alpha (build)",
                "zulu (build)",
                "alpha (dev)",
                "zulu (dev)",
                "alpha (optional)",
                "zulu (optional)",
                "alpha (peer)",
                "zulu (peer)",
            ]
        );
    }

    #[test]
    fn pre_one_patch_is_nonbreaking() {
        assert_eq!(
            resolved_update("^0.1.0", &["0.1.5", "0.2.0"], false),
            Some(("^0.1.5".to_string(), VersionBump::Patch))
        );
        assert_eq!(
            resolved_update("^0.0.1", &["0.0.2"], false),
            Some(("^0.0.2".to_string(), VersionBump::Patch))
        );
    }

    #[test]
    fn prerelease_updates_stay_breaking() {
        assert_eq!(
            resolved_update("^1.0.0-beta.1", &["1.0.0-beta.2"], false),
            None
        );
        assert_eq!(
            resolved_update("^1.0.0-beta.1", &["1.0.0-beta.2"], true),
            Some(("^1.0.0-beta.2".to_string(), VersionBump::Major))
        );
    }

    #[test]
    fn major_updates_are_not_preselected() {
        let unit = Unit::Global;
        let current_req = Requirement::from_cargo("^1.0.0".parse().unwrap());
        let usage = Usage {
            unit: unit.clone(),
            req: current_req.clone(),
            kind: DepKind::Normal,
            rename: None,
            supported_toolchain_version: None,
        };
        let package = Package {
            purl: "pkg:cargo/example".parse().unwrap(),
            usages: vec![],
            versions: vec![],
            repository: None,
            homepage: None,
        };
        let updates = [VersionBump::Patch, VersionBump::Major, VersionBump::Minor]
            .into_iter()
            .map(|bump| Update {
                name: "example",
                current_req: &current_req,
                new_req: Requirement::from_cargo("^2.0.0".parse().unwrap()),
                bump,
                yanked: false,
                usage: &usage,
                package: &package,
            })
            .collect();
        let grouped = BTreeMap::from([(&unit, updates)]);

        let (groups, default_selected) = selection_groups(&grouped, false);

        assert_eq!(groups[0].updates.len(), 3);
        assert_eq!(default_selected, vec![true, false, true]);
    }

    #[test]
    fn normalizes_repo_urls() {
        for url in [
            "git@github.com:user/repo",
            "git://github.com/user/repo",
            "https://github.com/user/repo.git",
            "http://github.com/user/repo",
            "https://github.com/user/repo",
        ] {
            assert_eq!(
                normalize_repo_url(url),
                "https://github.com/user/repo",
                "{url}"
            );
        }
    }

    #[test]
    fn escapes_group_names() {
        for manifest in ["Cargo.toml", "package.json"] {
            let unit = Unit::Workspace {
                manifest: format!("unsafe\x1b[2J/{manifest}").into(),
            };
            for mixed in [false, true] {
                let name = group_name(&unit, mixed);
                assert!(!name.chars().any(char::is_control), "{name:?}");
                assert!(name.contains("\\u{1b}[2J"));
            }
        }
    }
}
