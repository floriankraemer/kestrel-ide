//! Running a build's steps and reading what they say (B2).
//!
//! Extracted from `ui-shell`'s `BuildService` when a second caller appeared:
//! a run configuration's Build before-launch task has to run exactly the
//! same steps and wait for exactly the same answer, and a second copy of
//! this loop in the adapter is how the two would drift.
//!
//! Blocking by design. Whoever calls this owns the thread — `BuildService`
//! gives a build its own, and the before-launch path runs it on the launch's
//! worker thread, where waiting is the point.

use std::io::Read;
use std::sync::{Arc, Mutex};

use run_core::toolchain::ToolchainId;
use run_core::{AnsiStripper, ConsoleId, LaunchSpec, Supervisor};

use crate::diagnostics::BuildDiagnostic;
use crate::parser::DiagnosticParser;

/// What a caller wants to hear while a build runs.
pub trait BuildSink {
    /// A chunk of output, ANSI already stripped. Never a whole line: build
    /// tools write when they feel like it.
    fn output(&mut self, text: &str);
    /// Problems, as they are recognised rather than at the end.
    fn diagnostics(&mut self, diagnostics: Vec<BuildDiagnostic>);
}

/// A handle another thread can use to stop a running build.
///
/// Cloneable and `Send`: the Qt thread holds one while the build's own
/// thread runs. Stopping kills the process *tree* — `cargo` spawns `rustc`
/// and `gradle` spawns a daemon, so killing one process would leave the
/// build running (ADR-0040).
#[derive(Clone, Default)]
pub struct BuildHandle {
    supervisor: Arc<Mutex<Supervisor>>,
    current: Arc<Mutex<Option<ConsoleId>>>,
}

impl BuildHandle {
    pub fn new() -> Self {
        Self::default()
    }

    /// Kill whatever step is running, if any. A build that has already
    /// finished is not an error to stop.
    pub fn stop(&self) {
        let Ok(current) = self.current.lock() else {
            return;
        };
        let Some(console) = *current else {
            return;
        };
        drop(current);
        if let Ok(mut supervisor) = self.supervisor.lock() {
            let _ = supervisor.stop(console);
        }
    }
}

/// The exit code of a finished build.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuildOutcome {
    /// The failing step's exit code, `0` when every step succeeded, or `-1`
    /// when a step could not be started at all.
    pub exit_code: i32,
}

impl BuildOutcome {
    pub fn succeeded(&self) -> bool {
        self.exit_code == 0
    }
}

/// Run `steps` in order, stopping at the first one that fails, reporting
/// output and diagnostics through `sink` as they arrive.
///
/// Blocks until the build ends or [`BuildHandle::stop`] kills it.
pub fn run(
    handle: &BuildHandle,
    steps: &[LaunchSpec],
    toolchain: ToolchainId,
    project_root: &std::path::Path,
    sink: &mut dyn BuildSink,
) -> BuildOutcome {
    for (index, step) in steps.iter().enumerate() {
        if index > 0 {
            sink.output(&format!("\n$ {}\n", command_line(step)));
        }
        let exit_code = match run_step(handle, step, toolchain, project_root, sink) {
            Ok(code) => code,
            Err(message) => {
                // The failure is written where the build's own output would
                // have been: "no such file or directory" is the answer to
                // the same question the output was going to answer.
                sink.output(&format!("{message}\n"));
                -1
            }
        };
        if exit_code != 0 {
            return BuildOutcome { exit_code };
        }
    }
    BuildOutcome { exit_code: 0 }
}

/// A human-readable form of what is being run, for a dock header or a
/// step separator — `cargo build --message-format=json`, `./gradlew clean`.
pub fn command_line(spec: &LaunchSpec) -> String {
    let mut line = spec.program.clone();
    for arg in &spec.args {
        line.push(' ');
        line.push_str(arg);
    }
    line
}

fn run_step(
    handle: &BuildHandle,
    step: &LaunchSpec,
    toolchain: ToolchainId,
    project_root: &std::path::Path,
    sink: &mut dyn BuildSink,
) -> Result<i32, String> {
    let (id, reader) = {
        let mut supervisor = handle
            .supervisor
            .lock()
            .map_err(|_| "build lock poisoned")?;
        let id = supervisor
            .launch("build", step)
            .map_err(|err| err.to_string())?;
        let reader = supervisor.take_reader(id).map_err(|err| err.to_string())?;
        (id, reader)
    };
    *handle.current.lock().map_err(|_| "build lock poisoned")? = Some(id);

    read_to_end(reader, toolchain, project_root, step.path_map.clone(), sink);

    *handle.current.lock().map_err(|_| "build lock poisoned")? = None;
    let mut supervisor = handle
        .supervisor
        .lock()
        .map_err(|_| "build lock poisoned")?;
    // `wait`, not `exit_code`: reaching EOF on the output means the process
    // is exiting, not that it has been reaped, so a poll here answers `None`
    // often enough to report a failed build as a successful one. A stopped
    // build has had its console removed by `Supervisor::stop` and errors
    // instead, which is also a failure rather than a success.
    let code = supervisor.wait(id).map_err(|_| "build stopped")?;
    Ok(code as i32)
}

