//! The one Problems model (ADR-0046).
//!
//! Every diagnostic source in the IDE — a language server, a build tool, and
//! later an analyzer or a test run — reports through this crate's
//! [`DiagnosticStore`] rather than through a store of its own. The store is
//! keyed by `(source, uri)`, not by `uri` alone, so a build tool's rows for a
//! file and a language server's rows for the same file coexist: replacing
//! one source's rows for a file never touches another source's.
//!
//! Qt-free by design (`docs/architecture/layering.md`'s `diagnostics-core`
//! row): this crate knows nothing about LSP, a build tool, or Qt. `lsp-core`
//! and `build-core` translate their own protocol/tool-specific shapes into
//! [`Diagnostic`] at the point they publish; `ui-shell` owns the one shared
//! [`DiagnosticStore`] instance and reads it back for the Problems dock and
//! the editor's squiggles.

use std::collections::BTreeMap;

use serde_json::Value;

/// Severity as the UI ranks it. Ordered worst-first, which is also the
/// tie-break order for two diagnostics on the same position.
///
/// Four variants — the richer LSP enum, per ADR-0046 — rather than a
/// three-value build-tool enum: a build tool's "note"/"info" maps onto
/// [`Severity::Information`], which is what let `build-core` and `lsp-core`
/// share one severity type instead of translating between two at the seam.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}

/// A 0-based position, counted the way LSP counts one — in UTF-16 code
/// units. A build tool's 1-based line/column is converted to this at the
/// point it becomes a [`Diagnostic`], not carried through unconverted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// Where a diagnostic applies. `end` is optional: a build tool reports only
/// where a problem starts, never where it ends, while an LSP diagnostic
/// always carries both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Range {
    pub start: Position,
    pub end: Option<Position>,
}

/// One problem, from whichever source reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct Diagnostic {
    pub range: Range,
    pub severity: Severity,
    pub message: String,
    /// The word shown in the Problems dock's Source column — a compiler
    /// name (`rustc`), a toolchain (`cargo`), a linter (`phpstan`). Not the
    /// same thing as the store's `source` key: several LSP diagnostics in
    /// one `publishDiagnostics` batch can each name a different compiler
    /// while all being keyed under the one language server that sent them.
    pub source: String,
    /// The diagnostic exactly as its protocol produced it, when the
    /// protocol has one to keep — verbatim JSON, because several LSP
    /// servers compute a code action from a diagnostic's own opaque `data`
    /// field and refuse to offer one for a diagnostic not handed back
    /// exactly as they sent it. `None` for a source with no such payload
    /// (a build tool's diagnostics have none).
    pub raw: Option<Value>,
}

/// One row of the Problems panel, addressed the way the editor jumps:
/// `line` is 1-based (what `openAt(path, line, column)` takes and what the
/// panel prints), `column` is 0-based, both counted in UTF-16 code units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticRow {
    pub uri: String,
    pub path: String,
    pub line: u32,
    pub column: u32,
    pub end_line: u32,
    pub end_column: u32,
    pub severity: Severity,
    pub message: String,
    pub source: String,
}

/// How many of each severity are currently known, for the status bar and the
/// panel's filter buttons — across every source at once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DiagnosticCounts {
    pub errors: usize,
    pub warnings: usize,
    pub infos: usize,
    pub hints: usize,
}

/// The diagnostics currently published, keyed by `(source, uri)`.
///
/// `source` identifies the *publisher* — a language server's id, a build
/// toolchain's name — not the per-diagnostic display label
/// ([`Diagnostic::source`]). Replacing `(source, uri)` never touches another
/// source's rows for the same file, which is the whole reason this store
/// exists rather than the old uri-only one: a build diagnostic and an LSP
/// diagnostic on the same file must coexist.
#[derive(Debug, Default)]
pub struct DiagnosticStore {
    // BTreeMap so iteration order is deterministic with no extra sort.
    by_key: BTreeMap<(String, String), Vec<Diagnostic>>,
}

