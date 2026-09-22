//! Settings > Database (Database Tools plan F1.6): the list accessors
//! `AppSettings` grows (the same per-feature-module split `containers.rs`/
//! `build_tools.rs` already use) and `DataSourceEditor`, the one-source
//! draft the Add/Edit dialog drives — shaped after `BuildToolsEditorRust`
//! (`bridge::build_tools`), but per-item rather than a single global draft,
//! the same difference `RunConfigEditor` has from it.
//!
//! Sources always show the *merged* view (`settings_model::scope::
//! resolve_database_sources`) regardless of the settings dialog's own
//! global/project toggle — a data source is a union-by-id collection, not
//! a section one layer replaces wholesale (ADR-0045 §2, the same reason
//! [`crate::bridge::database`]'s doc comment gives).

use std::cell::RefCell;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use app_config::database::DataSourceSetting;
use settings_model::database::{DataSourceDraft, DataSourceField, DataSourceProblem};
use settings_model::Scope;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};
use crate::bridge::settings::{commit_to_project, scope_from_name};

/// `secret-store`'s service name for data-source credentials (ADR-0061 §1,
/// `database-tools.md` §8) — distinct from `container-registry`'s
/// `"ide.containers"`, so the two features never share one keychain
/// namespace.
const SECRET_SERVICE: &str = "ide.database";

fn secrets() -> secret_store::SecretStore {
    secret_store::SecretStore::new(SECRET_SERVICE)
}

/// An id for a freshly added source — same nanosecond-timestamp shape
/// `RunConfigEditor::generate_id` already uses, for the same reason: unique
/// enough within one edit session without a new dependency.
fn generate_id() -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    format!("ds-{nanos}")
}

/// Every data source, global and project merged by id, each with the
/// layer it actually lives in — what the settings page's "scope"
/// indicator and `DataSourceEditor::begin_edit`'s lookup both read.
pub(super) fn sources_with_scope() -> Vec<(DataSourceSetting, Scope)> {
    let global = crate::bridge::convert::load_settings();
    let project = crate::bridge::convert::load_project_settings();
    settings_model::scope::resolve_database_sources(&global, &project)
}

fn scope_id(scope: Scope) -> &'static str {
    match scope {
        Scope::Project => "project",
        _ => "global",
    }
}

fn to_ffi_row(setting: &DataSourceSetting, scope: Scope) -> ffi::FfiDataSourceRow {
    ffi::FfiDataSourceRow {
        id: QString::from(setting.id.as_str()),
        name: QString::from(setting.name.as_str()),
        driver: QString::from(setting.driver.as_str()),
        group: QString::from(setting.group.as_str()),
        color: QString::from(setting.color.as_str()),
        scope: QString::from(scope_id(scope)),
    }
}

/// Write `setting` into whichever layer `scope` names: replacing the
/// existing row with this id if one is there, appending otherwise — the
/// one place both [`ffi::DataSourceEditor::commit`] and "Duplicate" go
/// through, so the two never drift on how an upsert is done.
pub(super) fn upsert_source(setting: DataSourceSetting, scope: Scope) -> FfiResult {
    if scope == Scope::Project {
        return commit_to_project(move |project| {
            let database = project.database.get_or_insert_with(Default::default);
            match database.sources.iter_mut().find(|row| row.id == setting.id) {
                Some(existing) => *existing = setting,
                None => database.sources.push(setting),
            }
        });
    }
    let config_dir = app_core::resolve_config_dir();
    let Ok(mut settings) = app_config::load(&config_dir) else {
        return errors::failure(errors::CODE_SETTINGS_IO, "could not read global settings");
    };
    match settings
        .database
        .sources
        .iter_mut()
        .find(|row| row.id == setting.id)
    {
        Some(existing) => *existing = setting,
        None => settings.database.sources.push(setting),
    }
    match app_config::save(&config_dir, &settings) {
        Ok(()) => FfiResult::default(),
        Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
    }
}

