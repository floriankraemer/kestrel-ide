//! Converting `textDocument/publishDiagnostics` into the shared model
//! (ADR-0046).
//!
//! `DiagnosticStore`/`DiagnosticRow`/`Severity` used to live here, keyed by
//! URI alone; they now live in `diagnostics-core`, keyed by `(source, uri)`
//! so a build tool's rows and a language server's rows for the same file
//! coexist. What stays here is what only `lsp-core` can do: reading LSP's
//! own wire shape (`Option<DiagnosticSeverity>`, a `Range` with both ends
//! always present) and carrying the diagnostic's raw JSON so
//! `LspManager::intentions` can hand a server back exactly what it sent —
//! several servers compute a code action from a diagnostic's own opaque
//! `data` field and refuse to offer one otherwise.

use diagnostics_core::{Diagnostic, Position, Range, Severity};
use lsp_types::DiagnosticSeverity;

pub use diagnostics_core::{path_from_uri, uri_from_path};

/// LSP leaves `severity` optional; a server that omits it is reporting a
/// problem, so the honest default is the one the user must look at.
fn severity_from_lsp(severity: Option<DiagnosticSeverity>) -> Severity {
    match severity {
        Some(DiagnosticSeverity::WARNING) => Severity::Warning,
        Some(DiagnosticSeverity::INFORMATION) => Severity::Information,
        Some(DiagnosticSeverity::HINT) => Severity::Hint,
        _ => Severity::Error,
    }
}

/// One `publishDiagnostics` payload's diagnostics, converted to the shared
/// model. The raw JSON is kept on every row: cheap (the `Vec` this comes
/// from is dropped right after), and `diagnostics_at`'s callers never know
/// in advance which row a caret will land on.
pub fn to_diagnostics(diagnostics: Vec<lsp_types::Diagnostic>) -> Vec<Diagnostic> {
    diagnostics.into_iter().map(to_diagnostic).collect()
}

fn to_diagnostic(diagnostic: lsp_types::Diagnostic) -> Diagnostic {
    let raw = serde_json::to_value(&diagnostic).ok();
    Diagnostic {
        range: Range {
            start: Position {
                line: diagnostic.range.start.line,
                character: diagnostic.range.start.character,
            },
            end: Some(Position {
                line: diagnostic.range.end.line,
                character: diagnostic.range.end.character,
            }),
        },
        severity: severity_from_lsp(diagnostic.severity),
        source: diagnostic.source.clone().unwrap_or_default(),
        message: diagnostic.message.clone(),
        raw,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::{Position as LspPosition, Range as LspRange};

    fn diagnostic(
        line: u32,
        column: u32,
        severity: DiagnosticSeverity,
        message: &str,
    ) -> lsp_types::Diagnostic {
        lsp_types::Diagnostic {
            range: LspRange {
                start: LspPosition::new(line, column),
                end: LspPosition::new(line, column + 3),
            },
            severity: Some(severity),
            source: Some("rustc".into()),
            message: message.into(),
            ..Default::default()
        }
    }

    #[test]
    fn severities_map_from_the_lsp_enum_and_a_missing_one_is_an_error() {
        let mut unspecified = diagnostic(0, 0, DiagnosticSeverity::HINT, "no severity");
        unspecified.severity = None;
        let rows = to_diagnostics(vec![
            diagnostic(0, 0, DiagnosticSeverity::ERROR, "e"),
            diagnostic(0, 0, DiagnosticSeverity::WARNING, "w"),
            diagnostic(0, 0, DiagnosticSeverity::INFORMATION, "i"),
            diagnostic(0, 0, DiagnosticSeverity::HINT, "h"),
            unspecified,
        ]);
        assert_eq!(
            rows.iter().map(|d| d.severity).collect::<Vec<_>>(),
            [
                Severity::Error,
                Severity::Warning,
                Severity::Information,
                Severity::Hint,
                Severity::Error,
            ]
        );
    }

    #[test]
    fn the_range_and_message_and_source_carry_over_unconverted() {
        let rows = to_diagnostics(vec![diagnostic(2, 1, DiagnosticSeverity::ERROR, "boom")]);
        assert_eq!(rows[0].range.start, Position { line: 2, character: 1 });
        assert_eq!(rows[0].range.end, Some(Position { line: 2, character: 4 }));
        assert_eq!(rows[0].message, "boom");
        assert_eq!(rows[0].source, "rustc");
    }

    #[test]
    fn the_raw_lsp_diagnostic_round_trips_verbatim() {
        let rows = to_diagnostics(vec![diagnostic(2, 1, DiagnosticSeverity::ERROR, "boom")]);
        let raw = rows[0].raw.as_ref().expect("raw payload kept");
        assert_eq!(raw["message"], "boom");
        assert_eq!(raw["source"], "rustc");
    }

    #[test]
    fn a_diagnostic_with_no_source_gets_an_empty_display_source() {
        let mut without_source = diagnostic(0, 0, DiagnosticSeverity::ERROR, "e");
        without_source.source = None;
        let rows = to_diagnostics(vec![without_source]);
        assert_eq!(rows[0].source, "");
    }
}
