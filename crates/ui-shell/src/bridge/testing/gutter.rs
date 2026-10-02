//! The test gutter's half of `TestService` (PHP parity plan T3): which
//! lines of a PHP file carry a Run/Debug marker, and starting the one
//! test a marker names.
//!
//! Translation only: where the markers go and what filter each runs is
//! `php_core::tests::markers`'s; the framework, its argv and its output
//! parsing are the ordinary `start` path's.

use std::pin::Pin;

use cxx_qt_lib::QString;

use super::RerunSelection;
use crate::bridge::errors;
use crate::bridge::ffi;

fn is_php_file(path: &str) -> bool {
    syntax_core::language_for_path(std::path::Path::new(path)).id() == "php"
}

fn marker_at(path: &QString, text: &QString, line: u32) -> Option<php_core::tests::TestMarker> {
    let path = path.to_string();
    if !is_php_file(&path) {
        return None;
    }
    php_core::tests::markers(&text.to_string())
        .into_iter()
        .find(|marker| marker.line == line)
}

fn no_marker() -> ffi::FfiResult {
    errors::failure(errors::CODE_REFUSED, "there is no test on this line")
}

impl ffi::TestService {
    /// The 0-based lines of `text` that carry a test marker; empty for a
    /// file that is not a PHP test.
    pub fn marker_lines(&self, path: &QString, text: &QString) -> Vec<u32> {
        if !is_php_file(&path.to_string()) {
            return Vec::new();
        }
        php_core::tests::markers(&text.to_string())
            .into_iter()
            .map(|marker| marker.line)
            .collect()
    }

    /// The name a marker's menu shows, empty when `line` has no marker.
    pub fn marker_name(&self, path: &QString, text: &QString, line: u32) -> QString {
        marker_at(path, text, line)
            .map(|marker| QString::from(marker.name.as_str()))
            .unwrap_or_default()
    }

    /// Run the test the marker on `line` names.
    pub fn run_marker(
        self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> ffi::FfiResult {
        match marker_at(path, text, line) {
            Some(marker) => self.start(Some(RerunSelection::Pattern(marker.filter)), Vec::new()),
            None => no_marker(),
        }
    }

    /// Remember the test a Debug click chose, until the listener is up and
    /// [`Self::run_pending_with_env`] is called.
    pub fn prepare_debug_marker(
        self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> ffi::FfiResult {
        match marker_at(path, text, line) {
            Some(marker) => {
                *self.pending_debug.borrow_mut() = Some(marker.filter);
                ffi::FfiResult::default()
            }
            None => no_marker(),
        }
    }

    /// Start the remembered test with the Xdebug environment
    /// (`[[key, value], ...]`) `DebugService` computed.
    pub fn run_pending_with_env(self: Pin<&mut Self>, env_json: &QString) -> ffi::FfiResult {
        let Ok(env) = serde_json::from_str::<Vec<(String, String)>>(&env_json.to_string()) else {
            return errors::failure(
                errors::CODE_INVALID_ARGUMENT,
                "the extra environment is not a list of pairs",
            );
        };
        let Some(filter) = self.pending_debug.borrow_mut().take() else {
            return errors::failure(errors::CODE_REFUSED, "no test is waiting to be debugged");
        };
        self.start(Some(RerunSelection::Pattern(filter)), env)
    }
}