/// Persists a source's script policy — the console bar's own selector
/// (database-tools-plan F3e), not the Add/Edit Source dialog. Silently
/// does nothing when `source_id` no longer resolves to a configured
/// source (e.g. the console detached mid-write); the in-memory
/// `ConsoleState` the caller also updates is what governs the running
/// console either way.
pub(super) fn persist_script_policy(source_id: &str, policy: &str) {
    if let Some((mut setting, scope)) = sources_with_scope()
        .into_iter()
        .find(|(row, _)| row.id == source_id)
    {
        setting.script_policy = policy.to_string();
        let _ = upsert_source(setting, scope);
    }
}

fn to_ffi_problem(problem: DataSourceProblem) -> ffi::FfiDataSourceProblem {
    let field = match problem.field {
        DataSourceField::Name => ffi::FfiDataSourceField::Name,
        DataSourceField::Id => ffi::FfiDataSourceField::Id,
        DataSourceField::Port => ffi::FfiDataSourceField::Port,
        DataSourceField::Color => ffi::FfiDataSourceField::Color,
        DataSourceField::SshUser => ffi::FfiDataSourceField::SshUser,
        DataSourceField::FileSourcePath => ffi::FfiDataSourceField::FileSourcePath,
    };
    ffi::FfiDataSourceProblem {
        field,
        sentence: QString::from(problem.sentence.as_str()),
    }
}

fn port_text(port: Option<u16>) -> QString {
    QString::from(port.map(|p| p.to_string()).unwrap_or_default().as_str())
}

fn port_from_text(text: &QString) -> Option<u16> {
    text.to_string().trim().parse().ok()
}

fn to_ffi_fields(draft: &DataSourceDraft) -> ffi::FfiDataSourceFields {
    ffi::FfiDataSourceFields {
        id: QString::from(draft.id.as_str()),
        name: QString::from(draft.name.as_str()),
        driver: QString::from(draft.driver.as_str()),
        group: QString::from(draft.group.as_str()),
        color: QString::from(draft.color.as_str()),
        host: QString::from(draft.host.as_str()),
        port: port_text(draft.port),
        database: QString::from(draft.database.as_str()),
        user: QString::from(draft.user.as_str()),
        auth: QString::from(draft.auth.as_str()),
        read_only: draft.read_only,
        history: draft.history,
        url: QString::from(draft.url.as_str()),
        ssl_mode: QString::from(draft.ssl_mode.as_str()),
        ssl_ca_file: QString::from(draft.ssl_ca_file.as_str()),
        ssh_host: QString::from(draft.ssh_host.as_str()),
        ssh_port: port_text(draft.ssh_port),
        ssh_user: QString::from(draft.ssh_user.as_str()),
        ssh_auth: QString::from(draft.ssh_auth.as_str()),
        ssh_key_file: QString::from(draft.ssh_key_file.as_str()),
    }
}

impl ffi::AppSettings {
    /// Every data source, for the settings page's list — always the merged
    /// (global + project) view, see this module's doc comment.
    pub fn database_sources(&self) -> Vec<ffi::FfiDataSourceRow> {
        sources_with_scope()
            .into_iter()
            .map(|(setting, scope)| to_ffi_row(&setting, scope))
            .collect()
    }

    /// Removed from whichever layer(s) currently hold this id — idempotent,
    /// the same "removing something absent is not an error" rule
    /// `SecretStore::delete` already follows.
    pub fn remove_database_source(&self, id: &QString) -> FfiResult {
        let id = id.to_string();
        let config_dir = app_core::resolve_config_dir();
        if let Ok(mut settings) = app_config::load(&config_dir) {
            let before = settings.database.sources.len();
            settings.database.sources.retain(|row| row.id != id);
            if settings.database.sources.len() != before
                && app_config::save(&config_dir, &settings).is_err()
            {
                return errors::failure(errors::CODE_SETTINGS_IO, "could not save global settings");
            }
        }
        if let Some(root) = crate::bridge::convert::current_project_root() {
            let _ = app_config::project_settings::update(&root, |project| {
                if let Some(database) = &mut project.database {
                    database.sources.retain(|row| row.id != id);
                }
            });
        }
        let _ = secrets().delete(&id);
        let _ = secrets().delete(&format!("{id}/ssh"));
        let _ = secrets().delete(&format!("{id}/ssl-key"));
        FfiResult::default()
    }

