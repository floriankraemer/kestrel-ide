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

/// The server configurations in force: the catalog, plugins and the user's
/// `[[language_server]]` entries (`lsp_core::resolve_servers`), then the
/// `[php]` layer on the two PHP servers (`php_core::lsp::apply`). One
/// function for the start at project open and for a settings save, so both
/// derive the same configs and `reload_plan` sees only real differences.
pub(super) fn resolved_server_configs(
    settings: &app_config::Settings,
    root: Option<&std::path::Path>,
) -> Vec<lsp_core::ServerConfig> {
    let overrides = settings_model::servers::overrides_from_settings(&settings.language_servers);
    let mut configs = lsp_core::resolve_servers(&overrides, &plugin_servers());
    let Some(root) = root else {
        return configs;
    };
    let php = settings_model::php::resolve(settings);
    // An unreadable composer.json only means "no level from composer".
    let composer = php_core::composer::ComposerJson::read(root).ok().flatten();
    let language_level = php_core::level::resolve(
        php.language_level.as_deref(),
        composer.as_ref().and_then(|c| c.require_php.as_deref()),
        None,
    );
    // The key is a secret: the keychain, never a settings file. Unreadable
    // reads as "no key", the page says why.
    let licence_key = crate::bridge::php::licence_key();
    let input = php_core::lsp::LspInput {
        language_level,
        include_paths: &php.include_paths,
        stubs: php.stubs.as_deref(),
        licence_key: licence_key.as_deref(),
        storage_path: None,
        project_root: root,
    };
    php_core::lsp::apply(&mut configs, &input, |id| {
        let toggles = php.servers.get(id).copied().unwrap_or_default();
        php_core::lsp::Toggles {
            enabled: toggles.enabled,
            diagnostics: toggles.diagnostics,
        }
    });
    configs
}

impl ffi::LanguageService {
    pub fn apply_server_settings(mut self: Pin<&mut Self>) {
        // The resolved layer, not the global file: a project may name its
        // own language servers (ADR-0022), and a project that pins a
        // toolchain-local server is the reason that field is project-scoped.
        let settings = crate::bridge::convert::load_resolved_settings();
        let resolved = resolved_server_configs(
            &settings,
            crate::bridge::convert::current_project_root().as_deref(),
        );

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

    /// A respawned server process has seen none of the open documents. Send
    /// each its live text again; `lsp_core` forgot what the crashed process
    /// had (`did_open` reaches only servers that have not been told), so
    /// servers that kept running are not sent a second `didOpen`.
    pub(super) fn reopen_documents_for(mut self: Pin<&mut Self>, language_id: &str) {
        let docs: Vec<(String, String)> = {
            let session = self.session.borrow();
            self.open_docs
                .borrow()
                .iter()
                .filter(|(_, open_for)| open_for.as_str() == language_id)
                .filter_map(|(path, _)| {
                    let text = session.content_for_path(std::path::Path::new(path))?;
                    Some((lsp_core::uri_from_path(path), text))
                })
                .collect()
        };
        if docs.is_empty() {
            return;
        }
        let language_id = language_id.to_string();
        self.as_mut().push_job(move |manager| {
            for (uri, text) in &docs {
                let _ = manager.did_open(uri, &language_id, text);
            }
        });
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
        // A server with `exec = "interpreter"` runs where PHP does: in the
        // container the `[php]` settings name, if any (ADR-0067).
        let project_host = self.host.borrow().clone();
        let interpreter_host = match crate::bridge::convert::current_project_root() {
            Some(root) => crate::bridge::php::tool_host(
                Some("php"),
                &crate::bridge::convert::load_resolved_settings(),
                &root,
            ),
            None => project_host.clone(),
        };
        let plan = lsp_core::launch_plan(&configs, &project_host, &interpreter_host, cfg!(windows));
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
                let host = match config.exec {
                    lsp_core::catalog::ServerExec::Host => project_host.clone(),
                    lsp_core::catalog::ServerExec::Interpreter => interpreter_host.clone(),
                };
                if let Err(err) = manager.start_on(config, host) {
                    failures.push((
                        config.id.clone(),
                        config.name.clone(),
                        lsp_core::classify_start_failure(&config.id, &err),
                    ));
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
                for (id, name, failure) in failures {
                    let (state, detail) = match &failure {
                        lsp_core::StartFailure::NotFound { hint } => {
                            (ffi::FfiServerState::NotFound, hint)
                        }
                        lsp_core::StartFailure::Failed { message } => {
                            (ffi::FfiServerState::Failed, message)
                        }
                    };
                    service.as_mut().server_state_changed(
                        QString::from(id.as_str()),
                        QString::from(name.as_str()),
                        state,
                        QString::from(detail.as_str()),
                        0,
                    );
                }
            });
        });
    }
}
