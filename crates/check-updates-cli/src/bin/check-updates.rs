use check_updates_cli::cli::Args;
use clap::Parser;

fn main() {
    check_updates_cli::run(Args::parse());
}
