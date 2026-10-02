use std::path::Path;

use check_updates::VersionStrategy;
use check_updates::npm::{NpmOptions, PackageManager};
use console::{Key, Term};

use crate::cli::Args;

pub async fn check(
    root: &Path,
    args: &Args,
    cargo_packages: &[String],
    mixed: bool,
) -> Result<bool, String> {
    let packages = check_updates::npm::updates(
        root,
        &NpmOptions {
            packages: &args.package,
            strategy: VersionStrategy {
                compatible: args.compatible,
                pre: args.pre,
                ignore_toolchain_version: args.ignore_toolchain_version,
            },
        },
    )
    .await?;
    if !args.package.is_empty() {
        for name in &args.package {
            if !cargo_packages.contains(name) && !packages.projects.contains(name) {
                return Err(format!("workspace package '{name}' not found"));
            }
        }
    }
    let updates = packages.updates;
    let has_updates = !updates.is_empty();
    if !has_updates && !mixed {
        println!("No packages need version requirement updates.");
    }

    let mut last_project = None;
    for update in &updates {
        if last_project != Some(update.project.as_str()) {
            if last_project.is_some() {
                println!();
            }
            println!("{}{}", update.project, if mixed { " (npm)" } else { "" });
            last_project = Some(update.project.as_str());
        }
        println!(
            "  {} ({})  {} → {}",
            update.name, update.section, update.current, update.proposed
        );
    }

    if args.update || args.upgrade || args.interactive {
        let selected = if args.interactive {
            let term = Term::stderr();
            let mut selected = Vec::new();
            for update in &updates {
                term.write_line(&format!(
                    "Update {} from {} to {}? [Y/n]",
                    update.name, update.current, update.proposed
                ))
                .map_err(|error| error.to_string())?;
                if matches!(
                    term.read_key().map_err(|error| error.to_string())?,
                    Key::Enter | Key::Char('y' | 'Y')
                ) {
                    selected.push(update.clone());
                }
            }
            selected
        } else {
            updates.clone()
        };
        check_updates::npm::update_versions(&selected)?;
    }
    if args.upgrade {
        refresh_lockfile(root, args.lockfile_only)?;
    }
    Ok(has_updates)
}

fn refresh_lockfile(root: &Path, lockfile_only: bool) -> Result<(), String> {
    let manager = check_updates::npm::package_manager(root)?;
    let executable = match manager {
        PackageManager::Npm => "npm",
        PackageManager::Pnpm => "pnpm",
        PackageManager::Bun => "bun",
    };
    let mut command = std::process::Command::new(executable);
    command.arg("install");
    if lockfile_only {
        command.arg(if manager == PackageManager::Npm {
            "--package-lock-only"
        } else {
            "--lockfile-only"
        });
    }
    let status = command
        .arg("--ignore-scripts")
        .current_dir(root)
        .status()
        .map_err(|error| error.to_string())?;
    if !status.success() {
        return Err(format!("{executable} install failed"));
    }
    Ok(())
}
