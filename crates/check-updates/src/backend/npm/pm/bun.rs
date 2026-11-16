use std::process::Command;

use super::ManagerImpl;

/// Native bun command builder.
pub(super) struct Bun;

impl ManagerImpl for Bun {
    fn upgrade_command(&self, lockfile_only: bool) -> Command {
        let mut command = Command::new("bun");
        // Bun update rewrites package.json, so apply our edits with install instead.
        command.args(["install", "--ignore-scripts"]);
        if lockfile_only {
            command.arg("--lockfile-only");
        }
        command
    }
}
