//! Reading a build's output as it arrives (B1-3).
//!
//! A build's output is a stream, not a document: the Problems dock fills up
//! while the build is still running, so this is fed whatever bytes have
//! arrived and holds back the partial trailing line until its newline shows
//! up. That buffering is the whole reason this is a struct rather than a
//! function — a diagnostic split across two reads must not be missed, and
//! must not be reported twice.

use std::path::{Path, PathBuf};

use container_core::target::PathMap;
use run_core::toolchain::ToolchainId;

use crate::diagnostics::BuildDiagnostic;
use crate::{cargo_json, text};

/// How one toolchain's output is read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strategy {
    /// Cargo's `--message-format=json`: exact, no pattern matching.
    CargoJson,
    /// Everything else: the pattern table in [`crate::text`].
    Text,
}

impl Strategy {
    fn for_toolchain(toolchain: ToolchainId) -> Strategy {
        match toolchain {
            ToolchainId::Cargo => Strategy::CargoJson,
            _ => Strategy::Text,
        }
    }
}

/// Turns a build's output into diagnostics, one line at a time.
pub struct DiagnosticParser {
    strategy: Strategy,
    project_root: PathBuf,
    /// C8: set when the step this parser reads was wrapped to run inside a
    /// container run target, so an in-container absolute path
    /// (`/workspace/src/x.rs`) a diagnostic names resolves back to the
    /// project file it actually means — see [`Self::with_path_map`].
    path_map: Option<PathMap>,
    /// The trailing fragment of the last chunk, before its newline arrived.
    pending: String,
}

impl DiagnosticParser {
    pub fn new(toolchain: ToolchainId, project_root: impl Into<PathBuf>) -> Self {
        Self {
            strategy: Strategy::for_toolchain(toolchain),
            project_root: project_root.into(),
            path_map: None,
            pending: String::new(),
        }
    }

    /// Attach the launch's [`PathMap`] (C8) — a builder rather than a `new`
    /// parameter, the same "an extra field almost nothing sets" shape
    /// `run_core::MacroContext::with_containers` uses, so every existing
    /// caller/test keeps compiling unchanged.
    #[must_use]
    pub fn with_path_map(mut self, path_map: Option<PathMap>) -> Self {
        self.path_map = path_map;
        self
    }

    /// Feed the next chunk of output; returns whatever complete lines in it
    /// were diagnostics.
    ///
    /// A bare carriage return ends a line here just as a newline does. Build
    /// tools attached to a terminal redraw a progress bar by returning to
    /// the start of the line — `cargo` does it several times a second, and
    /// Gradle does the same — so a parser that only split on `\n` would glue
    /// the progress text onto the front of the next real line and fail to
    /// read it. That is not hypothetical: it is why the first version of
    /// this parser found no diagnostics at all in a real Cargo build.
    pub fn feed(&mut self, chunk: &str) -> Vec<BuildDiagnostic> {
        self.pending.push_str(chunk);
        let mut diagnostics = Vec::new();
        while let Some(end) = self.pending.find(['\n', '\r']) {
            let line: String = self.pending.drain(..=end).collect();
            if let Some(diagnostic) = self.parse_line(line.trim_end_matches(['\n', '\r'])) {
                diagnostics.push(diagnostic);
            }
        }
        diagnostics
    }

    /// Flush whatever is left when the process exits without a final
    /// newline — the last diagnostic of a build is exactly the one that
    /// would otherwise be dropped.
    pub fn finish(&mut self) -> Vec<BuildDiagnostic> {
        let rest = std::mem::take(&mut self.pending);
        self.parse_line(rest.trim_end_matches(['\n', '\r']))
            .into_iter()
            .collect()
    }

    fn parse_line(&self, line: &str) -> Option<BuildDiagnostic> {
        if line.is_empty() {
            return None;
        }
        match self.strategy {
            Strategy::CargoJson => {
                cargo_json::parse_line(line, Path::new(&self.project_root), self.path_map.as_ref())
            }
            Strategy::Text => {
                text::parse_line(line, Path::new(&self.project_root), self.path_map.as_ref())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CARGO_ERROR: &str = r#"{"reason":"compiler-message","message":{"message":"mismatched types","code":{"code":"E0308"},"level":"error","spans":[{"file_name":"src/main.rs","line_start":4,"column_start":9,"is_primary":true}]}}"#;

    #[test]
    fn cargo_output_is_read_as_json_and_other_toolchains_as_text() {
        let mut cargo = DiagnosticParser::new(ToolchainId::Cargo, "/p");
        assert_eq!(cargo.feed(&format!("{CARGO_ERROR}\n")).len(), 1);

        let mut cmake = DiagnosticParser::new(ToolchainId::Cmake, "/p");
        assert_eq!(cmake.feed("a.cpp:1:2: error: boom\n").len(), 1);
    }

    #[test]
    fn a_cargo_build_does_not_also_read_its_rendered_text() {
        // Cargo prints both the JSON and, with some settings, a rendered
        // copy. Reading only the JSON is what stops one problem appearing
        // twice in the Problems dock.
        let mut parser = DiagnosticParser::new(ToolchainId::Cargo, "/p");
        let diagnostics = parser.feed(&format!(
            "{CARGO_ERROR}\nerror[E0308]: mismatched types\n  --> src/main.rs:4:9\n"
        ));
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn a_line_split_across_two_chunks_is_reported_once_and_completely() {
        let mut parser = DiagnosticParser::new(ToolchainId::Cmake, "/p");
        assert!(parser.feed("src/main.cpp:12:5: err").is_empty());
        let diagnostics = parser.feed("or: expected ';'\n");
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].message, "expected ';'");
        assert_eq!(diagnostics[0].line, 12);
    }

    #[test]
    fn several_diagnostics_in_one_chunk_all_come_back_in_order() {
        let mut parser = DiagnosticParser::new(ToolchainId::Cmake, "/p");
        let diagnostics = parser.feed("a.cpp:1:1: error: one\nnoise\nb.cpp:2:1: warning: two\n");
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(diagnostics[0].message, "one");
        assert_eq!(diagnostics[1].message, "two");
    }

    #[test]
    fn the_last_line_without_a_newline_is_not_lost() {
        let mut parser = DiagnosticParser::new(ToolchainId::Cmake, "/p");
        assert!(parser.feed("a.cpp:9:1: error: at the very end").is_empty());
        let diagnostics = parser.finish();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 9);
    }

    #[test]
    fn finishing_twice_does_not_repeat_the_last_diagnostic() {
        let mut parser = DiagnosticParser::new(ToolchainId::Cmake, "/p");
        parser.feed("a.cpp:9:1: error: boom");
        assert_eq!(parser.finish().len(), 1);
        assert!(parser.finish().is_empty());
    }

    #[test]
    fn a_progress_redraw_does_not_swallow_the_line_after_it() {
        // What a real Cargo build over a PTY looks like: a progress bar
        // rewritten in place with a bare carriage return, then the JSON.
        let mut parser = DiagnosticParser::new(ToolchainId::Cargo, "/p");
        let diagnostics = parser.feed(&format!(
            "    Building [==>      ] 0/1: broken-build(bin)   \r{CARGO_ERROR}\n"
        ));
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].line, 4);
    }

    #[test]
    fn windows_line_endings_do_not_end_up_in_the_message() {
        let mut parser = DiagnosticParser::new(ToolchainId::Cmake, "/p");
        let diagnostics = parser.feed("a.cpp:1:1: error: boom\r\n");
        assert_eq!(diagnostics[0].message, "boom");
    }
}
