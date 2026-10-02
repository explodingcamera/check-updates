#![cfg(all(unix, feature = "npm"))]

use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn upgrade_uses_detected_manager_and_optional_lockfile_only() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    for manager in ["npm", "pnpm", "bun"] {
        let script = bin.join(manager);
        std::fs::write(&script, "#!/bin/sh\nprintf '%s\\n' \"$@\" > \"$CAPTURE\"\n").unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(script, permissions).unwrap();
    }

    for (manager, lockfile) in [
        ("npm", "package-lock.json"),
        ("pnpm", "pnpm-lock.yaml"),
        ("bun", "bun.lock"),
    ] {
        let project = dir.path().join(manager);
        std::fs::create_dir(&project).unwrap();
        std::fs::write(project.join("package.json"), r#"{"name":"test"}"#).unwrap();
        std::fs::write(project.join(lockfile), "").unwrap();
        let capture = dir.path().join(format!("{manager}-args"));
        for lockfile_only in [false, true] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_check-updates"));
            command
                .args(["--root", project.to_str().unwrap(), "-U"])
                .env("CAPTURE", &capture)
                .env(
                    "PATH",
                    format!(
                        "{}:{}",
                        bin.display(),
                        std::env::var("PATH").unwrap_or_default()
                    ),
                );
            if lockfile_only {
                command.arg("--lockfile-only");
            }
            let output = command.output().unwrap();
            assert!(
                output.status.success(),
                "{manager}: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let args = std::fs::read_to_string(&capture).unwrap();
            let expected = if lockfile_only {
                if manager == "npm" {
                    "install\n--package-lock-only\n--ignore-scripts\n"
                } else {
                    "install\n--lockfile-only\n--ignore-scripts\n"
                }
            } else {
                "install\n--ignore-scripts\n"
            };
            assert_eq!(args, expected, "{manager}");
        }
    }
}
