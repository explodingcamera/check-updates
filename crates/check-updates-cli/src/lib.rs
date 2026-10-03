use clap::CommandFactory;

pub mod cli;
mod cmd;
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

    let root = args
        .root
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory"));
    let has_npm = root.join("package.json").is_file();
    let has_cargo = root.join("Cargo.toml").is_file();
    if has_npm
        && args.upgrade
        && let Err(error) = check_updates::npm::package_manager(&root)
    {
        log::error!("Failed to select JavaScript package manager: {error}");
        std::process::exit(1);
    }
    let (cargo_updates, npm_updates) = match cmd::run(&root, &args, has_cargo, has_npm).await {
        Ok(updates) => updates,
        Err(error) => {
            log::error!("Failed to check dependencies: {error}");
            std::process::exit(1);
        }
    };
    if args.fail_on_updates && (npm_updates || cargo_updates) {
        std::process::exit(2);
    }
}
