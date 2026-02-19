use check_updates::CheckUpdates;

pub mod cli;

pub fn run(args: cli::Args) {
    env_logger::Builder::new()
        .filter_level(if args.verbose {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        })
        .init();

    let check_updates = CheckUpdates::new();
    let packages = check_updates.packages().unwrap();
    dbg!(packages.keys());
}
