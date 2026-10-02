//! Launching, restarting and reconfiguring a language's servers.
//!
//! Split out of `mod.rs` once it crossed the file-size ceiling
//! (`scripts/check-file-size.sh`): a second `impl ffi::LanguageService` block
//! for the same QObject. What to start, stop or push is `lsp_core`'s rule
//! (`launch_plan`, `reload_plan`); this only carries it out and reports it.

use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use super::plugin_servers;
use crate::bridge::ffi::{self};

impl ffi::LanguageService {
    pub fn apply_server_settings(mut self: Pin<&mut Self>) {
        // The resolved layer, not the global file: a project may name its
        // own language servers (ADR-0022), and a project that pins a
        // toolchain-local server is the reason that field is project-scoped.
        let settings = crate::bridge::convert::load_resolved_settings();
        let overrides =
            settings_model::servers::overrides_from_settings(&settings.language_servers);
        let resolved = lsp_core::resolve_servers(&overrides, &plugin_servers());

        // What the new settings mean for the running servers is
        // `lsp_core::reload_plan`'s rule: restart what was fixed at launch,
        // push what a server can re-read, leave the rest running.
        let previous = self.configs.borrow().clone();
        let plan = {
            let started = self.started.borrow();
            lsp_core::reload_plan(&previous, &resolved, |language_id| {
                started.contains(language_id)
            })
        };
        *self.configs.borrow_mut() = resolved.clone();

        let language_of = |id: &str| {
            previous
                .iter()
                .chain(resolved.iter())
                .find(|c| c.id == id)
                .map(|c| c.language_id.clone())
        };
        for id in &plan.stop {
            let Some(language_id) = language_of(id) else {
                continue;
            };
            if let Some(advertised) = self.advertised.borrow_mut().get_mut(&language_id) {
                advertised.remove(id);
            }
            // A stopped server's rows would otherwise outlive it.
            self.store
                .borrow_mut()
                .clear_source(&lsp_core::diagnostics::source_key(id));
            let stopping = id.clone();
            self.as_ref()
                .push_job(move |manager| manager.stop_server(&stopping));
        }
        for (id, settings) in plan.push {
            self.as_ref().push_job(move |manager| {
                let _ = manager.update_settings(&id, settings);
            });
        }
        // A server launched now has seen none of the open documents.
        // Forgetting them is what lets `reopenDocument` send `didOpen` again
        // (the servers that kept running ignore the repeat).
        let mut restarting: Vec<String> = Vec::new();
        for id in &plan.start {
            if let Some(language_id) = language_of(id) {
                if !restarting.contains(&language_id) {
                    restarting.push(language_id);
                }
            }
        }
        for language_id in restarting {
            self.open_docs
                .borrow_mut()
                .retain(|_, open_for| open_for != &language_id);
            let configs: Vec<lsp_core::ServerConfig> = resolved
                .iter()
                .filter(|c| c.language_id == language_id && plan.start.contains(&c.id))
                .cloned()
                .collect();
            self.as_mut().start_servers(configs);
        }
    }

    pub fn restart_server(mut self: Pin<&mut Self>, server_id: &QString) {
        let server_id = server_id.to_string();
        let config = self
            .configs
            .borrow()
            .iter()
            .find(|config| config.id == server_id)
            .cloned();
        let Some(config) = config else {
            return;
        };
        let stopping = server_id.clone();
        self.as_ref()
            .push_job(move |manager| manager.stop_server(&stopping));
        self.started.borrow_mut().insert(config.language_id.clone());
        self.as_mut().start_servers(vec![config]);
    }

    /// Queue the (blocking) launch of a language's servers, in answer
    /// order, and report each outcome. A launch where every server fails
    /// frees the language again, so opening another file of it retries
    /// rather than staying silently dead for the session; one server failing
    /// beside a working one does not, or a missing Phpactor would be
    /// relaunched on every file open. A server the platform rules out
    /// (`lsp_core::launch_plan`) is reported `Unavailable`, with the reason,
    /// and never launched.
    pub(super) fn start_servers(mut self: Pin<&mut Self>, configs: Vec<lsp_core::ServerConfig>) {
        let Some(language_id) = configs.first().map(|c| c.language_id.clone()) else {
            return;
        };
        let plan = lsp_core::launch_plan(&configs, &self.host.borrow(), cfg!(windows));
        for (config, reason) in &plan.skipped {
            self.as_mut().server_state_changed(
                QString::from(config.id.as_str()),
                QString::from(config.name.as_str()),
                ffi::FfiServerState::Unavailable,
                QString::from(reason.as_str()),
                0,
            );
        }
        let configs = plan.start;
        if configs.is_empty() {
            return;
        }
        let qt_thread = self.as_mut().qt_thread();
        for config in &configs {
            self.as_mut().server_state_changed(
                QString::from(config.id.as_str()),
                QString::from(config.name.as_str()),
                ffi::FfiServerState::Starting,
                QString::default(),
                0,
            );
        }
        self.push_job(move |manager| {
            let mut failures = Vec::new();
            for config in &configs {
                if let Err(err) = manager.start(config) {
                    failures.push((config.id.clone(), config.name.clone(), err.to_string()));
                }
            }
            if failures.is_empty() {
                return;
            }
            let all_failed = failures.len() == configs.len();
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if all_failed {
                    service.started.borrow_mut().remove(&language_id);
                }
                for (id, name, message) in failures {
                    service.as_mut().server_state_changed(
                        QString::from(id.as_str()),
                        QString::from(name.as_str()),
                        ffi::FfiServerState::Failed,
                        QString::from(message.as_str()),
                        0,
                    );
                }
            });
        });
    }
}