impl DiagnosticStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Apply what `source` currently knows about `uri`. An empty list drops
    /// the entry rather than leaving an empty one behind — how a language
    /// server says "fixed" and how a new build run starts clean.
    pub fn replace(&mut self, source: &str, uri: &str, diagnostics: Vec<Diagnostic>) {
        let key = (source.to_string(), uri.to_string());
        if diagnostics.is_empty() {
            self.by_key.remove(&key);
        } else {
            self.by_key.insert(key, diagnostics);
        }
    }

    /// Forget one source's rows for one file — what closing a tab means for
    /// the language server that had it open. Every other source's rows for
    /// the same file, and this source's rows for every other file, are
    /// untouched.
    pub fn remove(&mut self, source: &str, uri: &str) {
        self.by_key.remove(&(source.to_string(), uri.to_string()));
    }

    /// Forget everything a source ever published — what a new build run
    /// clears before it reports anything, and a stopped language server
    /// clears when it exits.
    pub fn clear_source(&mut self, source: &str) {
        self.by_key
            .retain(|(key_source, _), _| key_source != source);
    }

    /// Forget everything from every source — a new project opening.
    pub fn clear(&mut self) {
        self.by_key.clear();
    }

    /// Every row, ordered by file, then line, then column, then severity.
    pub fn rows(&self) -> Vec<DiagnosticRow> {
        let mut rows: Vec<DiagnosticRow> = self
            .by_key
            .iter()
            .flat_map(|((_, uri), diags)| diags.iter().map(move |d| row(uri, d)))
            .collect();
        rows.sort_by(|a, b| {
            (&a.path, a.line, a.column, a.severity).cmp(&(&b.path, b.line, b.column, b.severity))
        });
        rows
    }

    /// The rows for one file across every source, same order as
    /// [`Self::rows`] — what the editor underlines.
    pub fn rows_for_uri(&self, uri: &str) -> Vec<DiagnosticRow> {
        let mut rows: Vec<DiagnosticRow> = self
            .by_key
            .iter()
            .filter(|((_, key_uri), _)| key_uri == uri)
            .flat_map(|(_, diags)| diags.iter().map(|d| row(uri, d)))
            .collect();
        rows.sort_by_key(|row| (row.line, row.column, row.severity));
        rows
    }

    pub fn counts(&self) -> DiagnosticCounts {
        let mut counts = DiagnosticCounts::default();
        for diagnostic in self.by_key.values().flatten() {
            match diagnostic.severity {
                Severity::Error => counts.errors += 1,
                Severity::Warning => counts.warnings += 1,
                Severity::Information => counts.infos += 1,
                Severity::Hint => counts.hints += 1,
            }
        }
        counts
    }

    /// The raw payload of every diagnostic, from any source, whose range
    /// covers `(line, character)` — what `LspManager::intentions` needs
    /// verbatim. A source with no raw payload (a build tool's rows) never
    /// contributes here, which is correct: there is no protocol to hand a
    /// build diagnostic back to.
    pub fn diagnostics_at(&self, uri: &str, line: u32, character: u32) -> Vec<Value> {
        self.by_key
            .iter()
            .filter(|((_, key_uri), _)| key_uri == uri)
            .flat_map(|(_, diags)| diags.iter())
            .filter(|d| covers(&d.range, line, character))
            .filter_map(|d| d.raw.clone())
            .collect()
    }
}

/// Whether a range covers a position, inclusive of both ends — a diagnostic
/// reported for the empty range at the very end of a line must still be
/// found by a caret sitting there. A missing `end` covers only `start`
/// itself, exactly like a zero-width range would.
fn covers(range: &Range, line: u32, character: u32) -> bool {
    let end = range.end.unwrap_or(range.start);
    let after_start =
        line > range.start.line || (line == range.start.line && character >= range.start.character);
    let before_end = line < end.line || (line == end.line && character <= end.character);
    after_start && before_end
}

fn row(uri: &str, diagnostic: &Diagnostic) -> DiagnosticRow {
    let end = diagnostic.range.end.unwrap_or(diagnostic.range.start);
    DiagnosticRow {
        uri: uri.to_string(),
        path: path_from_uri(uri).unwrap_or_else(|| uri.to_string()),
        line: diagnostic.range.start.line + 1,
        column: diagnostic.range.start.character,
        end_line: end.line + 1,
        end_column: end.character,
        severity: diagnostic.severity,
        message: diagnostic.message.clone(),
        source: diagnostic.source.clone(),
    }
}

/// `file:///a/b%20c.rs` -> `/a/b c.rs`. Hand-rolled rather than pulled in as
/// a dependency: the only producer and consumer of these URIs is this
/// codebase's own client, and both halves are twenty lines.
pub fn path_from_uri(uri: &str) -> Option<String> {
    let rest = uri.strip_prefix("file://")?;
    // `file:///path` (empty authority) is the only form we emit; anything
    // else is a remote URI we have no local path for.
    if !rest.starts_with('/') {
        return None;
    }
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?;
            if let Ok(byte) = u8::from_str_radix(hex, 16) {
                out.push(byte);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).ok()
}

