use std::path::PathBuf;

use check_updates::{CheckUpdates, DepKind, Options, Unit};

#[tokio::test]
async fn discovers_workspace_dependencies() {
    let packages = CheckUpdates::try_new(
        Some(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")),
        Options::default(),
    )
    .unwrap()
    .packages()
    .await
    .unwrap();
    let entries: Vec<_> = packages.values().flatten().collect();
    let serde = &entries
        .iter()
        .find(|(_, _, package)| package.purl.name() == "serde")
        .expect("serde should be in packages")
        .2;
    assert!(
        serde.repository.is_some(),
        "serde should have a repository URL"
    );
    assert!(!serde.usages.is_empty());
    for usage in &serde.usages {
        assert!(matches!(&usage.unit, Unit::Workspace { .. }));
        assert!(usage.unit.path().unwrap().ends_with("Cargo.toml"));
    }
    assert!(
        entries
            .iter()
            .any(|(_, kind, package)| package.purl.name() == "semver" && *kind == DepKind::Normal)
    );
}

#[tokio::test]
async fn distinguishes_workspace_inheritance() {
    let root =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../examples/cargo/workspace-demo");
    let packages = CheckUpdates::try_new(Some(root), Options::default())
        .unwrap()
        .packages()
        .await
        .unwrap();
    let entries: Vec<_> = packages.values().flatten().collect();
    let rand = &entries
        .iter()
        .find(|(_, _, package)| package.purl.name() == "rand")
        .expect("rand should be in packages")
        .2;
    let renamed_usages: Vec<_> = rand
        .usages
        .iter()
        .filter(|usage| usage.rename.as_deref() == Some("rand07"))
        .collect();
    assert!(!renamed_usages.is_empty());
    assert!(
        renamed_usages
            .iter()
            .all(|usage| matches!(usage.unit, Unit::Project { .. }))
    );

    for (name, inherited) in [("anyhow", true), ("tokio", false)] {
        let package = &entries
            .iter()
            .find(|(_, _, package)| package.purl.name() == name)
            .unwrap_or_else(|| panic!("{name} should be in packages"))
            .2;
        assert!(
            package.usages.iter().any(|usage| match usage.unit {
                Unit::Workspace { .. } => inherited,
                Unit::Project { .. } => !inherited,
                Unit::Global => false,
            }),
            "{name}, inherited={inherited}"
        );
    }
}
