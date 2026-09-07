//! Named workspace layouts (ADR-0045): the `AppSettings` slots behind the
//! View > Layouts menu.
//!
//! Its own module rather than more of `settings.rs` for the reason
//! `bridge/mod.rs` gives — one module per feature — and because that file is
//! at its ADR-0025 size baseline. The slots still hang off `AppSettings`:
//! a layout is settings-shaped (two files, project over global), and a
//! second QObject for four slots would be seam for its own sake.

use cxx_qt_lib::{QString, QStringList};

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiLayout, FfiResult};
use crate::bridge::settings::{commit_to_project, scope_from_name};

/// The layouts the user can pick from: the global set merged with the open
/// project's, resolved by `settings-model` rather than here — precedence is a
/// rule, and the adapter holds none (ADR-0022 §3).
fn resolved_layouts() -> Vec<(String, app_config::Layout, settings_model::Scope)> {
    settings_model::scope::resolve_layouts(
        &crate::bridge::convert::load_settings(),
        &crate::bridge::convert::load_project_settings(),
    )
}

impl ffi::AppSettings {
    pub fn layout_names(&self) -> QStringList {
        resolved_layouts()
            .into_iter()
            .map(|(name, _, _)| QString::from(name.as_str()))
            .collect()
    }

    pub fn save_named_layout(
        &self,
        name: &QString,
        scope: &QString,
        window_state: &QString,
        editor_grid: &QString,
    ) -> FfiResult {
        let name = name.to_string().trim().to_string();
        if name.is_empty() {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                "A layout needs a name — nothing was saved.",
            );
        }
        let layout = app_config::Layout {
            window_state: window_state.to_string(),
            editor_grid: editor_grid.to_string(),
        };

        match scope_from_name(&scope.to_string()) {
            settings_model::Scope::Project => commit_to_project(|project| {
                project
                    .layouts
                    .get_or_insert_with(Default::default)
                    .insert(name, layout);
            }),
            // `Scope::Default` is not a layer anything writes to; anything
            // that is not the project layer is the person's own.
            _ => {
                let config_dir = app_core::resolve_config_dir();
                match app_config::update(&config_dir, |settings| {
                    settings.layouts.insert(name, layout);
                }) {
                    Ok(()) => FfiResult::default(),
                    Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
                }
            }
        }
    }

    pub fn named_layout(&self, name: &QString) -> FfiLayout {
        let name = name.to_string();
        let Some((_, layout, _)) = resolved_layouts()
            .into_iter()
            .find(|(candidate, _, _)| *candidate == name)
        else {
            return FfiLayout {
                code: errors::CODE_UNKNOWN_LAYOUT,
                message: QString::from(
                    format!("There is no layout named \u{201c}{name}\u{201d}.").as_str(),
                ),
                ..Default::default()
            };
        };
        FfiLayout {
            code: errors::CODE_OK,
            message: QString::default(),
            window_state: QString::from(layout.window_state.as_str()),
            editor_grid: QString::from(layout.editor_grid.as_str()),
        }
    }

    pub fn delete_named_layout(&self, name: &QString) -> FfiResult {
        let name = name.to_string();
        // The project layer first, and only that layer when it defines the
        // name: a project entry shadows a global one, so deleting the entry
        // the user is looking at reveals the global layout underneath rather
        // than silently taking both.
        if crate::bridge::convert::load_project_settings()
            .layouts
            .as_ref()
            .is_some_and(|layouts| layouts.contains_key(&name))
        {
            return commit_to_project(|project| {
                if let Some(layouts) = project.layouts.as_mut() {
                    layouts.remove(&name);
                }
            });
        }

        let config_dir = app_core::resolve_config_dir();
        let mut existed = false;
        match app_config::update(&config_dir, |settings| {
            existed = settings.layouts.remove(&name).is_some();
        }) {
            Ok(()) if !existed => errors::failure(
                errors::CODE_UNKNOWN_LAYOUT,
                format!("There is no layout named \u{201c}{name}\u{201d}."),
            ),
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }
}
