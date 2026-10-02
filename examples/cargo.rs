use std::path::PathBuf;

use check_updates::{CheckUpdates, Options, VersionStrategy};

#[tokio::main]
async fn main() {
    if let Err(err) = run().await {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

async fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/cargo/workspace-demo"));

    let checker = CheckUpdates::with_options(Some(root), Options::default());
    let packages = checker.packages().await?;

    let mut total = 0usize;

    for (unit, entries) in &packages {
        let mut printed_header = false;

        for (req, _kind, package) in entries {
            let Some(current) = req.current_version() else {
                continue;
            };
            let Some(latest) = package.latest(req, &VersionStrategy::stable(), None) else {
                continue;
            };
            if latest <= &current {
                continue;
            }

            if !printed_header {
                println!("\n{}", unit.name());
                printed_header = true;
            }

            println!("  {:<24} {:<12} -> {}", package.purl.name(), req, latest);
            total += 1;
        }
    }

    println!("\nFound {total} available upgrades.");
    Ok(())
}
