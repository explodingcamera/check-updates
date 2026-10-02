use std::path::Path;

use check_updates::{CheckUpdates, Options, RegistryCachePolicy, VersionStrategy};
use console::Style;
use indicatif::{ProgressBar, ProgressStyle};

use crate::{cli, interactive, update};

pub struct Result {
    pub has_updates: bool,
    pub matched_packages: Vec<String>,
}

pub async fn run(args: &cli::Args, mixed: bool) -> std::result::Result<Result, String> {
    let strategy = VersionStrategy {
        compatible: args.compatible,
        pre: args.pre,
        ignore_toolchain_version: args.ignore_toolchain_version,
    };
    let spinner = ProgressBar::new_spinner().with_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid template"),
    );
    spinner.set_message("Fetching package data...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));
    let options = Options {
        registry_cache_policy: match args.cache {
            cli::RegistryCacheMode::PreferLocal => RegistryCachePolicy::PreferLocal,
            cli::RegistryCacheMode::Refresh => RegistryCachePolicy::Refresh,
            cli::RegistryCacheMode::NoCache => RegistryCachePolicy::NoCache,
        },
    };
    let check_updates = CheckUpdates::with_options(args.root.clone(), options);
    let packages = check_updates
        .packages()
        .await
        .map_err(|error| error.to_string());
    spinner.finish_and_clear();
    let packages = packages?;
    let matched_packages: Vec<String> = args
        .package
        .iter()
        .filter(|name| {
            packages
                .keys()
                .any(|unit| update::unit_matches_filter(unit, name))
        })
        .cloned()
        .collect();
    if !mixed {
        for name in &args.package {
            if !matched_packages.contains(name) {
                return Err(format!("workspace package '{name}' not found"));
            }
        }
    }

    let updates = update::resolve_updates(&packages, &strategy, &args.package);
    let has_updates = !updates.is_empty();
    if args.interactive {
        if updates.is_empty() {
            update::print_summary(&updates, mixed);
            if args.upgrade {
                run_cargo_update(args.root.as_deref(), args.ignore_toolchain_version)?;
            }
        } else {
            let selected = interactive::prompt_updates(&updates, args.compact, mixed)
                .map_err(|error| error.to_string())?;
            if selected.is_empty() {
                if args.upgrade {
                    run_cargo_update(args.root.as_deref(), args.ignore_toolchain_version)?;
                }
                println!("No packages selected.");
            } else {
                check_updates
                    .update_versions(selected.iter().map(|(u, p, r)| (*u, *p, r.clone())))
                    .map_err(|error| error.to_string())?;
                if args.upgrade {
                    run_cargo_update(args.root.as_deref(), args.ignore_toolchain_version)?;
                }
                println!(
                    "\n Upgraded {} {}.",
                    selected.len(),
                    if selected.len() == 1 {
                        "dependency"
                    } else {
                        "dependencies"
                    }
                );
            }
        }
    } else if args.update || args.upgrade {
        update::print_summary(&updates, mixed);
        if !updates.is_empty() {
            let count: usize = updates.values().map(Vec::len).sum();
            check_updates
                .update_versions(updates.values().flat_map(|unit_updates| {
                    unit_updates
                        .iter()
                        .map(|u| (u.usage, u.package, u.new_req.clone()))
                }))
                .map_err(|error| error.to_string())?;
            if args.upgrade {
                run_cargo_update(args.root.as_deref(), args.ignore_toolchain_version)?;
            }
            println!(
                "\n Upgraded {count} {}.",
                if count == 1 {
                    "dependency"
                } else {
                    "dependencies"
                }
            );
        } else if args.upgrade {
            run_cargo_update(args.root.as_deref(), args.ignore_toolchain_version)?;
        }
    } else {
        update::print_summary(&updates, mixed);
        if has_updates && !mixed {
            println!(
                "\n{}",
                Style::new().dim().apply_to("Run with -u or -U to upgrade.")
            );
        }
    }
    Ok(Result {
        has_updates,
        matched_packages,
    })
}

fn run_cargo_update(
    root: Option<&Path>,
    ignore_toolchain_version: bool,
) -> std::result::Result<(), String> {
    let mut command = std::process::Command::new("cargo");
    command.arg("update");
    if ignore_toolchain_version {
        command.arg("--ignore-rust-version");
    }
    if let Some(root) = root {
        command.current_dir(root);
    }
    let status = command
        .status()
        .map_err(|error| format!("failed to run cargo update: {error}"))?;
    if !status.success() {
        return Err("cargo update failed".into());
    }
    Ok(())
}
