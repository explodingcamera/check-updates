use std::process::Command;

#[test]
#[ignore = "requires npm, pnpm, and bun"]
fn upgrade_with_installed_managers() {
    let dir = tempfile::tempdir().unwrap();
    for (manager, lockfile) in [
        ("npm", "package-lock.json"),
        ("pnpm", "pnpm-lock.yaml"),
        ("bun", "bun.lock"),
    ] {
        let root = dir.path().join(manager);
        std::fs::create_dir(&root).unwrap();
        let fixture = root.join("fixture");
        std::fs::create_dir(&fixture).unwrap();
        std::fs::write(
            fixture.join("package.json"),
            r#"{"name":"fixture","version":"1.0.0"}"#,
        )
        .unwrap();
        let version = Command::new(manager).arg("--version").output().unwrap();
        assert!(version.status.success(), "{manager} --version failed");
        let version = String::from_utf8(version.stdout).unwrap();
        std::fs::write(
            root.join("package.json"),
            format!(r#"{{"name":"test-{manager}","version":"1.0.0","packageManager":"{manager}@{}","dependencies":{{"fixture":"file:./fixture"}}}}"#, version.trim()),
        )
        .unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_check-updates"))
            .args(["--root", root.to_str().unwrap(), "-U", "--lockfile-only"])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{manager}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            root.join(lockfile).is_file(),
            "{manager} did not create {lockfile}"
        );
    }
}
