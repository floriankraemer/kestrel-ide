//! Running a test framework's process and turning its streamed output into
//! tree events (D2/D4).
//!
//! Mirrors `build_core::runner`'s shape — blocking by design, the caller
//! (`ui-shell`'s `TestServiceRust`, D4) gives it a thread and turns its
//! callbacks into Qt signals — but spawns over pipes
//! (`process_exec::spawn`) rather than `run_core::Supervisor`'s PTY, for
//! the plan's stated reason: a tty hard-wraps output, and a TeamCity
//! service message split mid-line by an 80-column wrap parses as garbage.
//!
//! `process_exec::run` (collect-to-completion) is the wrong primitive
//! here — a caller that wants "the tree fills while the run is in flight"
//! cannot use a function that returns after the process has already
//! exited, which is exactly why `process_exec::spawn` exists.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use crate::junit::JUnitTestCase;
use crate::teamcity::{TeamCityEvent, TeamCityParser};

/// What a caller wants to hear while a run is in flight.
pub trait TestSink {
    /// A chunk of raw stdout, exactly as the process wrote it (colours and
    /// all — `run_core::AnsiStripper` is applied by the view layer per the
    /// plan's "Reuse, not reinvention" section, not here).
    fn output(&mut self, text: &str);
    /// One parsed TeamCity service message, as it streams in.
    fn event(&mut self, event: TeamCityEvent);
    /// Every case parsed from a `junit-xml` run's report files, delivered
    /// once after the process has exited (C1). A `teamcity` run never calls
    /// this — the two output formats are mutually exclusive per run.
    fn junit(&mut self, cases: Vec<JUnitTestCase>);
}

/// Which shape a test framework's results arrive in
/// (`TestFrameworkContribution::output_format`, jvm-build-tools plan C1):
/// `teamcity` streams incrementally while the process runs; `junit_xml` has
/// no per-test timeline, only a batch of report files [`run`] reads once the
/// process exits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    TeamCity,
    JunitXml,
}

/// An `output-format` string a manifest names that no runner in this build
/// understands — a typed error the caller can turn into a Problems row or a
/// refused run, never a silent fallback to `teamcity` and never a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownOutputFormat(pub String);

impl std::fmt::Display for UnknownOutputFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "unknown test output format: {}", self.0)
    }
}

impl std::error::Error for UnknownOutputFormat {}

/// Parse a `TestFrameworkContribution::output_format` string into the
/// enum [`run`] dispatches on.
pub fn parse_output_format(value: &str) -> Result<OutputFormat, UnknownOutputFormat> {
    match value {
        "teamcity" => Ok(OutputFormat::TeamCity),
        "junit-xml" => Ok(OutputFormat::JunitXml),
        other => Err(UnknownOutputFormat(other.to_string())),
    }
}

/// Every path under `work_dir` matching glob `pattern` (e.g.
/// `**/target/{surefire,failsafe}-reports/TEST-*.xml`), matched against the
/// path relative to `work_dir` so the pattern never has to know the
/// project's absolute location. Walked by hand rather than pulling in a
/// `walkdir` dependency — this crate has none today and the recursion is a
/// handful of lines.
fn glob_matches(work_dir: &Path, pattern: &str) -> Vec<PathBuf> {
    let Ok(glob) = globset::Glob::new(pattern) else {
        return Vec::new();
    };
    let matcher = glob.compile_matcher();
    let mut found = Vec::new();
    walk_matching(work_dir, work_dir, &matcher, &mut found);
    found
}

fn walk_matching(
    root: &Path,
    dir: &Path,
    matcher: &globset::GlobMatcher,
    found: &mut Vec<PathBuf>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk_matching(root, &path, matcher, found);
        } else if let Ok(relative) = path.strip_prefix(root) {
            if matcher.is_match(relative) {
                found.push(path);
            }
        }
    }
}

/// Some filesystems (notably a bind mount backed by a coarser host clock)
/// truncate an mtime to whole seconds, so a report written a few
/// milliseconds after `run_started_at` can round down to *before* it. A
/// grace window this small still rejects anything genuinely stale — a
/// leftover report from a previous run is seconds to hours old, never
/// within a couple of seconds of "now".
const STALE_GRACE: std::time::Duration = std::time::Duration::from_secs(2);

/// Drop any report file whose mtime is older than `run_started_at` (minus
/// [`STALE_GRACE`]) — a stale Surefire/Failsafe report a previous run left
/// under the same `target/` directory must never be misread as this run's
/// own output. A file whose mtime cannot be read at all is dropped too,
/// rather than guessed to be fresh.
fn exclude_stale(paths: Vec<PathBuf>, run_started_at: SystemTime) -> Vec<PathBuf> {
    let cutoff = run_started_at
        .checked_sub(STALE_GRACE)
        .unwrap_or(run_started_at);
    paths
        .into_iter()
        .filter(|path| {
            std::fs::metadata(path)
                .and_then(|meta| meta.modified())
                .is_ok_and(|mtime| mtime >= cutoff)
        })
        .collect()
}

