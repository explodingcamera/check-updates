use clap::CommandFactory;

pub mod cli;
mod cmd;
#[cfg(feature = "cargo")]
mod interactive;
#[cfg(feature = "cargo")]
mod update;
#[cfg(feature = "cargo")]
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
    #[cfg(feature = "npm")]
    if has_npm && args.upgrade {
        if let Err(error) = check_updates::npm::package_manager(&root) {
            log::error!("Failed to select JavaScript package manager: {error}");
            std::process::exit(1);
        }
    }
    if (has_npm && !has_cargo && !cfg!(feature = "npm"))
        || (has_cargo && !has_npm && !cfg!(feature = "cargo"))
    {
        log::error!("The backend for {} is not enabled", root.display());
        std::process::exit(1);
    }
    #[cfg(feature = "cargo")]
    let (cargo_updates, cargo_packages) = if has_cargo || !has_npm {
        match cmd::cargo::run(&args, has_npm && cfg!(feature = "npm")).await {
            Ok(result) => (result.has_updates, result.matched_packages),
            Err(error) => {
                log::error!("Failed to check Cargo dependencies: {error}");
                std::process::exit(1);
            }
        }
    } else {
        (false, Vec::new())
    };
    #[cfg(all(feature = "cargo", not(feature = "npm")))]
    let _ = &cargo_packages;
    #[cfg(not(feature = "cargo"))]
    let cargo_updates = false;
    #[cfg(all(not(feature = "cargo"), feature = "npm"))]
    let cargo_packages: Vec<String> = Vec::new();

    #[cfg(feature = "npm")]
    let npm_updates = if has_npm {
        match cmd::npm::check(
            &root,
            &args,
            &cargo_packages,
            has_cargo && cfg!(feature = "cargo"),
        )
        .await
        {
            Ok(has_updates) => has_updates,
            Err(error) => {
                log::error!("Failed to check npm dependencies: {error}");
                std::process::exit(1);
            }
        }
    } else {
        false
    };
    #[cfg(not(feature = "npm"))]
    let npm_updates = false;

    if has_cargo
        && has_npm
        && cfg!(feature = "cargo")
        && cfg!(feature = "npm")
        && !cargo_updates
        && !npm_updates
    {
        println!("No packages need version requirement updates.");
    }

    if !has_cargo && !has_npm && !cfg!(feature = "cargo") {
        log::error!("No Cargo.toml or package.json found in {}", root.display());
        std::process::exit(1);
    }
    if args.fail_on_updates && (npm_updates || cargo_updates) {
        std::process::exit(2);
    }
}
