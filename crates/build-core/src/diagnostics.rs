//! What a build says about the code (B1-4).
//!
//! One shape for every toolchain, deliberately close to the shape the
//! Problems dock already renders for `lsp-core`'s diagnostics: a path, a
//! 1-based line/column, a severity and a message. A build diagnostic is not
//! a different kind of thing from a compiler diagnostic delivered over LSP,
//! and giving it its own panel would make the user look in two places for
//! the same answer (ADR-0040) — which is also why `severity` is
//! `diagnostics-core`'s shared enum rather than a build-specific one
//! (ADR-0046): `ui-shell` converts a `BuildDiagnostic` into a
//! `diagnostics_core::Diagnostic` and publishes it into the same store an
//! `lsp-core` diagnostic lands in.

use std::path::PathBuf;

/// The one Problems model's severity (ADR-0046) — shared with `lsp-core`
/// rather than a build-specific three-value enum, so the seam that used to
/// translate "Note" onto "Information" no longer exists.
pub use diagnostics_core::Severity;

/// The word a tool used, mapped onto the four kinds `diagnostics-core`
/// keeps. Unknown words (and anything a toolchain calls "note", "help" or
/// "info" — the distinctions below warning are per-tool and nothing
/// downstream renders them differently) become [`Severity::Information`]
/// rather than [`Severity::Error`]: over-reporting an error would put a red
/// row in the Problems dock for something the build was happy with.
pub fn severity_from_word(word: &str) -> Severity {
    match word.trim().to_ascii_lowercase().as_str() {
        "error" | "fatal error" | "fatal" => Severity::Error,
        "warning" | "warn" => Severity::Warning,
        _ => Severity::Information,
    }
}

/// One problem a build reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildDiagnostic {
    /// Absolute where the tool gave an absolute path, and joined onto the
    /// project root where it gave a relative one — resolved here, once, so
    /// no consumer has to know which tools report which.
    pub path: PathBuf,
    /// 1-based, like every tool's own output and like LSP's presentation
    /// layer. Zero means "the tool named a file but no line".
    pub line: u32,
    /// 1-based, or zero for a tool that reported none.
    pub column: u32,
    pub severity: Severity,
    pub message: String,
    /// The tool's own identifier for the problem (`E0308`, `unused_imports`,
    /// `-Wunused-variable`), empty when it gave none.
    pub code: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn severity_words_map_onto_the_shared_kinds() {
        assert_eq!(severity_from_word("error"), Severity::Error);
        assert_eq!(severity_from_word("Fatal Error"), Severity::Error);
        assert_eq!(severity_from_word("warning"), Severity::Warning);
        assert_eq!(severity_from_word("note"), Severity::Information);
    }

    #[test]
    fn an_unknown_word_is_information_rather_than_an_error() {
        assert_eq!(severity_from_word("blorp"), Severity::Information);
    }
}
