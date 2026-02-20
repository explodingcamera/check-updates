use check_updates::CheckUpdates;
use console::Style;
use indicatif::{ProgressBar, ProgressStyle};

pub mod cli;
mod interactive;
mod update;
mod version;

pub fn run(args: cli::Args) {
    env_logger::Builder::new()
        .filter_level(if args.verbose {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        })
        .init();

    let strategy = version::VersionStrategy::from_args(&args);

    let spinner = ProgressBar::new_spinner().with_style(
        ProgressStyle::default_spinner()
            .template("{spinner:.cyan} {msg}")
            .expect("valid template"),
    );
    spinner.set_message("Fetching package data...");
    spinner.enable_steady_tick(std::time::Duration::from_millis(80));

    let check_updates = CheckUpdates::new(args.root.clone());
    let packages = match check_updates.packages() {
        Ok(p) => p,
        Err(e) => {
            spinner.finish_and_clear();
            eprintln!("{} {e}", Style::new().red().bold().apply_to("error:"));
            std::process::exit(1);
        }
    };

    spinner.finish_and_clear();

    if !args.package.is_empty() {
        let known: std::collections::HashSet<&str> = packages
            .values()
            .flatten()
            .map(|(_, _, pkg)| pkg.purl.name())
            .collect();

        for name in &args.package {
            if !known.contains(name.as_str()) {
                eprintln!(
                    "{} package '{}' not found in dependencies",
                    Style::new().red().bold().apply_to("error:"),
                    name
                );
                std::process::exit(1);
            }
        }
    }

    let updates = update::resolve_updates(&packages, &strategy, &args.package);

    if args.interactive {
        if updates.is_empty() {
            update::print_summary(&updates);
            return;
        }

        let selected = match interactive::prompt_updates(&updates) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{} {e}", Style::new().red().bold().apply_to("error:"));
                std::process::exit(1);
            }
        };

        if selected.is_empty() {
            println!("\n No packages selected.");
            return;
        }

        let count = selected.len();
        if let Err(e) =
            check_updates.update_versions(selected.iter().map(|(u, p, r)| (*u, *p, r.clone())))
        {
            eprintln!("{} {e}", Style::new().red().bold().apply_to("error:"));
            std::process::exit(1);
        }

        if args.update {
            run_cargo_update();
        }

        println!(
            "\n Upgraded {count} {}.",
            if count == 1 {
                "dependency"
            } else {
                "dependencies"
            }
        );
    } else if args.upgrade || args.update {
        update::print_summary(&updates);

        if updates.is_empty() {
            if args.update {
                run_cargo_update();
            }
            return;
        }

        let count: usize = updates.values().map(|v| v.len()).sum();

        if let Err(e) = check_updates.update_versions(updates.values().flat_map(|unit_updates| {
            unit_updates
                .iter()
                .map(|u| (u.usage, u.package, u.new_req.clone()))
        })) {
            eprintln!("{} {e}", Style::new().red().bold().apply_to("error:"));
            std::process::exit(1);
        }

        if args.update {
            run_cargo_update();
        }

        println!(
            "\n Upgraded {count} {}.",
            if count == 1 {
                "dependency"
            } else {
                "dependencies"
            }
        );
    } else {
        update::print_summary(&updates);
        if !updates.is_empty() {
            println!(
                "\n{}",
                Style::new().dim().apply_to("Run with -u or -U to upgrade.")
            );
        }
    }
}

fn run_cargo_update() {
    let status = std::process::Command::new("cargo")
        .arg("update")
        .status()
        .expect("failed to run cargo update");

    if !status.success() {
        eprintln!(
            "{} cargo update failed",
            Style::new().red().bold().apply_to("error:")
        );
        std::process::exit(1);
    }
}
