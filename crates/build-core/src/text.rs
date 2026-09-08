//! Diagnostics recovered from a build tool's plain text (B1-3).
//!
//! Every toolchain except Cargo prints its problems for a human, so they
//! have to be read back out with a pattern. The table below is
//! deliberately small and table-driven: a tool that changes its format is a
//! fixture and a row, not a rewrite.
//!
//! The patterns are the same catalogue `run_core::links` already uses to
//! make `file:line:col` clickable in a run console — the shapes are
//! genuinely the same — but this module keeps its own table because it needs
//! the *severity and message* as well as the location, which a link
//! resolver has no use for. Neither is derived from the other, and
//! `docs/architecture/layering.md` records that as a deliberate second
//! reading of the same text rather than an oversight.

use std::path::Path;

use regex::Regex;

use crate::diagnostics::{severity_from_word, BuildDiagnostic};

/// One tool's way of writing a diagnostic line.
struct Pattern {
    regex: Regex,
    /// Capture names are used rather than indices so a pattern can omit a
    /// group (Maven reports no column in some layouts) without renumbering.
    has_column: bool,
}

/// The catalogue, most specific first.
///
/// - gcc, clang, CMake and javac's newer output: `path:line:col: error: msg`
/// - javac's classic output: `path:line: error: msg`
/// - Maven and Gradle: `[ERROR] /path/File.java:[12,5] msg`
fn patterns() -> &'static [Pattern] {
    use std::sync::OnceLock;
    static PATTERNS: OnceLock<Vec<Pattern>> = OnceLock::new();
    PATTERNS.get_or_init(|| {
        vec![
            Pattern {
                regex: Regex::new(
                    r"^\s*(?P<path>[^:\s][^:]*):(?P<line>\d+):(?P<col>\d+):\s*(?P<severity>fatal error|error|warning|note|help|info)\s*:\s*(?P<message>.+)$",
                )
                .expect("static pattern"),
                has_column: true,
            },
            Pattern {
                regex: Regex::new(
                    r"^\s*(?P<path>[^:\s][^:]*):(?P<line>\d+):\s*(?P<severity>fatal error|error|warning|note|help|info)\s*:\s*(?P<message>.+)$",
                )
                .expect("static pattern"),
                has_column: false,
            },
            Pattern {
                regex: Regex::new(
                    r"^\s*\[(?P<severity>ERROR|WARNING|INFO)\]\s+(?P<path>[^:\[\s][^:\[]*):\[(?P<line>\d+),(?P<col>\d+)\]\s*(?P<message>.+)$",
                )
                .expect("static pattern"),
                has_column: true,
            },
        ]
    })
}

/// Parse one line of a build tool's output, or `None` when it is not a
/// diagnostic — which is the overwhelming majority of lines, so this is
/// written to fail fast rather than to be clever.
pub fn parse_line(line: &str, project_root: &Path) -> Option<BuildDiagnostic> {
    // Every pattern needs a colon and a digit; skipping the ones that have
    // neither keeps a normal build's thousands of progress lines off the
    // regex engine entirely.
    if !line.contains(':') || !line.bytes().any(|b| b.is_ascii_digit()) {
        return None;
    }

    for pattern in patterns() {
        let Some(captures) = pattern.regex.captures(line) else {
            continue;
        };
        let raw_path = captures.name("path")?.as_str().trim();
        // A bare word with no separator is far more likely to be prose
        // ("note: run with ...") than a path, and a diagnostic pointing at a
        // file that cannot exist is worse than one not reported.
        if raw_path.is_empty() {
            continue;
        }
        let path = Path::new(raw_path);
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            project_root.join(path)
        };

        return Some(BuildDiagnostic {
            path,
            line: captures.name("line")?.as_str().parse().ok()?,
            column: if pattern.has_column {
                captures.name("col")?.as_str().parse().unwrap_or(0)
            } else {
                0
            },
            severity: severity_from_word(captures.name("severity")?.as_str()),
            message: captures.name("message")?.as_str().trim().to_string(),
            code: String::new(),
        });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnostics::Severity;
    use std::path::PathBuf;

    fn parse(line: &str) -> Option<BuildDiagnostic> {
        parse_line(line, Path::new("/p"))
    }

    #[test]
    fn table_driven_tool_output() {
        let cases: &[(&str, &str, u32, u32, Severity)] = &[
            (
                "src/main.cpp:12:5: error: expected ';' before '}' token",
                "/p/src/main.cpp",
                12,
                5,
                Severity::Error,
            ),
            (
                "/abs/util.c:3:1: warning: unused variable 'x'",
                "/abs/util.c",
                3,
                1,
                Severity::Warning,
            ),
            (
                "src/App.java:42: error: cannot find symbol",
                "/p/src/App.java",
                42,
                0,
                Severity::Error,
            ),
            (
                "[ERROR] /repo/src/App.java:[12,5] cannot find symbol",
                "/repo/src/App.java",
                12,
                5,
                Severity::Error,
            ),
            (
                "[WARNING] /repo/src/App.java:[7,1] deprecated API",
                "/repo/src/App.java",
                7,
                1,
                Severity::Warning,
            ),
            (
                "CMakeLists.txt:8:3: error: unknown command",
                "/p/CMakeLists.txt",
                8,
                3,
                Severity::Error,
            ),
        ];
        for (line, path, expected_line, column, severity) in cases {
            let diagnostic = parse(line).unwrap_or_else(|| panic!("no match for {line}"));
            assert_eq!(diagnostic.path, PathBuf::from(path), "{line}");
            assert_eq!(diagnostic.line, *expected_line, "{line}");
            assert_eq!(diagnostic.column, *column, "{line}");
            assert_eq!(diagnostic.severity, *severity, "{line}");
            assert!(!diagnostic.message.is_empty(), "{line}");
        }
    }

    #[test]
    fn ordinary_build_chatter_is_not_a_diagnostic() {
        for line in [
            "   Compiling build-core v0.1.0",
            "[INFO] BUILD SUCCESS",
            "> Task :app:compileJava",
            "make[1]: Entering directory '/p'",
            "Total time: 12:03 min",
            "",
        ] {
            assert!(parse(line).is_none(), "{line:?} was read as a diagnostic");
        }
    }

    #[test]
    fn the_message_keeps_its_own_colons() {
        let diagnostic = parse("a.cpp:1:1: error: expected ':' before 'x'").unwrap();
        assert_eq!(diagnostic.message, "expected ':' before 'x'");
    }

    #[test]
    fn a_text_diagnostic_carries_no_code() {
        // Only Cargo's JSON gives one; inventing one from the message would
        // put a made-up identifier in the Problems dock.
        assert_eq!(parse("a.cpp:1:1: error: boom").unwrap().code, "");
    }
}
