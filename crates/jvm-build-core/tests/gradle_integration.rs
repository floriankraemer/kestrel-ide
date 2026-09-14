//! Runs the real, system (wrapper-less) `gradle` binary against the fixture
//! projects under `tests/fixtures/gradle-*` (A4). Gated behind the
//! `jvm-integration` feature so `cargo test --workspace`/`make test` never
//! builds or runs this file — only `make test-jvm` inside the `linux-jvm`
//! image does (ADR-0057, `docs/architecture/jvm-integration.md`).
#![cfg(feature = "jvm-integration")]

use std::path::{Path, PathBuf};
use std::time::Duration;

use jvm_build_core::gradle::{init_script, run_sync, SyncOptions};
use jvm_build_core::model::{SourceContent, SourceRootKind, Tool};

fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// The real script `plugin-host` ships, reached the same
/// include-not-copy way `init_script::SOURCE_FOR_TESTS` does, but written
/// to a real temp file here — a sync needs a path, not a `&str`, the same
/// requirement `LoadedPlugin::asset_dir` exists to satisfy for the running
/// editor (A3).
fn write_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join(init_script::ASSET_NAME),
        include_str!("../../plugin-host/builtin/jvm-build-tools/ide-model.init.gradle"),
    )
    .expect("write script");
    dir
}

fn opts() -> SyncOptions {
    SyncOptions {
        offline: false,
        timeout: Some(Duration::from_secs(180)),
    }
}

#[test]
fn gradle_single_syncs_and_reports_its_java_test_source_roots() {
    let script_dir = write_script();
    let script_path = script_dir.path().join(init_script::ASSET_NAME);
    let model = run_sync(&fixture("gradle-single"), &script_path, &opts()).expect("sync succeeds");

    assert_eq!(model.tool, Tool::Gradle);
    assert_eq!(model.modules.len(), 1);
    let root = &model.modules[0];
    assert_eq!(root.name, "gradle-single");

    let test_java = root
        .source_roots
        .iter()
        .find(|r| r.kind == SourceRootKind::Test && r.content == SourceContent::Java)
        .expect("a Java test source root");
    assert!(test_java.path.ends_with("src/test/java"));

    assert!(model.tasks.iter().any(|t| t.path == ":test"));
    assert!(root
        .dependencies
        .iter()
        .any(|d| d.artifact == "junit-jupiter"));
}

#[test]
fn gradle_multi_kts_reports_every_module_and_the_project_dependency() {
    let script_dir = write_script();
    let script_path = script_dir.path().join(init_script::ASSET_NAME);
    let model =
        run_sync(&fixture("gradle-multi-kts"), &script_path, &opts()).expect("sync succeeds");

    let mut paths: Vec<&str> = model.modules.iter().map(|m| m.path.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, vec![":", ":app", ":lib"]);

    let app = model.modules.iter().find(|m| m.path == ":app").unwrap();
    assert!(app
        .dependencies
        .iter()
        .any(|d| d.artifact == "project :lib"));
}

#[test]
fn gradle_catalog_resolves_a_version_catalog_dependency() {
    let script_dir = write_script();
    let script_path = script_dir.path().join(init_script::ASSET_NAME);
    let model = run_sync(&fixture("gradle-catalog"), &script_path, &opts()).expect("sync succeeds");

    let root = &model.modules[0];
    assert!(root
        .dependencies
        .iter()
        .any(|d| d.artifact == "junit-jupiter" && d.resolved == "5.10.3"));
}

/// A second sync of a project that changed nothing must produce the same
/// shape again — proves the temp-file-per-call scheme (`sync::model_out_path`)
/// never leaves a stale file a second run could accidentally read instead
/// of its own fresh one.
#[test]
fn a_second_sync_of_the_same_project_succeeds_again() {
    let script_dir = write_script();
    let script_path = script_dir.path().join(init_script::ASSET_NAME);
    let root = fixture("gradle-single");
    let first = run_sync(&root, &script_path, &opts()).expect("first sync");
    let second = run_sync(&root, &script_path, &opts()).expect("second sync");
    assert_eq!(first.modules.len(), second.modules.len());
}
