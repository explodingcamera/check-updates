use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use check_updates::{Package, Packages, Requirement, Unit, Usage, VersionStrategy};
use console::Style;

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

fn display_name(update: &Update<'_>) -> String {
    match update.usage.rename.as_deref() {
        Some(alias) if alias != update.name => format!("{} ({alias})", display_text(update.name)),
        _ => display_text(update.name),
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
    filter: &[String],
) -> BTreeMap<&'a Unit, Vec<Update<'a>>> {
    let mut result: BTreeMap<&Unit, Vec<Update>> = BTreeMap::new();

    for (unit, entries) in packages {
        for (req, _dep_kind, package) in entries {
            let name = package.purl.name();

            if !filter.is_empty() && !filter.iter().any(|f| unit_matches_filter(unit, f)) {
                continue;
            }

            let current = req.current_version();
            let Some(usage) = package
                .usages
                .iter()
                .find(|u| u.unit == *unit && u.req == *req)
            else {
                continue;
            };
            let supported_toolchain_version = package
                .usages
                .iter()
                .filter(|u| u.unit == *unit && u.req == *req)
                .filter_map(|u| u.supported_toolchain_version.clone())
                .min();
            let latest = package.latest(req, strategy, supported_toolchain_version.as_ref());

            if let Some(supported_toolchain_version) = supported_toolchain_version.as_ref()
                && !strategy.ignore_toolchain_version
            {
                let ignored_strategy = VersionStrategy {
                    compatible: strategy.compatible,
                    pre: strategy.pre,
                    ignore_toolchain_version: true,
                };
                let unrestricted = package.latest(req, &ignored_strategy, None);

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

            let bump = current
                .as_ref()
                .map(|cur| version_bump(cur, latest))
                .unwrap_or(VersionBump::Major);

            let yanked = current
                .as_ref()
                .is_some_and(|version| package.is_version_yanked(version));

            result.entry(unit).or_default().push(Update {
                name,
                current_req: req,
                new_req,
                bump,
                yanked,
                usage,
                package,
            });
        }
    }

    for unit_updates in result.values_mut() {
        unit_updates.sort_unstable_by_key(|u| u.name);
    }

    result
}

pub(crate) fn unit_matches_filter(unit: &Unit, filter: &str) -> bool {
    match unit {
        Unit::Project { name, .. } => filter == name,
        Unit::Workspace { manifest } => {
            filter == "workspace"
                || workspace_root_name(manifest).is_some_and(|name| name == filter)
        }
        Unit::Global => filter == "global",
    }
}

fn workspace_root_name(manifest: &Path) -> Option<String> {
    manifest
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .map(ToString::to_string)
}

/// Format a single update as an aligned, colored line.
pub fn format_update_line(
    update: &Update,
    name_width: usize,
    cur_width: usize,
    new_width: usize,
) -> String {
    let mut line = String::new();
    let f = &mut line;

    let plain_display_name = display_name(update);
    let name_display = update
        .package
        .repository
        .as_ref()
        .map(|url| normalize_repo_url(url))
        .filter(|url| !url.chars().any(char::is_control))
        .map(|url| hyperlink(&url, &plain_display_name))
        .unwrap_or_else(|| plain_display_name.clone());

    // Pad manually since hyperlink escape codes don't count as visible width
    let padding = name_width.saturating_sub(plain_display_name.len());
    let _ = write!(f, " {}{:>padding$}", name_display, "");

    let cur_req_str = display_requirement(update, update.current_req);
    let cur_display_len = cur_req_str.len() + if update.yanked { 9 } else { 0 };
    let cur_padding = cur_width.saturating_sub(cur_display_len);

    if update.yanked {
        let _ = write!(
            f,
            "  {:>cur_padding$}{} {}",
            "",
            Style::new().white().apply_to(&cur_req_str),
            Style::new().yellow().apply_to("(yanked)")
        );
    } else {
        let _ = write!(
            f,
            "  {:>cur_padding$}{}",
            "",
            Style::new().white().apply_to(&cur_req_str)
        );
    }

    let _ = write!(f, "  →  ");

    let new_req_str = display_requirement(update, &update.new_req);
    let colorized = colorize_req(&cur_req_str, &new_req_str, update.bump);

    let new_padding = new_width.saturating_sub(new_req_str.len());
    let _ = write!(f, "{:>new_padding$}{}", "", colorized);

    line
}

pub fn summary_groups(updates: &BTreeMap<&Unit, Vec<Update<'_>>>, mixed: bool) -> Vec<UpdateGroup> {
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
                    .map(|update| format_update_line(update, name_w, cur_w, new_w))
                    .collect(),
            }
        })
        .collect()
}