/// Read one step's output until the process closes it. The blocking read
/// happens on the reader taken *out* of the supervisor, so `stop` can still
/// take the lock while this is waiting.
fn read_to_end(
    mut reader: Box<dyn Read + Send>,
    toolchain: ToolchainId,
    project_root: &std::path::Path,
    path_map: Option<container_core::target::PathMap>,
    sink: &mut dyn BuildSink,
) {
    let mut parser = DiagnosticParser::new(toolchain, project_root).with_path_map(path_map);
    let mut ansi = AnsiStripper::default();
    let mut buffer = [0u8; 8192];
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        // Lossy: a build tool writing invalid UTF-8 is not a reason to lose
        // the rest of its output.
        let chunk = String::from_utf8_lossy(&buffer[..read]).into_owned();
        // Stripped before parsing: a build over a PTY colours its
        // diagnostics, and `error:` wrapped in SGR codes matches nothing.
        let visible = ansi.feed(&chunk);
        let diagnostics = parser.feed(&visible);
        if !diagnostics.is_empty() {
            sink.diagnostics(diagnostics);
        }
        sink.output(&visible);
    }
    let diagnostics = parser.finish();
    if !diagnostics.is_empty() {
        sink.diagnostics(diagnostics);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[derive(Default)]
    struct Collected {
        output: String,
        diagnostics: Vec<BuildDiagnostic>,
    }

    impl BuildSink for Collected {
        fn output(&mut self, text: &str) {
            self.output.push_str(text);
        }
        fn diagnostics(&mut self, diagnostics: Vec<BuildDiagnostic>) {
            self.diagnostics.extend(diagnostics);
        }
    }

    fn shell(script: &str) -> LaunchSpec {
        LaunchSpec {
            program: "sh".into(),
            args: vec!["-c".into(), script.into()],
            cwd: Some(std::env::temp_dir()),
            env: Vec::new(),
            console: run_core::ConsoleKind::Pty,
            path_map: None,
        }
    }

    #[test]
    fn output_and_the_exit_code_both_come_back() {
        let handle = BuildHandle::new();
        let mut sink = Collected::default();
        let outcome = run(
            &handle,
            &[shell("echo hello-build; exit 0")],
            ToolchainId::Cmake,
            Path::new("/p"),
            &mut sink,
        );
        assert!(outcome.succeeded());
        assert!(sink.output.contains("hello-build"), "{:?}", sink.output);
    }

    #[test]
    fn a_failing_step_stops_the_ones_after_it() {
        let handle = BuildHandle::new();
        let mut sink = Collected::default();
        let outcome = run(
            &handle,
            &[shell("exit 3"), shell("echo should-not-run")],
            ToolchainId::Cmake,
            Path::new("/p"),
            &mut sink,
        );
        assert_eq!(outcome.exit_code, 3);
        assert!(!sink.output.contains("should-not-run"), "{:?}", sink.output);
    }

    #[test]
    fn diagnostics_are_recognised_in_the_stream() {
        let handle = BuildHandle::new();
        let mut sink = Collected::default();
        run(
            &handle,
            &[shell("echo 'src/main.cpp:12:5: error: expected token'")],
            ToolchainId::Cmake,
            Path::new("/p"),
            &mut sink,
        );
        assert_eq!(sink.diagnostics.len(), 1);
        assert_eq!(sink.diagnostics[0].line, 12);
    }

    #[test]
    fn a_step_that_cannot_start_fails_the_build_and_says_why() {
        let handle = BuildHandle::new();
        let mut sink = Collected::default();
        let outcome = run(
            &handle,
            &[LaunchSpec {
                program: "definitely-not-a-real-program".into(),
                ..shell("true")
            }],
            ToolchainId::Cmake,
            Path::new("/p"),
            &mut sink,
        );
        assert_eq!(outcome.exit_code, -1);
        assert!(!sink.output.is_empty(), "the failure has to be visible");
    }

    #[test]
    fn stopping_a_build_that_never_started_is_not_an_error() {
        BuildHandle::new().stop();
    }
}