/// `/a/b c.rs` -> `file:///a/b%20c.rs`. Percent-encodes everything outside
/// the unreserved set (plus `/`), which is stricter than required and
/// therefore always safe.
pub fn uri_from_path(path: &str) -> String {
    let mut uri = String::from("file://");
    if !path.starts_with('/') {
        uri.push('/');
    }
    for byte in path.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                uri.push(byte as char)
            }
            // Windows drive letters: `C:\x` arrives as `C:/x` from Qt.
            b':' => uri.push(':'),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;

    fn point_diagnostic(line: u32, column: u32, severity: Severity, message: &str) -> Diagnostic {
        Diagnostic {
            range: Range {
                start: Position {
                    line,
                    character: column,
                },
                end: Some(Position {
                    line,
                    character: column + 3,
                }),
            },
            severity,
            message: message.into(),
            source: "test".into(),
            raw: None,
        }
    }

    #[test]
    fn rows_are_grouped_by_file_and_ordered_within_it() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp",
            "file:///p/b.rs",
            vec![point_diagnostic(3, 0, Severity::Error, "b")],
        );
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![
                point_diagnostic(9, 1, Severity::Warning, "late"),
                point_diagnostic(2, 5, Severity::Error, "early"),
                point_diagnostic(2, 1, Severity::Hint, "earliest"),
            ],
        );

        let rows = store.rows();
        let order: Vec<(&str, u32, u32)> = rows
            .iter()
            .map(|r| (r.message.as_str(), r.line, r.column))
            .collect();
        assert_eq!(
            order,
            [
                ("earliest", 3, 1),
                ("early", 3, 5),
                ("late", 10, 1),
                ("b", 4, 0)
            ]
        );
        assert!(rows[0].path.ends_with("/p/a.rs"));
    }

    #[test]
    fn a_same_position_tie_puts_the_worse_severity_first() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![
                point_diagnostic(0, 0, Severity::Warning, "warn"),
                point_diagnostic(0, 0, Severity::Error, "err"),
            ],
        );
        let rows = store.rows();
        assert_eq!(rows[0].message, "err");
    }

    #[test]
    fn republishing_the_same_source_replaces_and_an_empty_list_clears_it() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Error, "one")],
        );
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![
                point_diagnostic(0, 0, Severity::Error, "two"),
                point_diagnostic(1, 0, Severity::Warning, "three"),
            ],
        );
        assert_eq!(store.rows().len(), 2);

        store.replace("lsp", "file:///p/a.rs", Vec::new());
        assert!(store.rows().is_empty());
    }

    #[test]
    fn a_build_source_and_an_lsp_source_coexist_on_the_same_file() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Warning, "unused import")],
        );
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![point_diagnostic(4, 2, Severity::Error, "mismatched types")],
        );

        let rows = store.rows_for_uri("file:///p/a.rs");
        assert_eq!(rows.len(), 2);
        let messages: Vec<&str> = rows.iter().map(|r| r.message.as_str()).collect();
        assert!(messages.contains(&"unused import"));
        assert!(messages.contains(&"mismatched types"));
    }

    #[test]
    fn replacing_one_source_never_touches_another_sources_rows() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Warning, "unused import")],
        );
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![point_diagnostic(4, 2, Severity::Error, "mismatched types")],
        );

        // A second build run's rows replace only the build source's.
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![point_diagnostic(5, 0, Severity::Error, "fixed differently")],
        );

        let rows = store.rows_for_uri("file:///p/a.rs");
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().any(|r| r.message == "unused import"));
        assert!(rows.iter().any(|r| r.message == "fixed differently"));
        assert!(!rows.iter().any(|r| r.message == "mismatched types"));
    }

    #[test]
    fn clearing_one_source_leaves_the_others_alone() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Warning, "unused import")],
        );
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![point_diagnostic(4, 2, Severity::Error, "mismatched types")],
        );
        store.replace(
            "build:cargo",
            "file:///p/b.rs",
            vec![point_diagnostic(1, 0, Severity::Error, "elsewhere")],
        );

        store.clear_source("build:cargo");

        let rows = store.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "unused import");
    }

    #[test]
    fn removing_one_uri_from_one_source_leaves_that_uri_other_sources_alone() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Warning, "unused import")],
        );
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![point_diagnostic(4, 2, Severity::Error, "mismatched types")],
        );

        store.remove("lsp:rust", "file:///p/a.rs");

        let rows = store.rows_for_uri("file:///p/a.rs");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "mismatched types");
    }

    #[test]
    fn counts_are_split_by_severity_across_every_source() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///p/a.rs",
            vec![
                point_diagnostic(0, 0, Severity::Error, "e"),
                point_diagnostic(1, 0, Severity::Warning, "w"),
            ],
        );
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![
                point_diagnostic(2, 0, Severity::Information, "i"),
                point_diagnostic(3, 0, Severity::Hint, "h"),
            ],
        );
        assert_eq!(
            store.counts(),
            DiagnosticCounts {
                errors: 1,
                warnings: 1,
                infos: 1,
                hints: 1
            }
        );
    }

    #[test]
    fn rows_for_one_uri_ignore_the_others() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Error, "a")],
        );
        store.replace(
            "lsp",
            "file:///p/b.rs",
            vec![point_diagnostic(0, 0, Severity::Error, "b")],
        );
        let rows = store.rows_for_uri("file:///p/b.rs");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].message, "b");
        assert!(store.rows_for_uri("file:///p/missing.rs").is_empty());
    }

    #[test]
    fn clear_forgets_every_source() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Error, "a")],
        );
        store.replace(
            "build",
            "file:///p/a.rs",
            vec![point_diagnostic(0, 0, Severity::Error, "b")],
        );
        store.clear();
        assert!(store.rows().is_empty());
    }

    #[test]
    fn a_diagnostic_with_no_end_range_points_at_its_start() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![Diagnostic {
                range: Range {
                    start: Position {
                        line: 3,
                        character: 8,
                    },
                    end: None,
                },
                severity: Severity::Error,
                message: "no end range".into(),
                source: "cargo".into(),
                raw: None,
            }],
        );
        let rows = store.rows();
        assert_eq!(rows[0].line, 4);
        assert_eq!(rows[0].column, 8);
        assert_eq!(rows[0].end_line, 4);
        assert_eq!(rows[0].end_column, 8);
    }

    #[test]
    fn paths_round_trip_through_the_uri_form() {
        for path in ["/home/u/main.rs", "/home/u/a b/#c.rs", "/tmp/ünïcode.rs"] {
            assert_eq!(path_from_uri(&uri_from_path(path)).as_deref(), Some(path));
        }
        assert!(uri_from_path("/home/u/a b.rs").contains("%20"));
    }

    #[test]
    fn a_non_file_uri_has_no_local_path() {
        assert!(path_from_uri("untitled:Untitled-1").is_none());
    }

    #[test]
    fn diagnostics_at_finds_the_one_covering_the_position_and_carries_its_raw_payload() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp:rust",
            "file:///p/a.rs",
            vec![
                Diagnostic {
                    range: Range {
                        start: Position {
                            line: 2,
                            character: 1,
                        },
                        end: Some(Position {
                            line: 2,
                            character: 4,
                        }),
                    },
                    severity: Severity::Error,
                    message: "here".into(),
                    source: "rustc".into(),
                    raw: Some(serde_json::json!({"message": "here"})),
                },
                point_diagnostic(9, 0, Severity::Warning, "elsewhere"),
            ],
        );
        let found = store.diagnostics_at("file:///p/a.rs", 2, 2);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0]["message"], "here");
    }

    #[test]
    fn diagnostics_at_skips_rows_with_no_raw_payload() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "build:cargo",
            "file:///p/a.rs",
            vec![point_diagnostic(2, 1, Severity::Error, "no raw payload")],
        );
        assert!(store.diagnostics_at("file:///p/a.rs", 2, 1).is_empty());
    }

    #[test]
    fn diagnostics_at_is_empty_off_every_range() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![Diagnostic {
                raw: Some(Value::Null),
                ..point_diagnostic(2, 1, Severity::Error, "here")
            }],
        );
        assert!(store.diagnostics_at("file:///p/a.rs", 2, 0).is_empty());
        assert!(store.diagnostics_at("file:///p/a.rs", 5, 1).is_empty());
        assert!(store
            .diagnostics_at("file:///p/missing.rs", 2, 1)
            .is_empty());
    }

    #[test]
    fn diagnostics_at_includes_both_endpoints_of_the_range() {
        let mut store = DiagnosticStore::new();
        store.replace(
            "lsp",
            "file:///p/a.rs",
            vec![Diagnostic {
                raw: Some(Value::Null),
                ..point_diagnostic(2, 1, Severity::Error, "here")
            }],
        );
        // `point_diagnostic()` builds a 3-character range: [2:1, 2:4).
        assert_eq!(store.diagnostics_at("file:///p/a.rs", 2, 1).len(), 1);
        assert_eq!(store.diagnostics_at("file:///p/a.rs", 2, 4).len(), 1);
    }
}