    /// A copy of an existing source, same scope, a fresh id and `"(copy)"`
    /// appended to the name — no dialog, matching the immediate-copy
    /// behaviour a "Duplicate" list action gives elsewhere. No secret is
    /// copied: a duplicated source starts with no stored password, the
    /// same "never silently propagate a credential" instinct
    /// `SecretStore` embodies by having no copy operation of its own.
    pub fn duplicate_database_source(&self, id: &QString) -> FfiResult {
        let id = id.to_string();
        let Some((setting, scope)) = sources_with_scope()
            .into_iter()
            .find(|(row, _)| row.id == id)
        else {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, "no such data source");
        };
        let copy = DataSourceSetting {
            id: generate_id(),
            name: format!("{} (copy)", setting.name),
            ..setting
        };
        upsert_source(copy, scope)
    }

    /// The driver catalogue a plugin contributes, for the dialog's driver
    /// combo box.
    pub fn database_drivers(&self) -> Vec<ffi::FfiDriverOption> {
        crate::bridge::database::driver_catalog()
            .into_iter()
            .map(|option| ffi::FfiDriverOption {
                id: QString::from(option.id.as_str()),
                name: QString::from(option.name.as_str()),
                backend: QString::from(option.backend.as_str()),
            })
            .collect()
    }

    /// `[database] allow_third_party_drivers` (F8.5, ADR-0061 §4), always
    /// read from the global layer — a driver-install consent decision is
    /// not the kind of thing a project should be able to relax on a
    /// teammate's behalf, unlike a data source itself.
    pub fn allow_third_party_drivers(&self) -> bool {
        let config_dir = app_core::resolve_config_dir();
        app_config::load(&config_dir)
            .map(|settings| settings.database.allow_third_party_drivers_or_default())
            .unwrap_or(app_config::database::DEFAULT_ALLOW_THIRD_PARTY_DRIVERS)
    }

    pub fn set_allow_third_party_drivers(&self, value: bool) -> FfiResult {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return errors::failure(errors::CODE_SETTINGS_IO, "could not read global settings");
        };
        settings.database.allow_third_party_drivers = Some(value);
        match app_config::save(&config_dir, &settings) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }
}

/// Rust side of the `DataSourceEditor` QObject.
#[derive(Default)]
pub struct DataSourceEditorRust {
    draft: RefCell<Option<DataSourceDraft>>,
    saved: RefCell<Option<DataSourceDraft>>,
    scope: RefCell<Scope>,
    /// The password typed this session when the keychain could not store
    /// it (`SecretError::Unavailable`) — never written to disk, exactly
    /// the degrade-to-session-only rule ADR-0061 §1 describes.
    session_password: RefCell<Option<String>>,
    /// Guards `testConnection`'s async result against a second call
    /// superseding the first before it reports back (database-tools.md
    /// §4's generation counter, applied to one editor's own in-flight
    /// test rather than a whole session).
    generation: Arc<AtomicU64>,
    /// The `(host, port)` a `hostKeyPrompt` signal was just raised for
    /// (F7b) — `acceptHostKey` reads this to know which pending key
    /// `db_drivers::ssh` should record, since the signal only carries
    /// what the dialog displays, not what the accept call needs to look
    /// the key back up by.
    pending_host_key: RefCell<Option<(String, u16)>>,
}

