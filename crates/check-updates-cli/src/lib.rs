use check_updates::{CheckUpdates, Options, RegistryCachePolicy};
use clap::CommandFactory;
use console::Style;
use indicatif::{ProgressBar, ProgressStyle};

pub mod cli;
mod interactive;
mod update;
mod version;

pub async fn run(args: cli::Args) {
    env_logger::Builder::new()
        .filter_level(if args.verbose {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        })
        .init();

    if let Some(cli::Command::GenerateShellCompletion { shell }) = args.cmd {
        clap_complete::generate(
            shell,
            &mut cli::Args::command(),
            "check-updates",
            &mut std::io::stdout(),
        );
        return;
    }

    let strategy = version::VersionStrategy::from_args(&args);

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
    let packages = match check_updates.packages().await {
        Ok(p) => p,
        Err(e) => {
            spinner.finish_and_clear();
            log::error!("Failed to fetch package data: {e}");
            std::process::exit(1);
        }
    };

    spinner.finish_and_clear();

    if !args.package.is_empty() {
        for name in &args.package {
            if !packages
                .keys()
                .any(|unit| update::unit_matches_filter(unit, name))
            {
                log::error!("workspace package '{}' not found", name);
                std::process::exit(1);
            }
        }
    }

    let updates = update::resolve_updates(&packages, &strategy, &args.package);
    let has_updates = !updates.is_empty();
    let fail_on_updates = args.fail_on_updates;

    if args.interactive {
        if updates.is_empty() {
            update::print_summary(&updates);
            if args.upgrade {
                run_cargo_update();
            }
            return;
        }

        let selected = match interactive::prompt_updates(&updates, args.compact) {
            Ok(s) => s,
            Err(e) => {
                log::error!("{e}");
                std::process::exit(1);
            }
        };

        if selected.is_empty() {
            if args.upgrade {
                run_cargo_update();
            }
            println!("No packages selected.");
            return;
        }

        let count = selected.len();
        if let Err(e) =
            check_updates.update_versions(selected.iter().map(|(u, p, r)| (*u, *p, r.clone())))
        {
            log::error!("{e}");
            std::process::exit(1);
        }

        if args.upgrade {
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
    } else if args.update || args.upgrade {
        update::print_summary(&updates);

        if updates.is_empty() {
            if args.upgrade {
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
            log::error!("{e}");
            std::process::exit(1);
        }

        if args.upgrade {
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

    if fail_on_updates && has_updates {
        std::process::exit(2);
    }
}

fn run_cargo_update() {
    let status = std::process::Command::new("cargo")
        .arg("update")
        .status()
        .expect("failed to run cargo update");

    if !status.success() {
        log::error!("cargo update failed");
        std::process::exit(1);
    }
}
