use check_updates_cli::cli::Args;
use clap::Parser;

#[tokio::main]
async fn main() {
    let mut args = std::env::args_os().skip(1).peekable();
    if args.peek().and_then(|a| a.to_str()) == Some("check-updates") {
        args.next();
    }

    let args = Args::parse_from(
        std::iter::once(std::ffi::OsString::from("cargo check-updates")).chain(args),
    );
    check_updates_cli::run(args).await;
}