/// Read and parse every surviving `junit-xml` report under `work_dir`
/// matching `report_glob`, in a stable (sorted-path) order so a caller's
/// resulting tree is deterministic across runs. A file that fails to parse
/// is skipped rather than failing the whole batch — one malformed report
/// should not hide every other test's result.
fn collect_junit_cases(
    work_dir: &Path,
    report_glob: &str,
    run_started_at: SystemTime,
) -> Vec<JUnitTestCase> {
    let mut paths = exclude_stale(glob_matches(work_dir, report_glob), run_started_at);
    paths.sort();
    paths
        .iter()
        .filter_map(|path| std::fs::read_to_string(path).ok())
        .filter_map(|xml| crate::junit::parse(&xml).ok())
        .flatten()
        .collect()
}

/// A handle another thread can use to stop a running test process.
#[derive(Clone, Default)]
pub struct TestRunHandle {
    spawned: Arc<Mutex<Option<process_exec::Spawned>>>,
}

impl TestRunHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Kill the running process, if any. Stopping a run that has already
    /// finished (or never started) is not an error.
    pub fn stop(&self) {
        if let Ok(guard) = self.spawned.lock() {
            if let Some(spawned) = guard.as_ref() {
                spawned.kill();
            }
        }
    }
}

/// Why a run could not be started.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RunFailure {
    NotFound,
    Io(String),
}

