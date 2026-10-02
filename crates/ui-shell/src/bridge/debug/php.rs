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
use dap_core::xdebug::{self, HostKind};
use dap_core::{DapError, DapSession};
use serde_json::Value;

use super::{current_project_root, handshake, no_project, to_ffi_result, QtListener, SessionState};
use crate::bridge::ffi;

/// The one listen session, and the adapter arguments it was started with —
/// a debug launch that needs a different listen address (a container)
/// replaces it.
pub(super) struct PhpListen {
    pub(super) session_id: u64,
    pub(super) arguments: Value,
}

/// A run to start once the listener is up, with the Xdebug environment.
pub(super) struct PhpRun {
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
            self.as_mut().stop_php_listener();
            return ffi::FfiResult::default();
        }
        let Some(root) = current_project_root() else {
            return no_project();
        };
        let plan = dap_core::xdebug::plan(dap_core::xdebug::HostKind::Local, None, xdebug_port());
        match self.ensure_php_listener(&root, plan.listen_arguments, None) {
            Ok(()) => ffi::FfiResult::default(),
            Err(err) => to_ffi_result(&err),
        }
    }

    pub fn is_php_listening(&self) -> bool {
        self.php_listen.borrow().is_some()
    }

    /// Make sure a listen session with `arguments` is running, replacing
    /// one that listens differently.
    ///
    /// `run` is started (through `phpLaunchRequested`) once the listener
    /// is ready: Xdebug does not retry, so a program that starts first
    /// finds nobody listening.
    pub(super) fn ensure_php_listener(
        mut self: Pin<&mut Self>,
        root: &Path,
        arguments: Value,
        run: Option<PhpRun>,
    ) -> Result<(), DapError> {
        if self
            .php_listen
            .borrow()
            .as_ref()
            .is_some_and(|listen| listen.arguments == arguments)
        {
            let session_id = self.php_listen.borrow().as_ref().map(|l| l.session_id);
            if let (Some(run), Some(session_id)) = (run, session_id) {
                let qt_thread = self.as_mut().qt_thread();
                std::thread::spawn(move || launch_after_check(qt_thread, session_id, run));
            }
            return Ok(());
        }
        // ponytail: the old adapter is shut down on the Qt thread (a couple
        // of seconds at worst) so its port is free for the new one.
        self.as_mut().stop_php_listener();

        let settings = app_config::project_settings::load(root).unwrap_or_default();
        let adapter = dap_core::catalog::resolve(
            dap_core::catalog::PHP_DEBUG,
            &settings.debug_adapters.unwrap_or_default(),
        )
        .ok_or_else(|| DapError::NoAdapter("PHP".to_string()))?;

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
        *self.php_listen.borrow_mut() = Some(PhpListen {
            session_id,
            arguments: arguments.clone(),
        });
        self.as_mut()
            .debug_started(session_id, QString::from("PHP"));
        self.as_mut().php_listening_changed(true);

        let breakpoints = self.breakpoints.borrow().clone();
        std::thread::spawn(move || {
            let result = handshake(&session, arguments, &breakpoints);
            if let (Ok(()), Some(run)) = (&result, run) {
                launch_after_check(qt_thread.clone(), session_id, run);
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
        let host = process_exec::host::ExecHost::for_path(root);
        // The adapter runs beside the project (in the distro, for a WSL
        // root), so the mapping's local side is a path it can read.
        let path_map = container_map
            .as_ref()
            .map(|map| process_exec::host::PathMap {
                local_root: PathBuf::from(dap_core::source_path(&host, &map.local_root)),
                remote_root: map.remote_root.clone(),
            });
        let kind = if path_map.is_some() {
            HostKind::Container
        } else {
            HostKind::Local
        };
        let plan = xdebug::plan(kind, path_map.as_ref(), xdebug_port());
        // The interpreter the run uses: the `[php]` one, in its container if
        // it has one.
        let settings = crate::bridge::convert::load_resolved_settings();
        let check_host =
            php_core::host::interpreter_host(&settings.php, &settings.containers, root);
        let run = PhpRun {
            config_id: config.id.clone(),
            env: plan.env,
            check: XdebugCheck {
                in_container: matches!(check_host, process_exec::host::ExecHost::Container(_)),
                host: check_host,
                interpreter: settings_model::php::resolve(&settings).interpreter,
                cwd: root.to_path_buf(),
            },
        };
        match self.ensure_php_listener(root, plan.listen_arguments, Some(run)) {
            Ok(()) => ffi::FfiResult::default(),
            Err(err) => to_ffi_result(&err),
        }
    }

    fn request_php_launch(self: Pin<&mut Self>, run: &PhpRun) {
        let env = serde_json::to_string(&run.env).unwrap_or_default();
        self.php_launch_requested(
            QString::from(run.config_id.as_str()),
            QString::from(env.as_str()),
        );
    }

    /// End the listen session, if any, and wait for its adapter to go.
    fn stop_php_listener(mut self: Pin<&mut Self>) {
        let Some(session_id) = self.php_listen.borrow().as_ref().map(|l| l.session_id) else {
            return;
        };
        let session = self.as_mut().session_handle(session_id);
        self.as_mut().finish_session(session_id, 0);
        if let Some(session) = session {
            session.shutdown();
        }
    }

    /// A session ended: if it was the listen session, say so.
    pub(super) fn forget_php_listener(mut self: Pin<&mut Self>, session_id: u64) {
        let was_listener = {
            let mut listen = self.php_listen.borrow_mut();
            let matches = listen.as_ref().is_some_and(|l| l.session_id == session_id);
            if matches {
                *listen = None;
            }
            matches
        };
        if was_listener {
            self.as_mut().php_listening_changed(false);
        }
    }
}
