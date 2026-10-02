//! `PhpSettingsEditor` (PHP parity plan, I7): the Settings > PHP page's
//! draft, following `AnalysisEditor`'s begin_edit(scope)/…/commit shape.
//!
//! Translation only: what a field means is `settings_model::php`'s
//! (`PhpForm`), what a probe reports is `php_core::probe`'s. The one thing
//! owned here is the pending licence-key change, which is not a setting —
//! the key goes to `secret-store` on commit and never into a
//! `settings.toml` that may be committed (ADR-0068).

mod composer;

use std::cell::RefCell;
use std::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use app_config::php::PhpSettings;
use settings_model::php::PhpForm;

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};

pub use composer::ComposerServiceRust;

/// The host a PHP-aware tool runs on (ADR-0067): the interpreter's, when
/// the tool `requires_interpreter`, else the project's own.
pub(crate) fn tool_host(
    requires_interpreter: Option<&str>,
    settings: &app_config::Settings,
    root: &std::path::Path,
) -> process_exec::host::ExecHost {
    match requires_interpreter {
        Some(_) => php_core::host::interpreter_host(&settings.php, &settings.containers, root),
        None => process_exec::host::ExecHost::for_path(root),
    }
}

/// A licence-key edit waiting for the dialog's OK.
#[derive(Debug, Clone, PartialEq, Eq)]
enum LicenceChange {
    Set(String),
    Remove,
}

#[derive(Default)]
pub struct PhpSettingsEditorRust {
    draft: RefCell<PhpSettings>,
    scope: RefCell<settings_model::Scope>,
    licence: RefCell<Option<LicenceChange>>,
}

fn secrets() -> secret_store::SecretStore {
    secret_store::SecretStore::new(php_core::SECRET_SERVICE)
}

static LICENCE_KEY: php_core::licence::LicenceCache = php_core::licence::LicenceCache::new();

/// The Intelephense licence key: the keychain on the first call, the cache
/// after (the settings page keeps it current). Unreadable reads as no key.
pub(crate) fn licence_key() -> Option<String> {
    LICENCE_KEY.get(|| {
        secrets()
            .load(php_core::INTELEPHENSE_LICENCE_ID)
            .ok()
            .flatten()
    })
}

/// Read the key on a worker, so the first use on the Qt thread finds it
/// cached instead of waiting on a slow keychain.
pub(crate) fn prime_licence_key() {
    std::thread::spawn(licence_key);
}

fn to_ffi_form(form: &PhpForm) -> ffi::FfiPhpForm {
    ffi::FfiPhpForm {
        interpreter: QString::from(form.interpreter.as_str()),
        language_level: QString::from(form.language_level.as_str()),
        include_paths: QString::from(form.include_paths.as_str()),
        stubs: QString::from(form.stubs.as_str()),
        container_target: QString::from(form.container_target.as_str()),
        container_mode: QString::from(form.container_mode.as_str()),
        formatter: QString::from(form.formatter.as_str()),
        intelephense_enabled: form.intelephense_enabled,
        intelephense_diagnostics: form.intelephense_diagnostics,
        phpactor_enabled: form.phpactor_enabled,
        phpactor_diagnostics: form.phpactor_diagnostics,
    }
}

fn from_ffi_form(form: &ffi::FfiPhpForm) -> PhpForm {
    PhpForm {
        interpreter: form.interpreter.to_string(),
        language_level: form.language_level.to_string(),
        include_paths: form.include_paths.to_string(),
        stubs: form.stubs.to_string(),
        container_target: form.container_target.to_string(),
        container_mode: form.container_mode.to_string(),
        formatter: form.formatter.to_string(),
        intelephense_enabled: form.intelephense_enabled,
        intelephense_diagnostics: form.intelephense_diagnostics,
        phpactor_enabled: form.phpactor_enabled,
        phpactor_diagnostics: form.phpactor_diagnostics,
    }
}

fn to_ffi_probe(
    result: Result<php_core::probe::PhpProbe, php_core::probe::ProbeError>,
    in_container: bool,
) -> ffi::FfiPhpProbe {
    match result {
        Ok(probe) => ffi::FfiPhpProbe {
            ok: true,
            version: QString::from(probe.version.as_str()),
            ini_file: QString::from(probe.ini_file.clone().unwrap_or_default().as_str()),
            xdebug: probe.xdebug,
            xdebug_modes: QString::from(probe.xdebug_modes.join(", ").as_str()),
            pcov: probe.pcov,
            xdebug_advice: QString::from(
                probe
                    .xdebug_issue()
                    .map(|issue| issue.advice(in_container))
                    .unwrap_or_default()
                    .as_str(),
            ),
            error: QString::default(),
        },
        Err(error) => ffi::FfiPhpProbe {
            error: QString::from(error.to_string().as_str()),
            ..ffi::FfiPhpProbe::default()
        },
    }
}