/// Spawn `program args` in `work_dir`, streaming stdout through a
/// [`TeamCityParser`] and reporting every event and every raw chunk to
/// `sink` as they arrive. Blocks until the process exits or
/// [`TestRunHandle::stop`] kills it.
///
/// When `format` is [`OutputFormat::JunitXml`], `report_glob` (required by
/// the manifest for that format, but taken as `Option` here so a caller
/// with a malformed contribution degrades to "no reports read" instead of
/// panicking) is globbed under `work_dir` once the process has exited, and
/// every surviving report's cases are delivered through
/// [`TestSink::junit`] in one call — no per-test timeline exists for this
/// format, unlike `teamcity`'s incremental [`TestSink::event`].
///
/// Returns the exit code, or `None` when the process could not be waited
/// on (most commonly: it was just stopped).
pub fn run(
    handle: &TestRunHandle,
    program: &str,
    args: &[String],
    work_dir: &Path,
    format: OutputFormat,
    report_glob: Option<&str>,
    sink: &mut dyn TestSink,
) -> Result<Option<i32>, RunFailure> {
    let run_started_at = SystemTime::now();
    let arg_refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let spawned = process_exec::spawn(program, &arg_refs, work_dir).map_err(|e| match e {
        process_exec::Failure::NotFound => RunFailure::NotFound,
        process_exec::Failure::Io(msg) => RunFailure::Io(msg),
        // `spawn` never blocks waiting for exit, so it has no timeout to
        // report; kept as an arm rather than matched away so a future
        // change to `process_exec::Failure` is a compile error here, not a
        // silent gap.
        process_exec::Failure::TimedOut => RunFailure::Io("unexpected timeout".into()),
    })?;
    *handle
        .spawned
        .lock()
        .map_err(|_| RunFailure::Io("run handle lock poisoned".into()))? = Some(spawned.clone());

    let mut stdout = spawned.take_stdout().expect("stdout was piped");
    let mut stderr = spawned.take_stderr().expect("stderr was piped");

    // Drained on its own thread purely to avoid the pipe-fills-and-blocks
    // deadlock `process_exec::run` avoids the same way. A test framework's
    // own diagnostic chatter belongs on stdout in every format this plan
    // supports, so stderr is captured wholesale rather than streamed to
    // `sink` — an acceptable v1 simplification, not a silent data loss:
    // nothing observed on stdout is affected by it.
    let stderr_reader = std::thread::spawn(move || {
        let mut buffer = Vec::new();
        let _ = stderr.read_to_end(&mut buffer);
        buffer
    });

    let mut parser = TeamCityParser::new();
    let mut buffer = [0u8; 8192];
    loop {
        let read = match stdout.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let chunk = String::from_utf8_lossy(&buffer[..read]).into_owned();
        for event in parser.feed(&chunk) {
            sink.event(event);
        }
        sink.output(&chunk);
    }
    for event in parser.finish() {
        sink.event(event);
    }
    let _ = stderr_reader.join();

    let exit_code = spawned.wait().ok().and_then(|status| status.code());

    *handle
        .spawned
        .lock()
        .map_err(|_| RunFailure::Io("run handle lock poisoned".into()))? = None;

    if let (OutputFormat::JunitXml, Some(pattern)) = (format, report_glob) {
        sink.junit(collect_junit_cases(work_dir, pattern, run_started_at));
    }

    Ok(exit_code)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn events_arrive_as_the_process_writes_them() {
        let dir = tempfile::tempdir().unwrap();
        let handle = TestRunHandle::new();
        let mut collected = Collected::default();
        let script = "echo \"##teamcity[testStarted name='t']\"; \
                       echo \"##teamcity[testFinished name='t' duration='1']\"";
        let code = run(
            &handle,
            "sh",
            &["-c".into(), script.into()],
            dir.path(),
            OutputFormat::TeamCity,
            None,
            &mut collected,
        )
        .unwrap();
        assert_eq!(code, Some(0));
        assert_eq!(collected.events.len(), 2);
        assert!(collected.output.contains("teamcity"));
    }

    #[test]
    fn a_missing_program_is_reported_as_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let handle = TestRunHandle::new();
        let mut collected = Collected::default();
        let err = run(
            &handle,
            "this-binary-does-not-exist-anywhere",
            &[],
            dir.path(),
            OutputFormat::TeamCity,
            None,
            &mut collected,
        )
        .unwrap_err();
        assert!(matches!(err, RunFailure::NotFound));
    }

    #[test]
    fn stopping_a_run_ends_it_before_completion() {
        let dir = tempfile::tempdir().unwrap();
        let handle = TestRunHandle::new();
        let stopper = handle.clone();
        let stop_thread = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(50));
            stopper.stop();
        });
        let mut collected = Collected::default();
        let code = run(
            &handle,
            "sh",
            &["-c".into(), "sleep 5".into()],
            dir.path(),
            OutputFormat::TeamCity,
            None,
            &mut collected,
        )
        .unwrap();
        stop_thread.join().unwrap();
        assert_ne!(code, Some(0));
    }

    // W2-3: `run` calls `process_exec::spawn` with `work_dir` verbatim, no
    // knowledge of hosts at all — `spawn` classifies `work_dir` itself
    // (W1-7), so a WSL project's test run reaches the distro without a
    // line changing here. Proven the same way `process-exec`'s own tests
    // stand in for a real `wsl.exe`: a fake script on `PATH`, this one
    // emitting TeamCity service messages, showing streamed stdout parses
    // exactly as it would locally.
    #[test]
    fn a_remote_work_dir_streams_teamcity_output_through_wsl_exe_unchanged() {
        use std::io::Write;
        use std::os::unix::fs::PermissionsExt;
        use std::sync::Mutex;
        static PATH_LOCK: Mutex<()> = Mutex::new(());
        let _guard = PATH_LOCK.lock().unwrap();

        let bin_dir = tempfile::tempdir().unwrap();
        let script_path = bin_dir.path().join("wsl.exe");
        {
            let mut script = std::fs::File::create(&script_path).unwrap();
            write!(
                script,
                "#!/bin/sh\n\
                 for arg in \"$@\"; do\n\
                 \x20\x20if [ \"$arg\" = \"-lc\" ]; then echo some-test-runner; exit 0; fi\n\
                 done\n\
                 echo \"##teamcity[testStarted name='t']\"\n\
                 echo \"##teamcity[testFinished name='t' duration='1']\"\n"
            )
            .unwrap();
        }
        let mut perms = std::fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script_path, perms).unwrap();

        // Not created on disk, and deliberately so: the WSL root is only
        // ever parsed for its UNC spelling — wsl.exe is handed the distro path it names, and
        // the process itself is spawned from a local directory. Creating it
        // would write `/wsl.localhost` at the filesystem root, which only
        // succeeds when the test runs as root (see #251, #252 for the same
        // trap in analysis-core and lsp-core).
        let work_dir = Path::new("//wsl.localhost/Ubuntu/tmp/test-core-e2e");

        let original_path = std::env::var("PATH").unwrap_or_default();
        // SAFETY: serialized by PATH_LOCK.
        unsafe {
            std::env::set_var(
                "PATH",
                format!("{}:{original_path}", bin_dir.path().display()),
            );
        }
        let handle = TestRunHandle::new();
        let mut collected = Collected::default();
        let code = run(
            &handle,
            "some-test-runner",
            &[],
            work_dir,
            OutputFormat::TeamCity,
            None,
            &mut collected,
        );
        unsafe {
            std::env::set_var("PATH", original_path);
        }

        assert_eq!(code.unwrap(), Some(0));
        assert_eq!(collected.events.len(), 2);
    }

    #[test]
    fn parse_output_format_accepts_the_two_known_values() {
        assert_eq!(parse_output_format("teamcity"), Ok(OutputFormat::TeamCity));
        assert_eq!(parse_output_format("junit-xml"), Ok(OutputFormat::JunitXml));
    }

    #[test]
    fn parse_output_format_rejects_an_unknown_value_as_a_typed_error_not_a_panic() {
        let err = parse_output_format("checkstyle-xml").unwrap_err();
        assert_eq!(err, UnknownOutputFormat("checkstyle-xml".to_string()));
    }

    fn junit_xml(name: &str) -> String {
        format!(
            "<testsuite name=\"{name}\"><testcase name=\"a\" time=\"0.1\"/>\
             <testcase name=\"b\" time=\"0.2\"><failure message=\"boom\"/></testcase></testsuite>"
        )
    }

    /// Both Surefire's (`target/surefire-reports`) and Failsafe's
    /// (`target/failsafe-reports`) report directory shapes match the
    /// manifest's one `{surefire,failsafe}` brace-alternate pattern.
    #[test]
    fn glob_matches_both_surefire_and_failsafe_report_directories() {
        let dir = tempfile::tempdir().unwrap();
        let surefire = dir.path().join("target/surefire-reports");
        let failsafe = dir.path().join("target/failsafe-reports");
        std::fs::create_dir_all(&surefire).unwrap();
        std::fs::create_dir_all(&failsafe).unwrap();
        std::fs::write(surefire.join("TEST-ATest.xml"), junit_xml("ATest")).unwrap();
        std::fs::write(failsafe.join("TEST-BIT.xml"), junit_xml("BIT")).unwrap();
        std::fs::write(surefire.join("not-a-report.txt"), "ignore me").unwrap();

        let pattern = "**/target/{surefire,failsafe}-reports/TEST-*.xml";
        let mut found = glob_matches(dir.path(), pattern);
        found.sort();
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn a_report_older_than_the_run_start_is_excluded_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let report = dir.path().join("stale.xml");
        std::fs::write(&report, junit_xml("Stale")).unwrap();
        // Force the mtime to well before "now" so it reads as older than a
        // run that starts after this line, regardless of filesystem mtime
        // resolution.
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        let file = std::fs::File::open(&report).unwrap();
        file.set_modified(old).unwrap();

        let run_started_at = SystemTime::now();
        let kept = exclude_stale(vec![report], run_started_at);
        assert!(
            kept.is_empty(),
            "a report older than the run start must be dropped"
        );
    }

    #[test]
    fn a_report_written_after_the_run_started_is_kept() {
        let dir = tempfile::tempdir().unwrap();
        let run_started_at = SystemTime::now();
        let report = dir.path().join("fresh.xml");
        std::fs::write(&report, junit_xml("Fresh")).unwrap();

        let kept = exclude_stale(vec![report.clone()], run_started_at);
        assert_eq!(kept, vec![report]);
    }

    /// The end-to-end shape C1 adds: a fake `mvn`-like script writes a
    /// Surefire report only after it starts (so it is never stale), and a
    /// stale report from a *previous* run already sits under the same
    /// `target/surefire-reports` — the runner must read the fresh one and
    /// skip the stale one.
    #[test]
    fn a_junit_xml_run_delivers_only_fresh_reports_to_the_sink() {
        let dir = tempfile::tempdir().unwrap();
        let reports = dir.path().join("target/surefire-reports");
        std::fs::create_dir_all(&reports).unwrap();

        let stale_path = reports.join("TEST-OldTest.xml");
        std::fs::write(&stale_path, junit_xml("OldTest")).unwrap();
        let old = SystemTime::now() - std::time::Duration::from_secs(3600);
        std::fs::File::open(&stale_path)
            .unwrap()
            .set_modified(old)
            .unwrap();

        let fresh_report = reports.join("TEST-NewTest.xml");
        let script = format!(
            "cat > {} <<'EOF'\n{}\nEOF\n",
            fresh_report.display(),
            junit_xml("NewTest")
        );

        let handle = TestRunHandle::new();
        let mut collected = Collected::default();
        let code = run(
            &handle,
            "sh",
            &["-c".into(), script],
            dir.path(),
            OutputFormat::JunitXml,
            Some("**/target/surefire-reports/TEST-*.xml"),
            &mut collected,
        )
        .unwrap();
        assert_eq!(code, Some(0));

        let suites: Vec<&str> = collected
            .junit_cases
            .iter()
            .map(|c| c.suite.as_str())
            .collect();
        assert_eq!(
            suites,
            vec!["NewTest", "NewTest"],
            "only the fresh report's cases arrive"
        );
    }
}
