//! `DiagnosticsService` (ADR-0046): the single QObject the Problems dock and
//! the editor's squiggles read.
//!
//! `LanguageService` and `BuildService` still publish into the shared
//! `diagnostics_core::DiagnosticStore` (`registry::SharedDiagnostics`) and
//! still emit their own `diagnosticsChanged` when they do — that signal
//! means "my part of the store changed", and the view already has to
//! listen to both a language server's and a build's to know when to
//! refresh. What used to be a second and third way to ask "what are the
//! diagnostics" — `LanguageService::diagnostics*` and
//! `BuildService::diagnostics` — is gone; this is the only place that
//! question is answered now.
//!
//! Translation only, per `docs/architecture/layering.md`: which rows exist,
//! their order and their severity ranking are `diagnostics-core`'s.

use cxx_qt_lib::QString;

use crate::bridge::ffi;
use crate::bridge::registry::SharedDiagnostics;

#[derive(Default)]
pub struct DiagnosticsServiceRust {
    store: SharedDiagnostics,
}

pub(crate) fn to_ffi_severity(severity: diagnostics_core::Severity) -> ffi::FfiSeverity {
    match severity {
        diagnostics_core::Severity::Error => ffi::FfiSeverity::Error,
        diagnostics_core::Severity::Warning => ffi::FfiSeverity::Warning,
        diagnostics_core::Severity::Information => ffi::FfiSeverity::Information,
        diagnostics_core::Severity::Hint => ffi::FfiSeverity::Hint,
    }
}

pub(crate) fn to_ffi_diagnostic(row: diagnostics_core::DiagnosticRow) -> ffi::FfiDiagnostic {
    ffi::FfiDiagnostic {
        path: QString::from(row.path.as_str()),
        line: row.line,
        column: row.column,
        end_line: row.end_line,
        end_column: row.end_column,
        severity: to_ffi_severity(row.severity),
        message: QString::from(row.message.as_str()),
        source: QString::from(row.source.as_str()),
    }
}

impl ffi::DiagnosticsService {
    /// Every diagnostic known, from every source, in the order the Problems
    /// dock renders them.
    pub fn diagnostics(&self) -> Vec<ffi::FfiDiagnostic> {
        self.store
            .borrow()
            .rows()
            .into_iter()
            .map(to_ffi_diagnostic)
            .collect()
    }

    /// The diagnostics for one file, from every source — what the editor
    /// underlines.
    pub fn diagnostics_for_file(&self, path: &QString) -> Vec<ffi::FfiDiagnostic> {
        let uri = diagnostics_core::uri_from_path(&path.to_string());
        self.store
            .borrow()
            .rows_for_uri(&uri)
            .into_iter()
            .map(to_ffi_diagnostic)
            .collect()
    }

    /// How many of each severity are known, across every source — the
    /// status bar's counter.
    pub fn diagnostic_counts(&self) -> ffi::FfiDiagnosticCounts {
        to_ffi_counts(self.store.borrow().counts())
    }

    /// R4/F2: the next diagnostic in `path` strictly after
    /// `(line, character)`, wrapping to the file's first.
    pub fn next_diagnostic(
        &self,
        path: &QString,
        line: u32,
        character: u32,
    ) -> ffi::FfiDiagnosticJump {
        let uri = diagnostics_core::uri_from_path(&path.to_string());
        to_ffi_jump(self.store.borrow().next_after(&uri, line, character))
    }

    /// R4/Shift+F2: the previous diagnostic in `path` strictly before
    /// `(line, character)`, wrapping to the file's last.
    pub fn previous_diagnostic(
        &self,
        path: &QString,
        line: u32,
        character: u32,
    ) -> ffi::FfiDiagnosticJump {
        let uri = diagnostics_core::uri_from_path(&path.to_string());
        to_ffi_jump(self.store.borrow().prev_before(&uri, line, character))
    }

    /// R4: how many of each severity `path` alone has — the error stripe's
    /// corner summary.
    pub fn diagnostic_summary(&self, path: &QString) -> ffi::FfiDiagnosticCounts {
        let uri = diagnostics_core::uri_from_path(&path.to_string());
        to_ffi_counts(self.store.borrow().summary(&uri))
    }
}

fn to_ffi_counts(counts: diagnostics_core::DiagnosticCounts) -> ffi::FfiDiagnosticCounts {
    ffi::FfiDiagnosticCounts {
        errors: counts.errors as u32,
        warnings: counts.warnings as u32,
        infos: counts.infos as u32,
        hints: counts.hints as u32,
    }
}

/// `found == false`'s other fields are meaningless (ADR-0003's typed-flag
/// shape) — `Severity::Hint`/an empty message are never read in that case,
/// only written to satisfy the struct.
fn to_ffi_jump(row: Option<diagnostics_core::DiagnosticRow>) -> ffi::FfiDiagnosticJump {
    match row {
        Some(row) => ffi::FfiDiagnosticJump {
            found: true,
            line: row.line,
            column: row.column,
            severity: to_ffi_severity(row.severity),
            message: QString::from(row.message.as_str()),
        },
        None => ffi::FfiDiagnosticJump {
            found: false,
            line: 0,
            column: 0,
            severity: ffi::FfiSeverity::Hint,
            message: QString::default(),
        },
    }
}
