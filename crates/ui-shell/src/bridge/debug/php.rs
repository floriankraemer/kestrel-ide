//! PHP debugging (ADR-0069): the listen session vscode-php-debug runs while
//! Xdebug connections arrive, shown in the debugger as threads.
//!
//! Translation only: the listen arguments and the Xdebug environment are
//! `dap_core::xdebug::plan`'s.

use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use cxx_qt::Threading;
use cxx_qt_lib::QString;
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
        match self.ensure_php_listener(&root, plan.listen_arguments) {
            Ok(()) => ffi::FfiResult::default(),
            Err(err) => to_ffi_result(&err),
        }
    }

    pub fn is_php_listening(&self) -> bool {
        self.php_listen.borrow().is_some()
    }

    /// Make sure a listen session with `arguments` is running, replacing
    /// one that listens differently.
    pub(super) fn ensure_php_listener(
        mut self: Pin<&mut Self>,
        root: &Path,
        arguments: Value,
    ) -> Result<(), DapError> {
        if self
            .php_listen
            .borrow()
            .as_ref()
            .is_some_and(|listen| listen.arguments == arguments)
        {
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
            if let Err(err) = handshake(&session, arguments, &breakpoints) {
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
