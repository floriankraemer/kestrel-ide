//! Runs the real `gradle`/`mvn` binaries `linux-jvm` ships against
//! jvm-build-core's fixture projects, proving `test_core::run` actually
//! drives the `junit-gradle`/`junit-maven` test-frameworks rows end to end
//! (jvm-build-tools plan C2, ADR-0057) — not just against checked-in
//! fixture *output*, the way `junit::tests` and `runner::tests` already do.
//!
//! Gated behind the `jvm-integration` feature, the same convention
//! `jvm-build-core`'s `gradle_integration.rs`/`maven_integration.rs` use:
//! never built or run by `cargo test --workspace`/`make test`, only
//! `make test-jvm` inside the `linux-jvm` image
//! (`docs/architecture/jvm-integration.md`).
//!
//! The argv and `report-glob` used here are copied from
//! `crates/plugin-host/builtin/jvm-build-tools/plugin.toml`'s
//! `junit-gradle`/`junit-maven` rows rather than loaded through
//! `plugin-api`/`plugin-host` — `test-core` depends on neither
//! (`docs/architecture/layering.md`), and a shell-out integration test is
//! exactly the place a literal copy of that argv earns its keep: if the
//! manifest and this test ever drift, `make test-jvm` is the thing that
//! notices.
#![cfg(feature = "jvm-integration")]

use std::path::{Path, PathBuf};

use test_core::{
    diagnostics_by_file, filter, run, JUnitTestCase, OutputFormat, TeamCityEvent, TestId,
    TestRunHandle, TestSink, TestStatus, TestTree,
};

/// jvm-build-core owns the fixture projects (phase A); no crate dependency
/// is needed to read them, just the sibling path — the two crates stay
/// mutually unaware of each other per the layering table.
fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../jvm-build-core/tests/fixtures")
        .join(name)
}

/// Directories a fixture project regenerates on every real `gradle`/`mvn`
/// run and never commits (`jvm-build-core`'s own `tests/fixtures/.gitignore`
/// lists the same three) — skipped on copy rather than dragged along stale
/// or, for `.gradle`, simply large.
const REGENERATED_DIRS: &[&str] = &[".gradle", "build", "target"];

/// A fresh, private copy of fixture `name` under its own `tempfile::tempdir()`
/// (review finding 5): `cargo-nextest` runs each test in its own process, so
/// two tests both running a real build tool directly in the *same* checked-
/// in fixture directory is a genuine race — build output collisions, Gradle
/// daemon lock contention on the same project, one test's `cleanTest`
/// wiping another's in-flight results. Copying first means concurrent tests
/// never share a working directory at all.
fn isolated_copy_of(fixture_name: &str) -> tempfile::TempDir {
    let dest = tempfile::tempdir().expect("temp dir for isolated fixture copy");
    copy_dir_contents(&fixture(fixture_name), dest.path());
    dest
}

/// `std::fs`-based recursive copy — this is test code, and a handful of
/// `read_dir` lines are simpler than adding a crate dependency for it.
/// Symlinks are skipped rather than followed, the same rule `runner.rs`'s
/// own walker now applies (review finding 2): none of these fixtures ship
/// one, but a copy helper that would follow one into a loop is not a trap
/// worth leaving for the next fixture that does.
fn copy_dir_contents(src: &Path, dest: &Path) {
    for entry in std::fs::read_dir(src).expect("read fixture dir") {
        let entry = entry.expect("read fixture dir entry");
        let file_type = entry.file_type().expect("read fixture entry file type");
        if file_type.is_symlink() {
            continue;
        }
        let dest_path = dest.join(entry.file_name());
        if file_type.is_dir() {
            if REGENERATED_DIRS.contains(&entry.file_name().to_string_lossy().as_ref()) {
                continue;
            }
            std::fs::create_dir_all(&dest_path).expect("create dir in fixture copy");
            copy_dir_contents(&entry.path(), &dest_path);
        } else {
            std::fs::copy(entry.path(), &dest_path).expect("copy file into fixture copy");
        }
    }
}

#[derive(Default)]
struct Collected {
    output: String,
    events: Vec<TeamCityEvent>,
    junit_cases: Vec<JUnitTestCase>,
}

impl TestSink for Collected {
    fn output(&mut self, text: &str) {
        self.output.push_str(text);
    }
    fn event(&mut self, event: TeamCityEvent) {
        self.events.push(event);
    }
    fn junit(&mut self, cases: Vec<JUnitTestCase>) {
        self.junit_cases = cases;
    }
}

