use check_updates_cli::cli::Args;
use clap::Parser;

#[tokio::main]
async fn main() {
    check_updates_cli::run(Args::parse()).await;
}
