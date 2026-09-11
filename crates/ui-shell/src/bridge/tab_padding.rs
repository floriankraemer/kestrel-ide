//! Air around an editor tab's label, per side (`[tab_padding]`): the
//! `AppSettings` slots behind Settings > Tabs.
//!
//! Its own module rather than more of `settings.rs`, for the reason
//! `bridge/mod.rs` gives — one module per feature — and because that file is
//! at its ADR-0025 size baseline, the same reason `layouts.rs` split out.

use crate::bridge::errors;
use crate::bridge::ffi::{self, FfiResult};
use crate::bridge::settings::commit_to_project;

impl ffi::AppSettings {
    /// The `[tab_padding]` section of the layer the settings dialog is
    /// currently editing, resolved to real numbers — never absent — the
    /// same "defaulted, not copied from the other layer" rule
    /// `terminal_settings` follows.
    pub fn tab_padding(&self) -> ffi::FfiTabPadding {
        let padding = match *self.scope.borrow() {
            settings_model::Scope::Project => crate::bridge::convert::load_project_settings()
                .tab_padding
                .unwrap_or_default(),
            _ => crate::bridge::convert::load_settings().tab_padding,
        };
        ffi::FfiTabPadding {
            top: padding.top_or_default(),
            bottom: padding.bottom_or_default(),
            left: padding.left_or_default(),
            right: padding.right_or_default(),
        }
    }

    /// Write the `[tab_padding]` section back to the layer being edited.
    ///
    /// Refuses — rather than clamping — a side over
    /// [`app_config::tab_padding::MAX_TAB_PADDING`], the one place this
    /// section asks for an error instead of the silent clamp `[editing]`'s
    /// own bounded fields use. The spin boxes on the page already stop the
    /// user from typing such a value; this is the seam's own guard, for
    /// whatever reaches it another way.
    pub fn save_tab_padding(&self, padding: &ffi::FfiTabPadding) -> FfiResult {
        let padding = app_config::TabPaddingSettings {
            top: Some(padding.top),
            bottom: Some(padding.bottom),
            left: Some(padding.left),
            right: Some(padding.right),
        };
        if let Err(error) = padding.validate() {
            return errors::failure(errors::CODE_INVALID_ARGUMENT, error.to_string());
        }
        if *self.scope.borrow() == settings_model::Scope::Project {
            let section = (padding != app_config::TabPaddingSettings::default()).then_some(padding);
            return commit_to_project(|project| project.tab_padding = section);
        }
        let config_dir = app_core::resolve_config_dir();
        match app_config::update(&config_dir, |settings| settings.tab_padding = padding) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }

    /// The air around an editor tab's label actually in force: the global
    /// default with the open project's `[tab_padding]` override applied, if
    /// it has one (ADR-0022). Distinct from `tab_padding()`, which follows
    /// `settingsScope()` for whichever layer the dialog is *editing* —
    /// `applyTheme()`'s caller wants what the app actually renders with,
    /// the same split `terminal_font()` makes from `terminal_settings()`.
    pub fn resolved_tab_padding(&self) -> ffi::FfiTabPadding {
        let padding = crate::bridge::convert::load_resolved_settings().tab_padding;
        ffi::FfiTabPadding {
            top: padding.top_or_default(),
            bottom: padding.bottom_or_default(),
            left: padding.left_or_default(),
            right: padding.right_or_default(),
        }
    }
}