/// The `gradle-single` fixture (A4) has one passing test (`greetsByName`)
/// and one deliberately failing one (`deliberatelyFails`) — see
/// `GreeterTest.java`. `cleanTest test`, `--init-script`,
/// `-Pide.teamcity=true`, `--console=plain` mirror the `junit-gradle` row
/// verbatim; the fixture ships no wrapper, so this runs the system
/// (wrapper-less) `gradle` the same way `gradle_integration.rs` does.
fn gradle_args(asset_dir: &Path) -> Vec<String> {
    vec![
        "--init-script".into(),
        asset_dir
            .join("ide-model.init.gradle")
            .to_string_lossy()
            .into_owned(),
        "-Pide.teamcity=true".into(),
        "--console=plain".into(),
        "cleanTest".into(),
        "test".into(),
    ]
}

fn write_init_script() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(
        dir.path().join("ide-model.init.gradle"),
        include_str!("../../plugin-host/builtin/jvm-build-tools/ide-model.init.gradle"),
    )
    .expect("write script");
    dir
}

fn run_gradle_test(work_dir: &Path) -> Collected {
    let asset_dir = write_init_script();
    let handle = TestRunHandle::new();
    let mut collected = Collected::default();
    let code = run(
        &handle,
        "gradle",
        &gradle_args(asset_dir.path()),
        work_dir,
        OutputFormat::TeamCity,
        None,
        &mut collected,
    )
    .expect("gradle test runs");
    // A failing test still exits non-zero; only assert the process was
    // actually reached (a real event stream is the real proof below).
    assert!(
        code.is_some(),
        "gradle process must have produced an exit code"
    );
    collected
}

#[test]
fn gradle_single_streams_one_pass_and_one_fail_via_teamcity() {
    let project = isolated_copy_of("gradle-single");
    let collected = run_gradle_test(project.path());

    let mut tree = TestTree::new();
    for event in collected.events {
        tree.apply(event);
    }
    let counts = tree.counts();
    assert_eq!(counts.passed, 1, "greetsByName must be reported passed");
    assert_eq!(
        counts.failed, 1,
        "deliberatelyFails must be reported failed"
    );

    let failing = tree.failing_nodes();
    assert_eq!(failing.len(), 1);
    assert!(!failing[0].failure.as_ref().unwrap().message.is_empty());
}

/// The `cleanTest` guard (phase A's review fix): without it, Gradle treats
/// a second `test` invocation as up-to-date and streams zero events —
/// which would read as "the project has no tests" rather than "nothing
/// changed". This proves the guard still holds after C1's changes.
#[test]
fn a_second_gradle_test_run_is_not_empty() {
    let project = isolated_copy_of("gradle-single");
    let first = run_gradle_test(project.path());
    let second = run_gradle_test(project.path());
    assert!(!first.events.is_empty());
    assert!(
        !second.events.is_empty(),
        "a second run must re-stream events, not read as zero tests (cleanTest guard)"
    );
}

/// The `maven-single` fixture (A5) has the same one-pass/one-fail shape as
/// its Gradle sibling. `-B test -Dsurefire.failIfNoSpecifiedTests=false`
/// and the `report-glob` mirror the `junit-maven` row verbatim; Surefire
/// writes `target/surefire-reports/TEST-*.xml` only after the test run,
/// never streamed, which is exactly why this framework uses `junit-xml`
/// rather than `teamcity`.
#[test]
fn maven_single_fills_the_tree_from_surefire_xml_and_the_failure_carries_a_message() {
    let project = isolated_copy_of("maven-single");
    let work_dir = project.path();

    let handle = TestRunHandle::new();
    let mut collected = Collected::default();
    let args = vec![
        "-B".to_string(),
        "test".to_string(),
        "-Dsurefire.failIfNoSpecifiedTests=false".to_string(),
    ];
    let _code = run(
        &handle,
        "mvn",
        &args,
        work_dir,
        OutputFormat::JunitXml,
        Some("**/target/{surefire,failsafe}-reports/TEST-*.xml"),
        &mut collected,
    )
    .expect("mvn test runs");

    assert_eq!(
        collected.junit_cases.len(),
        2,
        "one passing and one failing case, mvn output: {}",
        collected.output
    );

    let mut tree = TestTree::new();
    tree.apply_junit(&collected.junit_cases);
    let counts = tree.counts();
    assert_eq!(counts.passed, 1);
    assert_eq!(counts.failed, 1);

    let failing_case = collected
        .junit_cases
        .iter()
        .find(|c| c.status == TestStatus::Failed)
        .expect("a failing case");
    assert!(!failing_case.failure.as_ref().unwrap().message.is_empty());

    // C2's "a failing test becomes a diagnostic" requirement, at the same
    // tree -> diagnostics conversion `ui-shell`'s `republish` already calls
    // for the TeamCity path — proves the JUnit-XML path feeds the same
    // Problems surface, not a parallel one. Review finding 3: against a
    // real (untrimmed) Surefire report, the diagnostic must land on the
    // test's own `GreeterTest.java:16` assertion call site, not the
    // trace's last line (a JDK-internal frame Surefire never trims).
    let grouped = diagnostics_by_file(&tree, "junit-maven", &work_dir);
    assert_eq!(grouped.len(), 1, "exactly one file must carry a diagnostic");
    let (uri, diagnostics) = grouped.iter().next().unwrap();
    assert!(
        uri.ends_with("GreeterTest.java"),
        "diagnostic must resolve to the real test source file, got: {uri}"
    );
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(
        diagnostics[0].range.start.line, 15,
        "GreeterTest.java's `assertEquals` call for deliberatelyFails is on line 16 \
         (0-indexed 15), not a JDK-internal trace line"
    );
}

