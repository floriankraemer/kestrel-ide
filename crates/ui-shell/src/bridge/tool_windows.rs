//! Rust side of `AppSettings::contributedToolWindows`/`contributedSettingsPages`
//! (the database-tools plan's G1): translation only, from
//! `plugin_host::registry()`'s typed contributions to the FFI rows
//! `tool_window_factories.cpp`'s dock-factory loop and `settings_dialog.cpp`'s
//! page-factory loop each iterate.
//!
//! `plugin_host::registry()` rather than a fresh `plugin_host::load(...)`
//! scan (contrast `PluginCatalog::refresh`, which deliberately does scan
//! unfiltered so a disabled plugin still gets a row to re-enable): here, the
//! *opposite* is wanted — a disabled plugin's dock and settings page must be
//! absent, which the live, already-filtered registry gives for free.

use cxx_qt_lib::QString;

use crate::bridge::ffi;

fn to_ffi_area(area: plugin_api::ToolWindowArea) -> ffi::FfiToolWindowArea {
    match area {
        plugin_api::ToolWindowArea::Left => ffi::FfiToolWindowArea::Left,
        plugin_api::ToolWindowArea::Right => ffi::FfiToolWindowArea::Right,
        plugin_api::ToolWindowArea::Bottom => ffi::FfiToolWindowArea::Bottom,
        plugin_api::ToolWindowArea::Center => ffi::FfiToolWindowArea::Center,
    }
}

fn to_ffi_scope(scope: plugin_api::SettingsPageScope) -> ffi::FfiSettingsPageScope {
    match scope {
        plugin_api::SettingsPageScope::Global => ffi::FfiSettingsPageScope::Global,
        plugin_api::SettingsPageScope::Project => ffi::FfiSettingsPageScope::Project,
    }
}

impl ffi::AppSettings {
    pub fn contributed_tool_windows(&self) -> Vec<ffi::FfiToolWindow> {
        plugin_host::registry()
            .tool_windows()
            .map(|(plugin, window)| ffi::FfiToolWindow {
                plugin_id: QString::from(plugin.id()),
                id: QString::from(window.id.as_str()),
                title: QString::from(window.title.as_str()),
                area: to_ffi_area(window.area),
            })
            .collect()
    }

    pub fn contributed_settings_pages(&self) -> Vec<ffi::FfiSettingsPage> {
        plugin_host::registry()
            .settings_pages()
            .map(|(plugin, page)| ffi::FfiSettingsPage {
                plugin_id: QString::from(plugin.id()),
                id: QString::from(page.id.as_str()),
                title: QString::from(page.title.as_str()),
                scope: to_ffi_scope(page.scope),
            })
            .collect()
    }
}
