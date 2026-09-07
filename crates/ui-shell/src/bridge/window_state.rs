//! The window's own persisted state: geometry, whether it was maximized, the
//! dock blob and the editor split layout.
//!
//! Its own module rather than more of `settings.rs` for the reason
//! `bridge/mod.rs` gives — one module per feature — and because that file is
//! at its ADR-0025 size baseline. These four pairs are one feature: what
//! `IdeMainWindow::closeEvent` writes and what startup reads back.
//!
//! Three of them cross the seam as opaque blobs the view owns the format of
//! (ADR-0003 allows it precisely here: nothing in `app-core` models a dock
//! area or a splitter tree, so there is no domain type to translate into).
//! Named layouts, which persist the same two blobs under a name, are
//! [`crate::bridge::layouts`].

use cxx_qt_lib::QString;

use crate::bridge::ffi::{self, FfiWindowGeometry};

impl ffi::AppSettings {
    pub fn window_geometry(&self) -> FfiWindowGeometry {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        let g = settings.window_geometry;
        FfiWindowGeometry {
            x: g.x,
            y: g.y,
            width: g.width,
            height: g.height,
        }
    }

    pub fn save_window_geometry(&self, x: i32, y: i32, width: u32, height: u32) {
        let geometry = app_config::WindowGeometry {
            x,
            y,
            width,
            height,
        };
        // A window on its way out can report a 0x0 rect; persisting it would
        // replace a usable saved size with one the next launch has to throw
        // away. Keeping the previous geometry is the better answer.
        if !geometry.is_usable() {
            return;
        }
        let _ = app_config::update(&app_core::resolve_config_dir(), |settings| {
            settings.window_geometry = geometry;
        });
    }

    pub fn window_maximized(&self) -> bool {
        app_config::load(&app_core::resolve_config_dir())
            .unwrap_or_default()
            .window_maximized
    }

    pub fn save_window_maximized(&self, maximized: bool) {
        let config_dir = app_core::resolve_config_dir();
        let _ = app_config::update(&config_dir, |settings| {
            settings.window_maximized = maximized;
        });
    }

    pub fn window_state(&self) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(settings.window_state.as_str())
    }

    pub fn save_window_state(&self, state: &QString) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.window_state = state.to_string();
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn editor_layout(&self) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(settings.editor_layout.as_str())
    }

    pub fn save_editor_layout(&self, layout: &QString) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.editor_layout = layout.to_string();
        let _ = app_config::save(&config_dir, &settings);
    }
}
