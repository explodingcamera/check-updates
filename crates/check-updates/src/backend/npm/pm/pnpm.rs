use std::process::Command;

use super::ManagerImpl;

mod workspace;
pub(in crate::backend::npm) use workspace::PnpmWorkspace;

/// Native pnpm command builder.
pub(super) struct Pnpm;

impl ManagerImpl for Pnpm {
    fn upgrade_command(&self, lockfile_only: bool) -> Command {
        let mut command = Command::new("pnpm");
        command.args(["update", "--no-save", "--ignore-scripts"]);
        if lockfile_only {
            command.arg("--lockfile-only");
        }
        command
    }
}