fn group_name(unit: &Unit, mixed: bool) -> String {
    if unit
        .path()
        .is_some_and(|path| path.file_name().is_some_and(|name| name == "package.json"))
    {
        let name = display_text(&unit.name());
        return if mixed {
            match unit {
                Unit::Workspace { .. } => {
                    format!("{} (npm workspace)", name.trim_end_matches(" (workspace)"))
                }
                _ => format!("{name} (npm)"),
            }
        } else {
            name
        };
    }
    if mixed {
        match unit {
            Unit::Workspace { .. } => format!(
                "{} (cargo workspace)",
                unit.name().trim_end_matches(" (workspace)")
            ),
            _ => format!("{} (cargo)", unit.name()),
        }
    } else {
        unit.name().to_string()
    }
}

pub fn selection_groups(
    updates: &BTreeMap<&Unit, Vec<Update<'_>>>,
    mixed: bool,
) -> Vec<UpdateGroup> {
    let (name_w, cur_w, new_w) = updates.values().flatten().fold((0, 0, 0), |widths, u| {
        (
            widths.0.max(if u.package.purl.package_type() == "npm" {
                display_name(u).len()
            } else {
                u.name.len()
            }),
            widths
                .1
                .max(display_requirement(u, u.current_req).len() + if u.yanked { 9 } else { 0 }),
            widths.2.max(display_requirement(u, &u.new_req).len()),
        )
    });
    let target = (console::Term::stderr().size().1 as usize).min(40);
    let name_w = name_w.max(target.saturating_sub(cur_w + new_w + 4));
    updates
        .iter()
        .map(|(unit, entries)| UpdateGroup {
            name: group_name(unit, mixed),
            updates: entries
                .iter()
                .map(|u| format_update_line(u, name_w, cur_w, new_w))
                .collect(),
        })
        .collect()
}

fn hyperlink(url: &str, text: &str) -> String {
    format!("\x1b]8;;{url}\x1b\\{text}\x1b]8;;\x1b\\")
}

fn normalize_repo_url(url: &str) -> String {
    let url = url.trim();

    // Strip .git suffix
    let url = url.strip_suffix(".git").unwrap_or(url);

    // Convert git@ to https://
    if let Some(rest) = url.strip_prefix("git@") {
        // git@github.com:user/repo -> https://github.com/user/repo
        if let Some((host, path)) = rest.split_once(':') {
            return format!("https://{host}/{path}");
        }
    }

    // Convert git:// to https://
    if let Some(rest) = url.strip_prefix("git://") {
        return format!("https://{rest}");
    }

    // Ensure https://
    if url.starts_with("http://") {
        return url.replacen("http://", "https://", 1);
    }

    url.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_git_ssh() {
        assert_eq!(
            normalize_repo_url("git@github.com:user/repo"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn normalize_git_protocol() {
        assert_eq!(
            normalize_repo_url("git://github.com/user/repo"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn normalize_strip_git_suffix() {
        assert_eq!(
            normalize_repo_url("https://github.com/user/repo.git"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn normalize_http_to_https() {
        assert_eq!(
            normalize_repo_url("http://github.com/user/repo"),
            "https://github.com/user/repo"
        );
    }

    #[test]
    fn normalize_https_unchanged() {
        assert_eq!(
            normalize_repo_url("https://github.com/user/repo"),
            "https://github.com/user/repo"
        );
    }
}
