use std::path::PathBuf;

use check_updates::CheckUpdates;
use semver::{Version, VersionReq};

fn main() {
    if let Err(err) = run() {
        eprintln!("error: {err}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/cargo/workspace-demo"));

    let checker = CheckUpdates::new(Some(root));
    let packages = checker.packages()?;

    let mut total = 0usize;

    for (unit, entries) in &packages {
        let mut printed_header = false;

        for (req, _kind, package) in entries {
            let Some(current) = req_base_version(req) else {
                continue;
            };
            let Some(latest) = latest_stable(&package.versions) else {
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

fn req_base_version(req: &VersionReq) -> Option<Version> {
    let s = req.to_string();
    let stripped = s.trim_start_matches(|c: char| !c.is_ascii_digit());
    Version::parse(stripped).ok()
}

fn latest_stable(versions: &[check_updates::PackageVersion]) -> Option<&Version> {
    versions
        .iter()
        .filter(|v| !v.yanked && v.version.pre.is_empty())
        .map(|v| &v.version)
        .max()
}