impl ffi::DataSourceEditor {
    /// Load the draft: an empty `id` starts a new source in `scope`
    /// (`"global"`/`"project"`); a known id reopens it from whichever
    /// layer it actually lives in, ignoring `scope` — editing a source
    /// never moves it to a different layer.
    pub fn begin_edit(&self, id: &QString, scope: &QString) {
        let id = id.to_string();
        let requested_scope = scope_from_name(&scope.to_string());
        let (draft, found_scope) = if id.is_empty() {
            (
                DataSourceDraft::new(generate_id(), String::new()),
                requested_scope,
            )
        } else {
            match sources_with_scope().into_iter().find(|(s, _)| s.id == id) {
                Some((setting, scope)) => (DataSourceDraft::from_setting(&setting), scope),
                None => (DataSourceDraft::new(id, String::new()), requested_scope),
            }
        };
        *self.scope.borrow_mut() = found_scope;
        *self.session_password.borrow_mut() = None;
        *self.saved.borrow_mut() = Some(draft.clone());
        *self.draft.borrow_mut() = Some(draft);
    }

    pub fn fields(&self) -> ffi::FfiDataSourceFields {
        match self.draft.borrow().as_ref() {
            Some(draft) => to_ffi_fields(draft),
            None => ffi::FfiDataSourceFields::default(),
        }
    }

