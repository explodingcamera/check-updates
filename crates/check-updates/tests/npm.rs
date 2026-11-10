use check_updates::npm::{NpmOptions, PackageManager, package_manager, packages, update_packages};
use check_updates::{DepKind, Package, Requirement, Unit, Usage};

#[test]
fn applies_selected_package_requirement() {
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("package.json");
    std::fs::write(
        &manifest,
        r#"{"dependencies":{"foo":"^1.0.0"},"devDependencies":{"foo":"^1.0.0"}}"#,
    )
    .unwrap();
    let req = Requirement::from_node("^1.0.0").unwrap();
    let unit = Unit::Project {
        manifest: manifest.clone(),
        name: "app".into(),
    };
    let usage = Usage {
        unit,
        req,
        kind: DepKind::Dev,
        rename: None,
        supported_toolchain_version: None,
    };
    let package = Package {
        purl: "pkg:npm/foo".parse().unwrap(),
        usages: vec![usage.clone()],
        versions: Vec::new(),
        repository: None,
        homepage: None,
    };
    update_packages([(&usage, &package, Requirement::from_node("^2.0.0").unwrap())]).unwrap();
    let contents = std::fs::read_to_string(manifest).unwrap();
    assert!(contents.contains(r#""dependencies":{"foo":"^1.0.0"}"#));
    assert!(contents.contains(r#""devDependencies":{"foo":"^2.0.0"}"#));
}

#[test]
fn updates_workspace_root_and_member_separately() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("package.json");
    let member = dir.path().join("packages/app/package.json");
    std::fs::create_dir_all(member.parent().unwrap()).unwrap();
    for manifest in [&root, &member] {
        std::fs::write(manifest, r#"{"dependencies":{"foo":"^1.0.0"}}"#).unwrap();
    }
    let requirement = Requirement::from_node("^1.0.0").unwrap();
    let root_usage = Usage {
        unit: Unit::Workspace {
            manifest: root.clone(),
        },
        req: requirement.clone(),
        kind: DepKind::Normal,
        rename: None,
        supported_toolchain_version: None,
    };
    let member_usage = Usage {
        unit: Unit::Project {
            manifest: member.clone(),
            name: "app".into(),
        },
        ..root_usage.clone()
    };
    let package = Package {
        purl: "pkg:npm/foo".parse().unwrap(),
        usages: Vec::new(),
        versions: Vec::new(),
        repository: None,
        homepage: None,
    };
    assert!(root_usage.unit < member_usage.unit);
    update_packages([(
        &member_usage,
        &package,
        Requirement::from_node("^2.0.0").unwrap(),
    )])
    .unwrap();
    assert!(std::fs::read_to_string(&root).unwrap().contains("^1.0.0"));
    assert!(std::fs::read_to_string(&member).unwrap().contains("^2.0.0"));
}

#[tokio::test]
async fn discovers_bun_workspace_from_package_json() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let app = root.join("apps/app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"root","packageManager":"bun@1.2.0","workspaces":{"packages":["apps/*"]}}"#,
    )
    .unwrap();
    std::fs::write(app.join("package.json"), r#"{"name":"app"}"#).unwrap();
    std::fs::write(root.join("bun.lock"), "").unwrap();
    assert_eq!(package_manager(root).unwrap(), PackageManager::Bun);

    let found = packages(root, &NpmOptions::default()).await.unwrap();
    assert_eq!(found.projects, ["root", "app"]);
}

#[test]
fn detects_lockfile_before_package_manager_field() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("package.json"), r#"{"packageManager":"npm@10"}"#).unwrap();
    assert_eq!(package_manager(root).unwrap(), PackageManager::Npm);
    std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(package_manager(root).unwrap(), PackageManager::Pnpm);
    std::fs::write(root.join("bun.lock"), "").unwrap();
    assert!(package_manager(root).is_err());
}

#[test]
fn defaults_to_npm_without_known_manager() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::write(root.join("package.json"), "{}").unwrap();
    assert_eq!(package_manager(root).unwrap(), PackageManager::Npm);
    std::fs::write(root.join("aube.lock"), "").unwrap();
    assert_eq!(package_manager(root).unwrap(), PackageManager::Npm);

    std::fs::write(root.join("pnpm-lock.yaml"), "").unwrap();
    assert_eq!(package_manager(root).unwrap(), PackageManager::Pnpm);
}

#[test]
fn rejects_unsupported_package_manager() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("package.json"),
        r#"{"packageManager":"unsupported@1.0.0"}"#,
    )
    .unwrap();
    assert_eq!(
        package_manager(dir.path()).unwrap_err(),
        "unsupported package manager 'unsupported'"
    );
}

#[tokio::test]
async fn discovers_pnpm_workspace_without_package_json_workspaces() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("[workspace]");
    let app = root.join("packages/app");
    std::fs::create_dir_all(&app).unwrap();
    std::fs::write(root.join("package.json"), r#"{"name":"root"}"#).unwrap();
    std::fs::write(
        root.join("pnpm-workspace.yaml"),
        "packages:\n  - 'packages/*'\n",
    )
    .unwrap();
    std::fs::write(app.join("package.json"), r#"{"name":"app"}"#).unwrap();
    let found = packages(&root, &NpmOptions::default()).await.unwrap();
    assert_eq!(found.projects, ["root", "app"]);
}

#[cfg(unix)]
#[tokio::test]
async fn excludes_workspace_patterns_and_external_symlinks() {
    let dir = tempfile::tempdir().unwrap();
    let external = tempfile::tempdir().unwrap();
    let root = dir.path();
    std::fs::create_dir_all(root.join("apps/app")).unwrap();
    std::fs::create_dir_all(root.join("apps/skip")).unwrap();
    std::fs::write(
        root.join("package.json"),
        r#"{"name":"root","workspaces":["apps/*","!apps/skip"]}"#,
    )
    .unwrap();
    for (folder, name) in [("apps/app", "app"), ("apps/skip", "skip")] {
        std::fs::write(
            root.join(folder).join("package.json"),
            format!(r#"{{"name":"{name}"}}"#),
        )
        .unwrap();
    }
    std::fs::write(
        external.path().join("package.json"),
        r#"{"name":"external"}"#,
    )
    .unwrap();
    std::os::unix::fs::symlink(external.path(), root.join("apps/external")).unwrap();

    let found = packages(root, &NpmOptions::default()).await.unwrap();
    assert_eq!(found.projects, ["root", "app"]);
}
