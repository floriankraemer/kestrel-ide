//! PHP debugging (ADR-0069): the listen session vscode-php-debug runs while
//! Xdebug connections arrive, shown in the debugger as threads.
//!
//! Translation only: the listen arguments and the Xdebug environment are
//! `dap_core::xdebug::plan`'s.

use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;
use dap_core::xdebug::{self, ListenAction};
use dap_core::{DapError, DapSession};
use serde_json::Value;

use super::{current_project_root, handshake, no_project, to_ffi_result, QtListener, SessionState};
use crate::bridge::ffi;

/// A run to start once the listener is up, with the Xdebug environment.
pub(super) struct PhpRun {
    /// Empty for the pending gutter test run.
    config_id: String,
    env: Vec<(String, String)>,
    check: XdebugCheck,
}

/// Where to ask the interpreter whether it can debug.
struct XdebugCheck {
    host: process_exec::host::ExecHost,
    interpreter: String,
    cwd: PathBuf,
    in_container: bool,
}

/// Probe the interpreter, report what is wrong for debugging, and say
/// whether the run may start. Blocking: call it off the Qt thread. A probe
/// that fails says nothing about Xdebug, and the run then reports its own
/// error.
fn launch_after_check(qt_thread: CxxQtThread<ffi::DebugService>, session_id: u64, run: PhpRun) {
    let check = &run.check;
    let issue = php_core::probe::probe(&check.host, &check.interpreter, &check.cwd)
        .ok()
        .and_then(|probe| probe.xdebug_issue());
    let advice = issue
        .as_ref()
        .map(|issue| (issue.blocks_debugging(), issue.advice(check.in_container)));
    let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
        match advice {
            Some((true, advice)) => {
                let error = DapError::XdebugUnavailable(advice);
                service
                    .as_mut()
                    .debug_failed(session_id, to_ffi_result(&error));
                return;
            }
            Some((false, advice)) => service.as_mut().debug_output(
                session_id,
                QString::from("stderr"),
                QString::from(format!("{advice}\n").as_str()),
            ),
            None => {}
        }
        service.as_mut().request_php_launch(&run);
    });
}

/// `[php].xdebug_port` in force.
pub(super) fn xdebug_port() -> u16 {
    settings_model::php::resolve(&crate::bridge::convert::load_resolved_settings()).xdebug_port
}

impl ffi::DebugService {
    /// The "Start Listening for PHP Debug Connections" toggle.
    pub fn set_php_listening(mut self: Pin<&mut Self>, enabled: bool) -> ffi::FfiResult {
        if !enabled {
            let stopping = self.php_listen.borrow_mut().stop();
            if stopping.is_some() {
                if let Some(old) = self.as_mut().stop_php_listener() {
                    std::thread::spawn(move || old.shutdown());
                }
            }
            return ffi::FfiResult::default();
        }
        let Some(root) = current_project_root() else {
            return no_project();
        };
        // Listen the way the `[php]` interpreter's host needs, like a run does.
        let settings = crate::bridge::convert::load_resolved_settings();
        let container_map =
            match php_core::host::interpreter_host(&settings.php, &settings.containers, &root) {
                process_exec::host::ExecHost::Container(container) => Some(container.path_map),
                _ => None,
            };
        let plan = xdebug::plan_for(
            &process_exec::host::ExecHost::for_path(&root),
            container_map.as_ref(),
            xdebug_port(),
        );
        self.listen_when_located(root, plan.listen_arguments, None)
    }

    pub fn is_php_listening(&self) -> bool {
        self.php_listen.borrow().is_listening()
    }

    /// Make sure a listen session with `arguments` is running, replacing
    /// one that listens differently.
    ///
    /// `run` is started (through `phpLaunchRequested`) once the listener
    /// is ready: Xdebug does not retry, so a program that starts first
    /// finds nobody listening. Replacing shuts the old adapter down on a
    /// worker (it holds the port, and takes a couple of seconds at worst);
    /// the new listener starts when that is done.
    pub(super) fn ensure_php_listener(
        mut self: Pin<&mut Self>,
        root: &Path,
        arguments: Value,
        run: Option<PhpRun>,
    ) -> Result<(), DapError> {
        let action = self.php_listen.borrow_mut().request(root, arguments, run);
        match action {
            ListenAction::Nothing => Ok(()),
            ListenAction::Launch { session_id, run } => {
                let qt_thread = self.as_mut().qt_thread();
                std::thread::spawn(move || launch_after_check(qt_thread, session_id, run));
                Ok(())
            }
            ListenAction::Start(request) => {
                self.start_php_listener(&request.root, request.arguments, request.run)
            }
            ListenAction::Replace => {
                let old = self.as_mut().stop_php_listener();
                let qt_thread = self.as_mut().qt_thread();
                std::thread::spawn(move || {
                    if let Some(old) = old {
                        old.shutdown();
                    }
                    let _ = qt_thread.queue(|service: Pin<&mut ffi::DebugService>| {
                        service.start_pending_php_listener();
                    });
                });
                Ok(())
            }
        }
    }

