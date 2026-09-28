//! Runs the real `mvn` binary against the fixture projects under
//! `tests/fixtures/maven-*` (A5). Gated behind the `jvm-integration`
//! feature, same as `gradle_integration.rs` — never built or run by
//! `cargo test --workspace`/`make test`, only `make test-jvm` inside the
//! `linux-jvm` image.
#![cfg(feature = "jvm-integration")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use jvm_build_core::maven::{run_sync, SyncOptions};
use jvm_build_core::model::Tool;

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn opts() -> SyncOptions {
    SyncOptions {
        // Not `-o`/offline: the fixtures' JUnit 5 dependency needs a first,
        // online resolution the same way the Gradle fixtures do — see
        // gradle_integration.rs's identical note.
        offline: false,
        timeout: Some(Duration::from_secs(180)),
    }
}

#[test]
fn maven_single_syncs_its_coordinates_and_junit_dependency() {
    let model = run_sync(&fixture("maven-single"), &opts()).expect("sync succeeds");
    assert_eq!(model.tool, Tool::Maven);
    assert_eq!(model.modules.len(), 1);
    let module = &model.modules[0];
    assert_eq!(module.name, "maven-single");
    assert!(module
        .source_roots
        .iter()
        .any(|r| r.path.ends_with("src/main/java")));
    assert!(module
        .source_roots
        .iter()
        .any(|r| r.path.ends_with("src/test/java")));

    let junit = module
        .dependencies
        .iter()
        .find(|d| d.artifact == "junit-jupiter")
        .expect("junit-jupiter dependency");
    assert_eq!(junit.resolved, "5.10.3");
    assert_eq!(junit.scope, "test");

    assert!(model.tasks.iter().any(|t| t.path == "test"));
    assert!(model.tasks.iter().any(|t| t.path == "package"));
}

#[test]
fn maven_multi_syncs_both_child_modules_and_their_inter_module_dependency() {
    let model = run_sync(&fixture("maven-multi"), &opts()).expect("sync succeeds");
    assert_eq!(model.modules.len(), 2, "{:?}", model.modules);

    let app = model
        .modules
        .iter()
        .find(|m| m.name == "app")
        .expect("app module");
    assert!(app.dependencies.iter().any(|d| d.artifact == "lib"));
    let junit = app
        .dependencies
        .iter()
        .find(|d| d.artifact == "junit-jupiter")
        .expect("junit dependency, version resolved through dependencyManagement");
    assert_eq!(junit.resolved, "5.10.3");

    assert!(model.modules.iter().any(|m| m.name == "lib"));
}

/// B3 (review fix 2): the fixture's root pom declares one `<profile>`
/// (`ci`) with no other change — proves `BuildModel::profiles` actually
/// reaches a real sync's output, not only the static `pom::parse` unit
/// tests.
#[test]
fn maven_multi_reports_its_declared_profiles() {
    let model = run_sync(&fixture("maven-multi"), &opts()).expect("sync succeeds");
    assert_eq!(model.profiles, vec!["ci".to_string()]);
}

/// A second sync of the same project must succeed again — the effective-pom
/// temp file scheme (`sync::temp_file`) must never leave a stale file a
/// later run could collide with or accidentally read.
#[test]
fn a_second_sync_succeeds_again() {
    let root = fixture("maven-single");
    let first = run_sync(&root, &opts()).expect("first sync");
    let second = run_sync(&root, &opts()).expect("second sync");
    assert_eq!(first.modules.len(), second.modules.len());
}
