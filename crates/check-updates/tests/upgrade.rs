use std::process::Command;

#[test]
fn requires_manifest_for_upgrade() {
    let dir = tempfile::tempdir().unwrap();
    for (binary, prefix) in [
        (env!("CARGO_BIN_EXE_check-updates"), &[][..]),
        (
            env!("CARGO_BIN_EXE_cargo-check-updates"),
            &["check-updates"][..],
        ),
    ] {
        let output = Command::new(binary)
            .env("NO_COLOR", "1")
            .args(prefix)
            .arg("--root")
            .arg(dir.path())
            .arg("-U")
            .output()
            .unwrap();
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains("No Cargo.toml or package.json found"));
        assert!(stderr.starts_with("ERROR: "), "{stderr}");
        assert!(!stderr.contains('\x1b'), "{stderr:?}");
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn unsupported_managers_allow_manifest_updates() {
    let dir = tempfile::tempdir().unwrap();
    for (manifest, lockfile, error) in [
        (
            r#"{"name":"test","packageManager":"bad\u001b[2J@1.0.0"}"#,
            false,
            "unsupported package manager 'bad\\u{1b}[2J'",
        ),
        (
            r#"{"name":"test","packageManager":"yarn@4.0.0"}"#,
            false,
            "unsupported package manager 'yarn'",
        ),
        (
            r#"{"name":"test"}"#,
            true,
            "unsupported lockfile 'yarn.lock'",
        ),
    ] {
        std::fs::write(dir.path().join("package.json"), manifest).unwrap();
        if lockfile {
            std::fs::write(dir.path().join("yarn.lock"), "").unwrap();
        }
        for flag in ["-u", "-U"] {
            let mut command = Command::new(env!("CARGO_BIN_EXE_check-updates"));
            command.arg("--root").arg(dir.path()).arg(flag);
            if flag == "-U" {
                command.args(["--package", "missing"]);
            }
            let output = command.output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(output.status.success(), flag == "-u", "{stderr}");
            if flag == "-U" {
                assert!(stderr.contains(error), "{stderr}");
            }
            assert_eq!(
                std::fs::read_to_string(dir.path().join("package.json")).unwrap(),
                manifest
            );
        }
    }
}

#[test]
fn upgrade_with_installed_managers() {
    let dir = tempfile::tempdir().unwrap();
    for (manager, lockfile) in [
        ("npm", "package-lock.json"),
        ("pnpm", "pnpm-lock.yaml"),
        ("bun", "bun.lock"),
    ] {
        let output = Command::new(manager).arg("--version").output().unwrap();
        assert!(output.status.success(), "{manager} --version failed");
        let version = String::from_utf8(output.stdout).unwrap();
        let version = version.trim();
        for lockfile_only in [false, true] {
            let root = dir.path().join(format!("{manager}-{lockfile_only}"));
            std::fs::create_dir(&root).unwrap();
            let fixture = root.join("fixture");
            std::fs::create_dir(&fixture).unwrap();
            // Fail if lifecycle scripts run.
            let scripts = r#""scripts":{"postinstall":"node -e \"process.exit(1)\""}"#;
            std::fs::write(
                fixture.join("package.json"),
                format!(r#"{{"name":"fixture","version":"1.0.0",{scripts}}}"#),
            )
            .unwrap();
            let package_manager = if manager == "npm" {
                String::new()
            } else {
                format!(r#""packageManager":"{manager}@{version}","#)
            };
            let manifest = format!(
                r#"{{"name":"test-{manager}","version":"1.0.0",{package_manager}"dependencies":{{"fixture":"file:./fixture"}},{scripts}}}"#
            );
            std::fs::write(root.join("package.json"), &manifest).unwrap();
            let mut command = Command::new(env!("CARGO_BIN_EXE_check-updates"));
            command
                .args(["--root", root.to_str().unwrap(), "-U"])
                .env("CI", "true");
            if lockfile_only {
                command.arg("--lockfile-only");
            }
            let output = command.output().unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert!(
                output.status.success(),
                "{manager} {version} (lockfile_only={lockfile_only}): {}\n{stderr}",
                String::from_utf8_lossy(&output.stdout)
            );
            assert!(root.join(lockfile).is_file());
            assert_eq!(root.join("node_modules").is_dir(), !lockfile_only);
            assert_eq!(
                std::fs::read_to_string(root.join("package.json")).unwrap(),
                manifest
            );
        }
    }
}
