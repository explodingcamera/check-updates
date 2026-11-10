use std::process::Command;

#[test]
fn handles_help_version_and_invalid_options() {
    let dir = tempfile::tempdir().unwrap();
    for (binary, prefix) in [
        (env!("CARGO_BIN_EXE_check-updates"), &[][..]),
        (
            env!("CARGO_BIN_EXE_cargo-check-updates"),
            &["check-updates"][..],
        ),
    ] {
        for (args, status, expected) in [
            (&["--help"][..], 0, "Usage:"),
            (&["-h"][..], 0, "Usage:"),
            (&["--version"][..], 0, env!("CARGO_PKG_VERSION")),
            (&["-V"][..], 0, env!("CARGO_PKG_VERSION")),
            (&["-iUV"][..], 0, env!("CARGO_PKG_VERSION")),
            (&["-iU"][..], 1, "No Cargo.toml or package.json found"),
            (
                &["--cache", "invalid"][..],
                2,
                "expected prefer-local, refresh, or no-cache",
            ),
            (
                &["--lockfile-only"][..],
                2,
                "--lockfile-only requires --upgrade",
            ),
        ] {
            let output = Command::new(binary)
                .args(prefix)
                .args(args)
                .current_dir(dir.path())
                .env("NO_COLOR", "1")
                .output()
                .unwrap();
            assert_eq!(output.status.code(), Some(status), "{args:?}");
            let text = String::from_utf8_lossy(if status == 0 {
                &output.stdout
            } else {
                &output.stderr
            });
            assert!(text.contains(expected), "{args:?}: {text}");
            assert!(!text.contains('\x1b'), "{text:?}");
        }
    }
}

#[test]
fn generates_shell_completions() {
    for shell in ["bash", "fish", "zsh", "elvish"] {
        let output = Command::new(env!("CARGO_BIN_EXE_check-updates"))
            .args(["generate-shell-completion", shell])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let script = String::from_utf8(output.stdout).unwrap();
        assert!(script.contains("check-updates"), "{shell}");
        assert!(script.contains("--bpaf-complete-rev="), "{shell}");
    }
    let output = Command::new(env!("CARGO_BIN_EXE_check-updates"))
        .args(["--bpaf-complete-rev=0", "--lock"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--lockfile-only"));
}