    pub fn problems(&self) -> Vec<ffi::FfiDataSourceProblem> {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return Vec::new();
        };
        let other_ids: Vec<String> = sources_with_scope()
            .into_iter()
            .map(|(setting, _)| setting.id)
            .filter(|existing| existing != &draft.id)
            .collect();
        settings_model::database::validate(draft, &other_ids)
            .into_iter()
            .map(to_ffi_problem)
            .collect()
    }

    pub fn set_name(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.name = value.to_string();
        }
    }

    pub fn set_driver(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.driver = value.to_string();
        }
    }

    /// The current draft's driver's own [`db_core::console::
    /// database_field_label_key`] (F7b's Data Source dialog task) — the
    /// dialog re-reads this whenever the driver combo changes, and maps
    /// the key to a `tr()`'d label itself (ADR-0049), never showing this
    /// string verbatim.
    pub fn database_field_label_key(&self) -> QString {
        let driver = self
            .draft
            .borrow()
            .as_ref()
            .map(|draft| draft.driver.clone())
            .unwrap_or_default();
        let family = db_core::console::family_for_driver(&driver);
        QString::from(db_core::console::database_field_label_key(family))
    }

    pub fn set_group(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.group = value.to_string();
        }
    }

    pub fn set_color(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.color = value.to_string();
        }
    }

    pub fn set_host(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.host = value.to_string();
        }
    }

    pub fn set_port(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.port = port_from_text(value);
        }
    }

    pub fn set_database(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.database = value.to_string();
        }
    }

    pub fn set_user(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.user = value.to_string();
        }
    }

    pub fn set_auth(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.auth = value.to_string();
        }
    }

    pub fn set_read_only(&self, value: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.read_only = value;
        }
    }

    pub fn set_history(&self, value: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.history = value;
        }
    }

    pub fn set_url(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.url = value.to_string();
        }
    }

    pub fn set_ssl_mode(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.ssl_mode = value.to_string();
        }
    }

    pub fn set_ssl_ca_file(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.ssl_ca_file = value.to_string();
        }
    }

    pub fn set_ssh_host(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.ssh_host = value.to_string();
        }
    }

    pub fn set_ssh_port(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.ssh_port = port_from_text(value);
        }
    }

    pub fn set_ssh_user(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.ssh_user = value.to_string();
        }
    }

    pub fn set_ssh_auth(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.ssh_auth = value.to_string();
        }
    }

    pub fn set_ssh_key_file(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.ssh_key_file = value.to_string();
        }
    }

    pub fn is_dirty(&self) -> bool {
        *self.draft.borrow() != *self.saved.borrow()
    }

    pub fn commit(&self) -> FfiResult {
        let Some(draft) = self.draft.borrow().clone() else {
            return FfiResult::default();
        };
        let other_ids: Vec<String> = sources_with_scope()
            .into_iter()
            .map(|(setting, _)| setting.id)
            .filter(|existing| existing != &draft.id)
            .collect();
        if !settings_model::database::validate(&draft, &other_ids).is_empty() {
            return errors::failure(errors::CODE_REFUSED, "fix the highlighted fields first");
        }
        // `settings_model::database::commit` builds a fresh row from the
        // dialog's own draft, which has no `script_policy` field (that's
        // the console bar's own policy selector, not this dialog's) — carry
        // whatever the source already had forward so editing a source here
        // never silently resets it.
        let mut setting = settings_model::database::commit(&draft);
        if let Some((existing, _)) = sources_with_scope()
            .into_iter()
            .find(|(row, _)| row.id == setting.id)
        {
            setting.script_policy = existing.script_policy;
        }
        let previous_read_only = self.saved.borrow().as_ref().map(|s| s.read_only);
        let source_id = setting.id.clone();
        let new_read_only = setting.read_only;
        let result = upsert_source(setting, *self.scope.borrow());
        if result.code == errors::CODE_OK {
            *self.saved.borrow_mut() = Some(draft);
            // F4a residual (database-tools.md §11): a read-only flip must
            // reach every console already attached to this source, not
            // just future ones — `rebuild_guards_for_source`'s own doc
            // comment.
            if previous_read_only != Some(new_read_only) {
                super::edit::rebuild_guards_for_source(
                    &crate::bridge::registry::shared_database_consoles(),
                    &source_id,
                    new_read_only,
                );
            }
        }
        result
    }

    /// Whether a password is currently reachable for this draft — the
    /// keychain, or this session's fallback when it was unavailable.
    pub fn has_password(&self) -> bool {
        let id = self.draft.borrow().as_ref().map(|d| d.id.clone());
        let Some(id) = id else {
            return false;
        };
        secrets().has(&id) || self.session_password.borrow().is_some()
    }

    /// Store `value` in the keychain; degrades to a session-only value
    /// (never written to disk) when no keychain is reachable, and reports
    /// that hint through [`Self::password_hint`] rather than failing the
    /// call outright — a missing keychain is not a reason to refuse
    /// typing a password that still works for the rest of this session.
    pub fn set_password(&self, value: &QString) {
        let Some(id) = self.draft.borrow().as_ref().map(|d| d.id.clone()) else {
            return;
        };
        let text = value.to_string();
        match secrets().store(&id, &text) {
            Ok(()) => *self.session_password.borrow_mut() = None,
            Err(secret_store::SecretError::Unavailable(_)) => {
                *self.session_password.borrow_mut() = Some(text);
            }
            Err(_) => {}
        }
    }

    /// Empty when the keychain is reachable (or nothing has been typed
    /// yet); otherwise [`secret_store::NO_KEYCHAIN_HINT`], for the dialog
    /// to show as an informational label beside the password field.
    pub fn password_hint(&self) -> QString {
        let id = self.draft.borrow().as_ref().map(|d| d.id.clone());
        let Some(id) = id else {
            return QString::default();
        };
        match secrets().load(&id) {
            Err(secret_store::SecretError::Unavailable(hint)) => QString::from(hint.as_str()),
            _ => QString::default(),
        }
    }

    /// Attempt a real connection with the draft's current fields, off the
    /// UI thread, reporting through `testConnectionFinished` — or, for an
    /// SSH tunnel whose host key `~/.ssh/known_hosts` has never seen,
    /// through `hostKeyPrompt` instead (F7b; this slot's own doc comment
    /// on `hostKeyPrompt` in `ffi.rs`). A result tagged with a generation
    /// older than the latest call is dropped (database-tools.md §4).
    pub fn test_connection(self: Pin<&mut Self>) {
        let Some(draft) = self.draft.borrow().clone() else {
            return;
        };
        let id = draft.id.clone();
        let password = secrets()
            .load(&id)
            .ok()
            .flatten()
            .or_else(|| self.session_password.borrow().clone());
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        let guard = self.generation.clone();
        let qt_thread = self.qt_thread();
        let ssh_host = draft.ssh_host.clone();
        let ssh_port = draft.ssh_port.unwrap_or(22);

        let ssh_password = secrets().load(&format!("{id}/ssh")).ok().flatten();

        std::thread::spawn(move || {
            let setting = settings_model::database::commit(&draft);
            let db_secrets = db_core::datasource::Secrets {
                password,
                ssh_password,
                ..Default::default()
            };
            let source = db_core::datasource::DataSource::from_setting(&setting, &db_secrets);
            let result = crate::bridge::database::test_connection_typed(&source, &db_secrets);
            if guard.load(Ordering::SeqCst) != generation {
                return; // superseded by a newer test_connection() call
            }
            let _ = qt_thread.queue(move |mut editor: Pin<&mut ffi::DataSourceEditor>| {
                if let Err(error) = &result {
                    if error.code == db_core::error::DbErrorCode::HostKeyUnknown {
                        *editor.pending_host_key.borrow_mut() = Some((ssh_host.clone(), ssh_port));
                        editor.as_mut().host_key_prompt(
                            QString::from(ssh_host.as_str()),
                            ssh_port as i32,
                            QString::from(fingerprint_from_message(&error.message).as_str()),
                        );
                        return;
                    }
                }
                let (ok, message) = match result {
                    Ok(()) => (true, "Connected.".to_string()),
                    Err(error) => (false, error.message),
                };
                editor
                    .as_mut()
                    .test_connection_finished(ok, QString::from(message.as_str()));
            });
        });
    }

    /// "Accept and add to known_hosts" (F7b): records the pending key
    /// `db_drivers::ssh::check_server_key` stashed for the last
    /// `hostKeyPrompt`, then retries `testConnection` — the dialog itself
    /// never opens a tunnel; it only fixes `known_hosts` and asks for the
    /// same attempt again.
    pub fn accept_host_key(self: Pin<&mut Self>) -> FfiResult {
        let Some((host, port)) = self.pending_host_key.borrow_mut().take() else {
            return errors::failure(
                errors::CODE_REFUSED,
                "no host key prompt is pending for this editor",
            );
        };
        if let Err(error) = db_drivers::ssh::accept_host_key(&host, port) {
            return errors::failure(errors::CODE_REFUSED, error.message);
        }
        self.test_connection();
        FfiResult::default()
    }
}

