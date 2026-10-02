use std::path::PathBuf;

use check_updates::npm::{self, NpmOptions};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("examples/npm/workspace-demo"));

    let packages = npm::updates(&root, &NpmOptions::default()).await?;
    for update in packages.updates {
        println!(
            "{}: {} ({}) {} -> {}",
            update.project, update.name, update.section, update.current, update.proposed
        );
    }
    Ok(())
}
