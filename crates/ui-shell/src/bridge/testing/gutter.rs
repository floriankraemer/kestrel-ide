//! The test gutter's half of `TestService` (PHP parity plan T3): which
//! lines of a PHP file carry a Run/Debug marker, and starting the one
//! test a marker names.
//!
//! Translation only: where the markers go and what filter each runs is
//! `php_core::tests::markers`'s; the framework, its argv and its output
//! parsing are the ordinary `start` path's.

use std::pin::Pin;

use cxx_qt_lib::QString;

use super::{MarkerSelection, RerunSelection};
use crate::bridge::errors;
use crate::bridge::ffi;

fn is_php_file(path: &str) -> bool {
    syntax_core::language_for_path(std::path::Path::new(path)).id() == "php"
}

fn marker_at(path: &QString, text: &QString, line: u32) -> Option<MarkerSelection> {
    let path = path.to_string();
    if !is_php_file(&path) {
        return None;
    }
    php_core::tests::markers(&text.to_string())
        .into_iter()
        .find(|marker| marker.line == line)
        .map(|marker| MarkerSelection {
            is_class: marker.scope == php_core::tests::MarkerScope::Group,
            filter: marker.filter,
            name: marker.name,
            file: path,
        })
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
            Some(marker) => self.start(Some(RerunSelection::Marker(marker)), Vec::new(), false),
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
                *self.pending_debug.borrow_mut() = Some(marker);
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
        let Some(marker) = self.pending_debug.borrow_mut().take() else {
            return errors::failure(errors::CODE_REFUSED, "no test is waiting to be debugged");
        };
        self.start(Some(RerunSelection::Marker(marker)), env, false)
    }
}

impl ffi::TestService {
    pub fn run_all_with_coverage(self: Pin<&mut Self>) -> ffi::FfiResult {
        self.start(None, Vec::new(), true)
    }

    pub fn run_marker_with_coverage(
        self: Pin<&mut Self>,
        path: &QString,
        text: &QString,
        line: u32,
    ) -> ffi::FfiResult {
        match marker_at(path, text, line) {
            Some(marker) => self.start(Some(RerunSelection::Marker(marker)), Vec::new(), true),
            None => no_marker(),
        }
    }

    /// 0-based lines of `path` whose hit count satisfies `wanted`.
    fn coverage_lines(&self, path: &QString, wanted: impl Fn(u64) -> bool) -> Vec<u32> {
        let path = path.to_string();
        self.coverage
            .borrow()
            .as_ref()
            .and_then(|coverage| coverage.file(std::path::Path::new(&path)).cloned())
            .map(|file| {
                file.lines
                    .iter()
                    .filter(|(_, hits)| wanted(**hits))
                    .map(|(line, _)| line.saturating_sub(1))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn covered_lines(&self, path: &QString) -> Vec<u32> {
        self.coverage_lines(path, |hits| hits > 0)
    }

    pub fn uncovered_lines(&self, path: &QString) -> Vec<u32> {
        self.coverage_lines(path, |hits| hits == 0)
    }

    pub fn coverage_rows(&self) -> Vec<ffi::FfiCoverageRow> {
        let Some(root) = super::current_project_root() else {
            return Vec::new();
        };
        let coverage = self.coverage.borrow();
        let Some(coverage) = coverage.as_ref() else {
            return Vec::new();
        };
        coverage
            .rows(&root)
            .into_iter()
            .map(|row| ffi::FfiCoverageRow {
                path: QString::from(row.path.to_string_lossy().replace('\\', "/").as_str()),
                abs_path: QString::from(root.join(&row.path).to_string_lossy().as_ref()),
                is_file: row.is_file,
                covered: row.covered as u32,
                total: row.total as u32,
                percent: row.percent(),
                has_lines: row.has_lines(),
            })
            .collect()
    }

    pub fn clear_coverage(mut self: Pin<&mut Self>) {
        *self.coverage.borrow_mut() = None;
        self.as_mut().coverage_changed();
    }
}