    /// The old adapter is gone: start the listener queued behind it, if
    /// the user has not stopped listening meanwhile.
    fn start_pending_php_listener(mut self: Pin<&mut Self>) {
        let pending = self.php_listen.borrow_mut().take_pending();
        let Some(request) = pending else {
            return;
        };
        if let Err(err) =
            self.as_mut()
                .start_php_listener(&request.root, request.arguments, request.run)
        {
            // No session exists to attach the failure to; the debug console
            // shows it whatever the id.
            self.as_mut().debug_failed(0, to_ffi_result(&err));
        }
    }

    fn start_php_listener(
        mut self: Pin<&mut Self>,
        root: &Path,
        arguments: Value,
        run: Option<PhpRun>,
    ) -> Result<(), DapError> {
        let settings = app_config::project_settings::load(root).unwrap_or_default();
        let adapter = dap_core::catalog::resolve_on(
            dap_core::catalog::PHP_DEBUG,
            &settings.debug_adapters.unwrap_or_default(),
            &process_exec::host::ExecHost::for_path(root),
            root,
        )
        .ok_or_else(|| DapError::NoAdapter("PHP".to_string()))?;
        if let Some(missing) = dap_core::catalog::not_located(&adapter) {
            return Err(missing);
        }

        let session_id = self.next_id.get() + 1;
        self.next_id.set(session_id);
        let qt_thread = self.as_mut().qt_thread();
        let session = DapSession::start(
            &adapter,
            Some(root),
            Box::new(QtListener {
                session_id,
                qt_thread: qt_thread.clone(),
            }),
        )?;
        self.sessions
            .borrow_mut()
            .insert(session_id, SessionState::new(Arc::clone(&session)));
        self.php_listen
            .borrow_mut()
            .started(session_id, arguments.clone());
        self.as_mut()
            .debug_started(session_id, QString::from("PHP"));
        self.as_mut().php_listening_changed(true);

        let breakpoints = self.breakpoints.borrow().clone();
        std::thread::spawn(move || {
            let result = handshake(&session, arguments, &breakpoints);
            if result.is_ok() {
                if let Some(run) = run {
                    launch_after_check(qt_thread.clone(), session_id, run);
                }
                // Runs that asked to launch while this handshake was in flight.
                let waiting = qt_thread.clone();
                let _ = qt_thread.queue(move |service: Pin<&mut ffi::DebugService>| {
                    let runs = service.php_listen.borrow_mut().handshake_done(session_id);
                    for run in runs {
                        let qt_thread = waiting.clone();
                        std::thread::spawn(move || launch_after_check(qt_thread, session_id, run));
                    }
                });
            }
            if let Err(err) = result {
                let failure = (err.code(), err.to_string());
                let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                    service.as_mut().debug_failed(
                        session_id,
                        ffi::FfiResult {
                            code: failure.0,
                            message: QString::from(failure.1.as_str()),
                        },
                    );
                    service.as_mut().finish_session(session_id, -1);
                });
            }
        });
        Ok(())
    }

    /// Debug a PHP run configuration: listen (in the shape the host needs),
    /// then have the run start with the Xdebug environment.
    pub(super) fn debug_php(
        self: Pin<&mut Self>,
        config: &run_core::RunConfig,
        root: &Path,
    ) -> ffi::FfiResult {
        let containers = crate::bridge::run::effective_container_settings();
        let container_map = config
            .run_on
            .as_deref()
            .and_then(|run_on| run_core::container_target::path_map(run_on, &containers, root));
        let check_host =
            run_core::container_target::run_host(config.run_on.as_deref(), &containers, root);
        self.launch_php_debug(config.id.clone(), container_map, check_host, root)
    }

    /// Debug the gutter test `TestService` has pending: it runs on the
    /// `[php]` interpreter's host, so a container interpreter's mount is
    /// the path map.
    pub fn debug_php_tests(self: Pin<&mut Self>) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let settings = crate::bridge::convert::load_resolved_settings();
        let container_map =
            match php_core::host::interpreter_host(&settings.php, &settings.containers, &root) {
                process_exec::host::ExecHost::Container(container) => Some(container.path_map),
                _ => None,
            };
        let check_host =
            php_core::host::interpreter_host(&settings.php, &settings.containers, &root);
        self.launch_php_debug(String::new(), container_map, check_host, &root)
    }

    /// Listen and start the run once the listener is up; `config_id` is
    /// empty for the pending test run.
    fn launch_php_debug(
        self: Pin<&mut Self>,
        config_id: String,
        container_map: Option<process_exec::host::PathMap>,
        check_host: process_exec::host::ExecHost,
        root: &Path,
    ) -> ffi::FfiResult {
        // The adapter runs beside the project (in the distro, for a WSL
        // root), so the mapping's local side is a path it can read.
        let plan = xdebug::plan_for(
            &process_exec::host::ExecHost::for_path(root),
            container_map.as_ref(),
            xdebug_port(),
        );
        // The interpreter the run uses, on the host the run uses (its
        // `run_on` target, or the `[php]` interpreter's for the test run).
        let settings = crate::bridge::convert::load_resolved_settings();
        let run = PhpRun {
            config_id,
            env: plan.env,
            check: XdebugCheck {
                in_container: matches!(check_host, process_exec::host::ExecHost::Container(_)),
                host: check_host,
                interpreter: settings_model::php::resolve(&settings).interpreter,
                cwd: root.to_path_buf(),
            },
        };
        self.listen_when_located(root.to_path_buf(), plan.listen_arguments, Some(run))
    }

    /// [`Self::ensure_php_listener`], once the adapter script is known. For
    /// a WSL project the first lookup is a blocking `wsl.exe` call
    /// (`dap_core::catalog::resolve_on`), so it runs on a worker and the
    /// listener starts back on the Qt thread; a failure then reaches the
    /// debug console like any listener failure, since the caller has
    /// already returned.
    fn listen_when_located(
        mut self: Pin<&mut Self>,
        root: PathBuf,
        arguments: Value,
        run: Option<PhpRun>,
    ) -> ffi::FfiResult {
        let host = process_exec::host::ExecHost::for_path(&root);
        if dap_core::catalog::php_debug_is_located(&host) {
            return match self.ensure_php_listener(&root, arguments, run) {
                Ok(()) => ffi::FfiResult::default(),
                Err(err) => to_ffi_result(&err),
            };
        }
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            // Fills the per-distro cache; the answer itself is not needed.
            let _ = dap_core::catalog::resolve_on(dap_core::catalog::PHP_DEBUG, &[], &host, &root);
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::DebugService>| {
                if let Err(err) = service.as_mut().ensure_php_listener(&root, arguments, run) {
                    service.as_mut().debug_failed(0, to_ffi_result(&err));
                }
            });
        });
        ffi::FfiResult::default()
    }

    fn request_php_launch(self: Pin<&mut Self>, run: &PhpRun) {
        let env = serde_json::to_string(&run.env).unwrap_or_default();
        if run.config_id.is_empty() {
            self.php_test_launch_requested(QString::from(env.as_str()));
            return;
        }
        self.php_launch_requested(
            QString::from(run.config_id.as_str()),
            QString::from(env.as_str()),
        );
    }

    /// End the listen session, if any. The adapter is returned still
    /// running: shutting it down blocks, so the caller does that off the
    /// Qt thread.
    fn stop_php_listener(mut self: Pin<&mut Self>) -> Option<Arc<DapSession>> {
        let session_id = self.php_listen.borrow().session_id()?;
        let session = self.as_mut().session_handle(session_id);
        self.as_mut().finish_session(session_id, 0);
        session
    }

    /// A session ended: if it was the listen session, say so.
    pub(super) fn forget_php_listener(mut self: Pin<&mut Self>, session_id: u64) {
        let was_listener = self.php_listen.borrow_mut().ended(session_id);
        if was_listener {
            self.as_mut().php_listening_changed(false);
        }
    }
}