impl ffi::PhpSettingsEditor {
    /// Load the draft from `scope` — `"global"` or `"project"`.
    pub fn begin_edit(&self, scope: &QString) {
        let scope = crate::bridge::settings::scope_from_name(&scope.to_string());
        *self.scope.borrow_mut() = scope;
        *self.draft.borrow_mut() = match scope {
            settings_model::Scope::Project => crate::bridge::convert::load_project_settings()
                .php
                .unwrap_or_default(),
            _ => {
                app_config::load(&app_core::resolve_config_dir())
                    .unwrap_or_default()
                    .php
            }
        };
        *self.licence.borrow_mut() = None;
    }

    pub fn form(&self) -> ffi::FfiPhpForm {
        to_ffi_form(&PhpForm::from_settings(&self.draft.borrow()))
    }

    pub fn formatter_choices(&self) -> Vec<ffi::FfiFormatterChoice> {
        let installed = plugin_host::registry()
            .formatters()
            .filter(|(_, f)| f.languages.iter().any(|l| l == "php"))
            .map(|(_, f)| (f.id.clone(), f.name.clone()))
            .collect();
        settings_model::php::formatter_choices(installed, self.draft.borrow().formatter.as_deref())
            .into_iter()
            .map(|choice| ffi::FfiFormatterChoice {
                id: QString::from(choice.id.as_str()),
                name: QString::from(choice.name.as_str()),
                installed: choice.installed,
            })
            .collect()
    }

    pub fn set_form(&self, form: &ffi::FfiPhpForm) -> FfiResult {
        let form = from_ffi_form(form);
        if let Err(error) = form.validate() {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, error.to_string());
        }
        form.apply_to(&mut self.draft.borrow_mut());
        FfiResult::default()
    }

    pub fn has_licence_key(&self) -> bool {
        match &*self.licence.borrow() {
            Some(LicenceChange::Set(_)) => true,
            Some(LicenceChange::Remove) => false,
            None => secrets().has(php_core::INTELEPHENSE_LICENCE_ID),
        }
    }

    pub fn set_licence_key(&self, key: &QString) {
        let key = key.to_string();
        let key = key.trim();
        // Emptying the field again cancels the edit; removing a stored key
        // is the explicit `remove_licence_key`.
        *self.licence.borrow_mut() = (!key.is_empty()).then(|| LicenceChange::Set(key.to_string()));
    }

    pub fn remove_licence_key(&self) {
        *self.licence.borrow_mut() = Some(LicenceChange::Remove);
    }

    /// Probe on a worker: the interpreter may be slow to start, and a
    /// wedged one is bounded by `php_core::probe`'s own timeout, not by the
    /// Qt thread.
    pub fn probe_interpreter(mut self: Pin<&mut Self>, interpreter: &QString) {
        let interpreter = interpreter.to_string();
        let interpreter = match interpreter.trim() {
            "" => settings_model::php::DEFAULT_INTERPRETER.to_string(),
            trimmed => trimmed.to_string(),
        };
        let root =
            crate::bridge::convert::current_project_root().unwrap_or_else(std::env::temp_dir);
        // The draft's container choice, not the saved one: the user is
        // editing it and presses Detect to see whether it works.
        let host = php_core::host::interpreter_host(
            &self.draft.borrow(),
            &crate::bridge::convert::load_resolved_settings().containers,
            &root,
        );
        let in_container = matches!(host, process_exec::host::ExecHost::Container(_));
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let result = to_ffi_probe(
                php_core::probe::probe(&host, &interpreter, &root),
                in_container,
            );
            let _ = qt_thread.queue(move |mut editor: Pin<&mut Self>| {
                editor.as_mut().probe_finished(result);
            });
        });
    }

    pub fn commit(&self) -> FfiResult {
        let draft = self.draft.borrow().clone();
        let saved = if *self.scope.borrow() == settings_model::Scope::Project {
            crate::bridge::settings::commit_to_project(move |project| {
                project.php = (!app_config::php::is_default(&draft)).then_some(draft);
            })
        } else {
            let config_dir = app_core::resolve_config_dir();
            match app_config::load(&config_dir) {
                Ok(mut settings) => {
                    settings.php = draft;
                    match app_config::save(&config_dir, &settings) {
                        Ok(()) => FfiResult::default(),
                        Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
                    }
                }
                Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
            }
        };
        if saved.code != 0 {
            return saved;
        }
        let change = self.licence.borrow_mut().take();
        let outcome = match change {
            Some(LicenceChange::Set(key)) => {
                let stored = secrets().store(php_core::INTELEPHENSE_LICENCE_ID, &key);
                if stored.is_ok() {
                    LICENCE_KEY.set(Some(key));
                }
                stored
            }
            Some(LicenceChange::Remove) => {
                let removed = secrets().delete(php_core::INTELEPHENSE_LICENCE_ID);
                if removed.is_ok() {
                    LICENCE_KEY.set(None);
                }
                removed
            }
            None => Ok(()),
        };
        match outcome {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }
}