/// The `SHA256:…` fingerprint out of `db_drivers::ssh`'s own
/// `HostKeyUnknown`/`HostKeyMismatch` message text (`"the server's host
/// key (SHA256:…) is not in ~/.ssh/known_hosts"`) — the dialog shows only
/// the fingerprint, not the whole sentence a second time. Falls back to
/// the full message if the wrapping parentheses are ever missing (a
/// future wording change should not panic here, only show more text than
/// intended).
fn fingerprint_from_message(message: &str) -> String {
    match (message.find('('), message.find(')')) {
        (Some(open), Some(close)) if open < close => message[open + 1..close].to_string(),
        _ => message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_id_names_match_the_settings_dialog_vocabulary() {
        assert_eq!(scope_id(Scope::Global), "global");
        assert_eq!(scope_id(Scope::Project), "project");
        assert_eq!(scope_id(Scope::Default), "global");
    }

    #[test]
    fn fingerprint_from_message_reads_the_parenthesised_text() {
        assert_eq!(
            fingerprint_from_message(
                "the server's host key (SHA256:abc123) is not in ~/.ssh/known_hosts"
            ),
            "SHA256:abc123"
        );
    }

    #[test]
    fn fingerprint_from_message_falls_back_to_the_whole_message_without_parentheses() {
        assert_eq!(
            fingerprint_from_message("connection refused"),
            "connection refused"
        );
    }

    #[test]
    fn port_text_round_trips_through_from_text() {
        assert_eq!(port_from_text(&port_text(Some(5432))), Some(5432));
        assert_eq!(port_text(None).to_string(), "");
        assert_eq!(port_from_text(&QString::from("not-a-number")), None);
    }

    #[test]
    fn generated_ids_are_unique_across_calls() {
        let a = generate_id();
        let b = generate_id();
        assert_ne!(a, b);
    }
}
