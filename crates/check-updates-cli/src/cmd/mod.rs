pub mod cargo;
pub mod npm;

use std::path::Path;

use console::Style;
use indicatif::{ProgressBar, ProgressStyle};

use crate::{cli, interactive, update};

pub async fn run(
    root: &Path,
    args: &cli::Args,
    has_cargo: bool,
    has_npm: bool,
) -> Result<(bool, bool), String> {
    if !has_cargo && !has_npm {
        return Err(format!(
            "No Cargo.toml or package.json found in {}",
            root.display()
        ));
    }
    let spinner = ProgressBar::new_spinner().with_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid template"),
    );
    spinner.set_message("Fetching package data...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    let cargo = if has_cargo {
        Some(cargo::fetch(args).await?)
    } else {
        None
    };
    let npm = if has_npm {
        Some(npm::fetch(root, args).await?)
    } else {
        None
    };
    spinner.finish_and_clear();

    for name in &args.package {
        let cargo_match = cargo
            .as_ref()
            .is_some_and(|project| project.matched_packages.contains(name));
        let npm_match = npm
            .as_ref()
            .is_some_and(|project| project.projects.contains(name));
        if !cargo_match && !npm_match {
            return Err(format!("workspace package '{name}' not found"));
        }
    }

    let strategy = args.version_strategy();
    let cargo_updates = cargo
        .as_ref()
        .map(|project| update::resolve_updates(&project.packages, &strategy, &args.package))
        .unwrap_or_default();
    let npm_updates = npm
        .as_ref()
        .map(|project| update::resolve_updates(&project.packages, &strategy, &[]))
        .unwrap_or_default();

    let mixed = has_cargo && has_npm;
    let has_updates = !cargo_updates.is_empty() || !npm_updates.is_empty();
    if !args.interactive || !has_updates {
        if !has_updates {
            println!("No packages need version requirement updates.");
        } else {
            let mut groups = update::summary_groups(&cargo_updates, mixed);
            groups.extend(update::summary_groups(&npm_updates, mixed));
            interactive::print_groups(
                &groups,
                mixed || cargo_updates.len() > 1 || !npm_updates.is_empty(),
            );
        }
    }

    if args.interactive || args.update || args.upgrade {
        let cargo_count: usize = cargo_updates.values().map(Vec::len).sum();
        let mut groups = update::selection_groups(&cargo_updates, mixed);
        groups.extend(update::selection_groups(&npm_updates, mixed));
        let selected = if args.interactive {
            interactive::prompt_updates(&groups, args.compact).map_err(|error| error.to_string())?
        } else {
            (0..groups.iter().map(|group| group.updates.len()).sum()).collect()
        };
        if let Some(project) = &cargo {
            let edits: Vec<_> = cargo_updates
                .values()
                .flatten()
                .enumerate()
                .filter(|(index, _)| selected.binary_search(index).is_ok())
                .map(|(_, entry)| (entry.usage, entry.package, entry.new_req.clone()))
                .collect();
            if !edits.is_empty() {
                project
                    .checker
                    .update_versions(edits)
                    .map_err(|error| error.to_string())?;
            }
        }
        if npm.is_some() {
            let edits: Vec<_> = npm_updates
                .values()
                .flatten()
                .enumerate()
                .filter(|(index, _)| selected.binary_search(&(cargo_count + index)).is_ok())
                .map(|(_, entry)| (entry.usage, entry.package, entry.new_req.clone()))
                .collect();
            if !edits.is_empty() {
                check_updates::npm::update_packages(edits)?;
            }
        }
        if args.upgrade {
            if cargo.is_some() {
                cargo::update_lockfile(args.root.as_deref(), args.ignore_toolchain_version)?;
            }
            if npm.is_some() {
                npm::install_updates(root, args.lockfile_only)?;
            }
        }
        if !selected.is_empty() {
            println!(
                "\n Upgraded {} {}.",
                selected.len(),
                if selected.len() == 1 {
                    "dependency"
                } else {
                    "dependencies"
                }
            );
        } else if args.interactive && has_updates {
            println!("No packages selected.");
        }
    } else if has_updates && !mixed {
        println!(
            "\n{}",
            Style::new().dim().apply_to("Run with -u or -U to upgrade.")
        );
    }
    Ok((!cargo_updates.is_empty(), !npm_updates.is_empty()))
}
