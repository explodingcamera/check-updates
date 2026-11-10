pub(crate) mod project;

use std::path::Path;

use owo_colors::{OwoColorize, Stream};

use self::project::Project;
use crate::{cli, interactive, update};

pub async fn run(root: &Path, args: &cli::Args) -> Result<bool, String> {
    let projects =
        interactive::with_spinner("Fetching package data...", Project::discover(root, args))
            .await?;

    let include_breaking = args.interactive || args.breaking || (!args.update && !args.upgrade);
    let updates: Vec<_> = projects
        .iter()
        .map(|project| project.updates(args, include_breaking))
        .collect();

    let mixed = projects.len() > 1;
    let has_updates = updates.iter().any(|updates| !updates.is_empty());
    let breaking_updates_available = !include_breaking
        && !has_updates
        && projects
            .iter()
            .any(|project| !project.updates(args, true).is_empty());
    if !args.interactive || !has_updates {
        if !has_updates {
            if breaking_updates_available {
                println!(
                    "No non-breaking updates available. Use --breaking to include breaking updates."
                );
            } else {
                println!("No packages need version requirement updates.");
            }
        } else {
            let groups: Vec<_> = updates
                .iter()
                .flat_map(|updates| update::summary_groups(updates, mixed))
                .collect();
            let show_headers = mixed
                || updates.iter().any(|updates| updates.len() > 1)
                || projects
                    .iter()
                    .any(|project| project.backend.show_headers());
            interactive::print_groups(&groups, show_headers);
        }
    }

    if args.interactive || args.update || args.upgrade {
        let entries: Vec<_> = updates
            .iter()
            .flat_map(|updates| updates.values().flatten())
            .collect();
        let selected: Vec<_> = if args.interactive {
            let mut groups = Vec::new();
            let mut default_selected = Vec::new();
            for updates in &updates {
                let (project_groups, project_selected) = update::selection_groups(updates, mixed);
                groups.extend(project_groups);
                default_selected.extend(project_selected);
            }
            if args.breaking {
                default_selected.fill(true);
            }
            interactive::prompt_updates(&groups, &default_selected, args.compact)
                .map_err(|error| error.to_string())?
                .into_iter()
                .map(|index| entries[index])
                .collect()
        } else {
            entries
                .into_iter()
                .filter(|update| args.breaking || update.bump != crate::version::VersionBump::Major)
                .collect()
        };
        for project in &projects {
            project.update_versions(&selected)?;
        }
        if args.upgrade {
            for project in &projects {
                project.upgrade_dependencies(args)?;
            }
        }
        if !selected.is_empty() {
            println!(
                "\n Upgraded {} {}.",
                selected.len(),
                if selected.len() == 1 {
                    "dependency"
                } else {
                    "dependencies"
                }
            );
        } else if args.interactive && has_updates {
            println!("No packages selected.");
        }
    } else if has_updates && !mixed {
        println!(
            "\n{}",
            "Run with -u or -U to upgrade.".if_supports_color(Stream::Stdout, |text| text.dimmed())
        );
    }
    Ok(has_updates || breaking_updates_available)
}