/// Review finding 1: a rerun built in PHPUnit's PCRE dialect against Gradle
/// compiles fine and matches zero tests. This proves the `gradle` dialect
/// (`filter::for_node` + `filter::apply_filter`) actually narrows a real
/// `gradle --tests` invocation to exactly the one failing node, both
/// `greetsByName` (still run, since `cleanTest` always re-runs) excluded.
#[test]
fn rerunning_one_failing_gradle_node_runs_exactly_that_one_test() {
    let project = isolated_copy_of("gradle-single");
    let asset_dir = write_init_script();

    let id = TestId("com.example.GreeterTest::deliberatelyFails".to_string());
    let patterns = filter::for_node(&TestTree::new(), &id, filter::FilterDialect::Gradle);
    let mut args = gradle_args(asset_dir.path());
    args = filter::apply_filter(&args, Some("--tests"), None, &patterns);

    let handle = TestRunHandle::new();
    let mut collected = Collected::default();
    let code = run(
        &handle,
        "gradle",
        &args,
        project.path(),
        OutputFormat::TeamCity,
        None,
        &mut collected,
    )
    .expect("gradle test runs");
    assert!(code.is_some());

    let mut tree = TestTree::new();
    for event in collected.events {
        tree.apply(event);
    }
    let counts = tree.counts();
    assert_eq!(
        counts.passed + counts.failed,
        1,
        "exactly one test must run, not the whole suite: {}",
        collected.output
    );
    assert_eq!(counts.failed, 1, "the one test run must be the failing one");
}

/// Same review finding, Maven side: a `-Dtest=` pattern built in PHPUnit's
/// dialect (or Gradle's) silently matches nothing, and with
/// `-Dsurefire.failIfNoSpecifiedTests=false` the run then "passes" having
/// run zero tests. This proves the `surefire` dialect
/// (`Class#method`) actually narrows a real `mvn -Dtest=` rerun to exactly
/// the one failing node.
#[test]
fn rerunning_one_failing_maven_node_runs_exactly_that_one_test() {
    let project = isolated_copy_of("maven-single");

    let id = TestId("com.example.GreeterTest::deliberatelyFails".to_string());
    let patterns = filter::for_node(&TestTree::new(), &id, filter::FilterDialect::Surefire);
    let base_args = vec![
        "-B".to_string(),
        "test".to_string(),
        "-Dsurefire.failIfNoSpecifiedTests=false".to_string(),
    ];
    let args = filter::apply_filter(&base_args, None, Some("-Dtest={pattern}"), &patterns);

    let handle = TestRunHandle::new();
    let mut collected = Collected::default();
    let _code = run(
        &handle,
        "mvn",
        &args,
        project.path(),
        OutputFormat::JunitXml,
        Some("**/target/{surefire,failsafe}-reports/TEST-*.xml"),
        &mut collected,
    )
    .expect("mvn test runs");

    assert_eq!(
        collected.junit_cases.len(),
        1,
        "exactly one test case must come back, not the whole suite (or zero, if the \
         dialect were wrong): mvn output: {}",
        collected.output
    );
    assert_eq!(collected.junit_cases[0].name, "deliberatelyFails");
    assert_eq!(collected.junit_cases[0].status, TestStatus::Failed);
}
