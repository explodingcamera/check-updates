use std::path::PathBuf;

use check_updates::VersionStrategy;
use check_updates::npm::{self, NpmOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/npm/workspace-demo"));

    let found = npm::packages(&root, &NpmOptions::default()).await?;
    for (unit, dependencies) in found.packages {
        for (requirement, _, package) in dependencies {
            if let Some(latest) = package.latest(&requirement, &VersionStrategy::stable(), None)
                && let Some(proposed) = requirement.with_version(latest)
                && proposed != requirement
            {
                println!(
                    "{}: {} {} -> {}",
                    unit.name(),
                    package.purl.name(),
                    requirement,
                    proposed
                );
            }
        }
    }
    Ok(())
}
