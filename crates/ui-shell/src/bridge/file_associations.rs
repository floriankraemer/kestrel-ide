//! Rust side of the `FileAssociationsEditor` QObject: the Settings > File
//! Associations page's rows.
//!
//! Live-effect, like `PluginCatalog`/`LanguageCatalog`: there is no draft
//! and no OK-shaped promise, and every add/remove/edit writes straight to
//! disk. What a pattern matches and what a handler name means are
//! `settings_model::file_associations`'s answers; this file only moves rows
//! across the seam.

use cxx_qt_lib::QString;

use crate::bridge::convert;
use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiFileAssociationRule, FfiResult};
use crate::bridge::settings::commit_to_project;

#[derive(Default)]
pub struct FileAssociationsEditorRust;

fn to_ffi_rules(rules: &[app_config::FileAssociationRule]) -> Vec<FfiFileAssociationRule> {
    rules
        .iter()
        .map(|rule| FfiFileAssociationRule {
            pattern: QString::from(rule.pattern.as_str()),
            handler: QString::from(rule.handler.as_str()),
        })
        .collect()
}

/// The inverse of [`to_ffi_rules`]. A row whose pattern is blank is dropped
/// rather than persisted — the same "a half-typed row is normal to find in
/// a free-text table" rule [`crate::bridge::settings::parse_env_lines`]
/// applies to the Terminal page's environment list.
fn from_ffi_rules(rules: &[FfiFileAssociationRule]) -> Vec<app_config::FileAssociationRule> {
    rules
        .iter()
        .map(|row| app_config::FileAssociationRule {
            pattern: row.pattern.to_string().trim().to_string(),
            handler: row.handler.to_string(),
        })
        .filter(|rule| !rule.pattern.is_empty())
        .collect()
}

impl ffi::FileAssociationsEditor {
    /// Whether a project is open — the page disables its Project tab when
    /// there is nowhere to write a project-scoped rule.
    pub fn has_project(&self) -> bool {
        convert::current_project_root().is_some()
    }

    /// Every [`settings_model::file_associations::HandlerKind`] the page may
    /// offer, one per line, in display order — the combo's own vocabulary,
    /// read from the one place that owns it rather than duplicated here.
    pub fn handler_names(&self) -> QString {
        let names: Vec<&str> = settings_model::file_associations::ALL_HANDLERS
            .iter()
            .map(|kind| settings_model::file_associations::handler_name(*kind))
            .collect();
        QString::from(names.join("\n").as_str())
    }

    pub fn global_rules(&self) -> Vec<FfiFileAssociationRule> {
        to_ffi_rules(&convert::load_settings().file_associations.rules)
    }

    /// Whether the open project overrides the global rules at all — distinct
    /// from "overrides with an empty list", the same `None`-is-not-`Some(vec![])`
    /// rule every other project-scoped section in `app-config` follows.
    pub fn project_overrides(&self) -> bool {
        convert::load_project_settings().file_associations.is_some()
    }

    pub fn project_rules(&self) -> Vec<FfiFileAssociationRule> {
        convert::load_project_settings()
            .file_associations
            .map(|associations| to_ffi_rules(&associations.rules))
            .unwrap_or_default()
    }

    /// Replaces the global `[[file_associations.rule]]` list. Called with
    /// the whole table after any add/remove/edit — the page has no partial
    /// update, the same as `PluginCatalog::set_disabled` rewriting the whole
    /// `disabled_plugins` list on one toggle.
    pub fn set_global_rules(&self, rules: Vec<FfiFileAssociationRule>) -> FfiResult {
        let config_dir = app_core::resolve_config_dir();
        let rules = from_ffi_rules(&rules);
        let mut settings = match app_config::load(&config_dir) {
            Ok(settings) => settings,
            Err(err) => return errors::failure(errors::CODE_SETTINGS_IO, err.to_string()),
        };
        settings.file_associations = app_config::FileAssociationSettings { rules };
        match app_config::save(&config_dir, &settings) {
            Ok(()) => FfiResult::default(),
            Err(err) => errors::failure(errors::CODE_SETTINGS_IO, err.to_string()),
        }
    }

    /// Turns the project's override on or off without touching its rules —
    /// the checkbox the table's rows are otherwise disabled underneath.
    pub fn set_project_overrides(&self, overrides: bool) -> FfiResult {
        commit_to_project(|project| {
            if overrides {
                project
                    .file_associations
                    .get_or_insert_with(app_config::FileAssociationSettings::default);
            } else {
                project.file_associations = None;
            }
        })
    }

    /// Replaces the project's rule list. Only meaningful once
    /// [`Self::set_project_overrides`] has turned the override on; called
    /// with `overrides` already true otherwise this would silently create
    /// an override the checkbox never asked for, so it refuses instead.
    pub fn set_project_rules(&self, rules: Vec<FfiFileAssociationRule>) -> FfiResult {
        if !self.project_overrides() {
            return errors::failure(
                errors::CODE_REFUSED,
                "Turn on the project override before editing its rules.",
            );
        }
        let rules = from_ffi_rules(&rules);
        commit_to_project(|project| {
            project.file_associations = Some(app_config::FileAssociationSettings { rules });
        })
    }
}
