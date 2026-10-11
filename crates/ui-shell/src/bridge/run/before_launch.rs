//! Before-launch tasks (B2): what a configuration runs, in order, before
//! it starts, and how their progress reaches the Build dock. Split out of
//! `run/mod.rs` under the file-size ratchet.

use std::path::Path;
use std::pin::Pin;

use cxx_qt::CxxQtThread;
use cxx_qt_lib::QString;
use run_core::RunConfigExt;

use crate::bridge::errors;
use crate::bridge::ffi;

/// The `phpunit` configuration a "run another configuration" task names.
fn phpunit_task<'a>(
    task: &run_core::BeforeLaunchTask,
    configs: &'a [run_core::RunConfig],
) -> Option<&'a run_core::RunConfig> {
    let run_core::BeforeLaunchTask::RunConfiguration(id) = task else {
        return None;
    };
    configs
        .iter()
        .find(|c| &c.id == id && c.kind.as_deref() == Some(app_config::php::PHPUNIT_KIND))
}

fn announce_before_launch(qt_thread: &CxxQtThread<ffi::RunService>, config_id: &str, label: &str) {
    let (config_id, label) = (config_id.to_string(), label.to_string());
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
        service.as_mut().before_launch_started(
            QString::from(config_id.as_str()),
            QString::from(label.as_str()),
        );
    });
}

/// Run one configuration's before-launch tasks, in order, stopping at the
/// first failure. Returns whether the launch may proceed.
///
/// Every task is a process run to completion, which is exactly what
/// `build_core::runner` does — including a Build task, whose steps come from
/// the same `BuildSpec` the Build dock uses. There is no second way to run a
/// build in this codebase (ADR-0040).
pub(super) fn run_before_launch(
    tasks: &[run_core::BeforeLaunchTask],
    config_id: &str,
    root: &Path,
    configs: &[run_core::RunConfig],
    qt_thread: &CxxQtThread<ffi::RunService>,
) -> bool {
    for task in tasks {
        // A PHPUnit configuration runs through `test-core`, not a build
        // step: the same scope and argv as its own run, minus the Tests dock
        // (`TestService` lives on the Qt thread this task cannot wait on).
        if let Some(config) = phpunit_task(task, configs) {
            announce_before_launch(qt_thread, config_id, &config.name);
            if let Err(reason) = crate::bridge::testing::run_blocking(config, root) {
                fail_before_launch(
                    qt_thread,
                    config_id,
                    &format!("{} failed: {reason}", config.name),
                );
                return false;
            }
            continue;
        }
        let (label, steps, toolchain) = match resolve_task(task, root, configs) {
            Ok(resolved) => resolved,
            Err(message) => {
                fail_before_launch(qt_thread, config_id, &message);
                return false;
            }
        };

        announce_before_launch(qt_thread, config_id, &label);

        let mut sink = BeforeLaunchSink {
            config_id: config_id.to_string(),
            qt_thread: qt_thread.clone(),
        };
        let handle = build_core::BuildHandle::new();
        let outcome = build_core::runner::run(&handle, &steps, toolchain, root, &mut sink);
        if !outcome.succeeded() {
            fail_before_launch(
                qt_thread,
                config_id,
                &format!("{label} failed (exit {})", outcome.exit_code),
            );
            return false;
        }
    }
    true
}

/// What one task actually runs: a label for the dock, the steps, and the
/// toolchain whose diagnostics the output is read as.
type ResolvedTask = (String, Vec<run_core::LaunchSpec>, run_core::ToolchainId);

fn resolve_task(
    task: &run_core::BeforeLaunchTask,
    root: &Path,
    configs: &[run_core::RunConfig],
) -> Result<ResolvedTask, String> {
    match task {
        run_core::BeforeLaunchTask::Build => {
            let toolchain = build_core::buildable_toolchain(root).map_err(|err| err.to_string())?;
            let steps = build_core::BuildSpec::new(toolchain, build_core::BuildKind::Build, root)
                .steps()
                .map_err(|err| err.to_string())?;
            Ok(("Build".to_string(), steps, toolchain))
        }
        run_core::BeforeLaunchTask::RunConfiguration(id) => {
            let config = configs
                .iter()
                .find(|c| &c.id == id)
                .ok_or_else(|| format!("unknown run configuration \"{id}\""))?;
            let mut spec = config.to_launch_spec(root);
            spec.cwd = Some(spec.cwd.clone().unwrap_or_else(|| root.to_path_buf()));
            Ok((config.name.clone(), vec![spec], tool_for_output(root)))
        }
        run_core::BeforeLaunchTask::ExternalTool { program, args } => {
            let spec = run_core::LaunchSpec {
                program: program.clone(),
                args: args.clone(),
                cwd: Some(root.to_path_buf()),
                env: Vec::new(),
                console: run_core::ConsoleKind::Pty,
                path_map: None,
            };
            Ok((program.clone(), vec![spec], tool_for_output(root)))
        }
    }
}

/// Which parser a non-build task's output is read with. The project's own
/// toolchain, so a script that runs the compiler still produces recognisable
/// diagnostics; `Make` (the text table) when the project has no build tool,
/// because guessing Cargo's JSON for arbitrary output would find nothing.
fn tool_for_output(root: &Path) -> run_core::ToolchainId {
    build_core::buildable_toolchain(root).unwrap_or(run_core::ToolchainId::Make)
}

fn fail_before_launch(qt_thread: &CxxQtThread<ffi::RunService>, config_id: &str, message: &str) {
    let config_id = config_id.to_string();
    let message = message.to_string();
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::RunService>| {
        service.as_mut().before_launch_failed(
            QString::from(config_id.as_str()),
            ffi::FfiResult {
                code: errors::CODE_BEFORE_LAUNCH,
                message: QString::from(message.as_str()),
            },
        );
    });
}

/// Streams a before-launch task's output to the Build dock. Diagnostics are
/// deliberately dropped: the Problems dock is fed by `BuildService`, and a
/// second writer to it from here would leave rows nothing clears. The
/// errors themselves are still visible — they are in the output.
struct BeforeLaunchSink {
    config_id: String,
    qt_thread: CxxQtThread<ffi::RunService>,
}

impl build_core::BuildSink for BeforeLaunchSink {
    fn output(&mut self, text: &str) {
        let config_id = self.config_id.clone();
        let text = text.to_string();
        let _ = self
            .qt_thread
            .queue(move |mut service: Pin<&mut ffi::RunService>| {
                service.as_mut().before_launch_output(
                    QString::from(config_id.as_str()),
                    QString::from(text.as_str()),
                );
            });
    }

    fn diagnostics(&mut self, _diagnostics: Vec<build_core::BuildDiagnostic>) {}
}
