use std::process::Command;

use super::ManagerImpl;

/// Native npm command builder.
pub(super) struct Npm;

impl ManagerImpl for Npm {
    fn upgrade_command(&self, lockfile_only: bool) -> Command {
        let mut command = Command::new("npm");
        command.args(["update", "--ignore-scripts"]);
        if lockfile_only {
            command.arg("--package-lock-only");
        }
        command
    }
}
