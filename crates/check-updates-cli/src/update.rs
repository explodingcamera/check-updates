use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

use check_updates::{Package, Packages, Requirement, Unit, Usage, VersionStrategy};
use console::Style;

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
        Some(alias) if alias != update.name => format!("{} ({alias})", update.name),
        _ => update.name.to_string(),
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
        .map(|url| hyperlink(&normalize_repo_url(url), &plain_display_name))
        .unwrap_or_else(|| plain_display_name.clone());

    // Pad manually since hyperlink escape codes don't count as visible width
    let padding = name_width.saturating_sub(plain_display_name.len());
    let _ = write!(f, " {}{:>padding$}", name_display, "");

    let cur_req_str = update.current_req.to_string();
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

    let new_req_str = update.new_req.to_string();
    let colorized = colorize_req(&cur_req_str, &new_req_str, update.bump);

    let new_padding = new_width.saturating_sub(new_req_str.len());
    let _ = write!(f, "{:>new_padding$}{}", "", colorized);

    line
}

/// Print the update table, grouped by unit.
// TODO: maybe print this as a table / add a json output option
pub fn print_summary(updates: &BTreeMap<&Unit, Vec<Update<'_>>>, show_unit: bool) {
    if updates.is_empty() {
        if !show_unit {
            println!("No packages need version requirement updates.");
        }
        return;
    }

    let multi_unit = show_unit || updates.len() > 1;
    let mut first = true;

    for (unit, unit_updates) in updates {
        if multi_unit {
            if !first {
                println!();
            }
            let name = if show_unit {
                match unit {
                    Unit::Workspace { .. } => format!(
                        "{} (cargo workspace)",
                        unit.name().trim_end_matches(" (workspace)")
                    ),
                    _ => format!("{} (cargo)", unit.name()),
                }
            } else {
                unit.name().to_string()
            };
            println!("{}", Style::new().bold().apply_to(name));
        } else if !first {
            println!();
        }
        first = false;

        let name_w = unit_updates
            .iter()
            .map(|u| display_name(u).len())
            .max()
            .unwrap_or(0);
        let cur_w = unit_updates
            .iter()
            .map(|u| u.current_req.to_string().len() + if u.yanked { 9 } else { 0 })
            .max()
            .unwrap_or(0);
        let new_w = unit_updates
            .iter()
            .map(|u| u.new_req.to_string().len())
            .max()
            .unwrap_or(0);

        for update in unit_updates {
            println!("{}", format_update_line(update, name_w, cur_w, new_w));
        }
    }
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
