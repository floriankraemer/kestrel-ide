//! `file:line[:col]` link detection in run/terminal output (F4-8).
//!
//! Extends the same idea as `terminal_core::TerminalEmulator::link_at`
//! (`http(s)` URLs on a live grid row): scan text for a recognizable span
//! and hand back where it points. That function stays as-is — it hit-tests
//! grid cells for URLs, a different shape of input entirely — but this
//! module is deliberately the *one* place a `path:line:col` pattern is
//! recognized, per the plan's instruction to build a per-language catalogue
//! rather than a second table. A future `TerminalSession::resolveLink` and
//! `RunService::resolveLink` both call [`resolve_link`], so console output
//! and terminal output share one rule.
//!
//! # Design
//!
//! One table of regexes, each producing a candidate `(path, line, col)`
//! span. Negative cases matter more than positives (the plan's own words):
//! a URL's `host:port`, a bare `HH:MM:SS` timestamp, and two colons with no
//! digits either side must never be mistaken for a location. The generic
//! pattern below requires the "path" half to end in a `.`-extension (or be
//! literally `Makefile`/`makefile`, which has none) and the "line" half to
//! be all digits — which is precisely what rules those cases out:
//!
//! - `http://example.com:8080` — `example.com` does have a dot, but the
//!   recovered path starts with the scheme's leftover `//`, which no real
//!   file path does, so `find_candidates` rejects it.
//! - `12:30:00` — `12` has no dot-extension, so it is not a path candidate.
//! - `foo:bar:baz` — `bar` is not all digits, so it is not a line number.
//! - `-c:5` — `-c` has no dot-extension.
//! - a colon inside an ANSI escape (e.g. `\x1b[38;5;196m`) — the segments
//!   around it are bare numbers with no dot-extension.
//!
//! Python's `File "path", line N` has no colon-delimited line number at
//! all, so it gets its own regex rather than being forced into the generic
//! one.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// `path:line` or `path:line:col`, where `path` is either `Makefile`
/// (`makefile`) or ends in a `.`-extension, and `line`/`col` are plain
/// digits. Matches rustc's bare form (`src/main.rs:42:5`), its `--> `
/// form (the prefix is not part of the pattern — it's just text before the
/// match), rustc's panic form (`panicked at 'msg', src/main.rs:12:5`),
/// gcc/clang (`main.cpp:42:1: error:`), Node (`at fn (/abs/app/main.js:12:3)`),
/// and `Makefile:17:`.
static GENERIC_LOCATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:[A-Za-z]:)?[\w./\\-]*(?:[\w-]+\.[A-Za-z0-9]+|[Mm]akefile):(\d+)(?::(\d+))?")
        .unwrap()
});

/// Python's traceback frame header: `File "app/main.py", line 12`.
static PYTHON_FRAME: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"File "([^"]+)", line (\d+)"#).unwrap());

/// A `path:line[:col]` span found in text, before resolution against a
/// working directory.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Candidate {
    start: usize,
    end: usize,
    path: String,
    line: u32,
    col: Option<u32>,
}

/// A location a console/terminal link resolved to: a file that exists on
/// disk, plus the line (and, where known, column) it points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLink {
    pub path: PathBuf,
    pub line: u32,
    pub col: Option<u32>,
    /// The half-open byte range of `text` the link was recognised in.
    ///
    /// A console indexes its own document by offset and needs none of
    /// this; a terminal underlines the cells it hovers, and cannot ask
    /// where the link starts without being told (R2-6).
    pub start: usize,
    pub end: usize,
}

fn find_candidates(text: &str) -> Vec<Candidate> {
    let mut candidates = Vec::new();

    for m in GENERIC_LOCATION.captures_iter(text) {
        let whole = m.get(0).unwrap();
        let Ok(line) = m[1].parse::<u32>() else {
            continue;
        };
        let col = m.get(2).and_then(|c| c.as_str().parse::<u32>().ok());
        // Strip the trailing `:line[:col]` back off to recover just the path.
        let path_end = path_end_before_line_col(whole.as_str(), col.is_some());
        let path = &whole.as_str()[..path_end];
        // A URL's `host:port` (e.g. `http://example.com:8080`) matches the
        // same shape as a real path:line — `example.com` even has a dot —
        // but its path capture starts with the `//` left over from the
        // scheme, which no real file path does. Reject those rather than
        // resolving `//example.com` as if it were a path.
        if path.starts_with("//") {
            continue;
        }
        candidates.push(Candidate {
            start: whole.start(),
            end: whole.end(),
            path: path.to_string(),
            line,
            col,
        });
    }

    for m in PYTHON_FRAME.captures_iter(text) {
        let whole = m.get(0).unwrap();
        let Ok(line) = m[2].parse::<u32>() else {
            continue;
        };
        candidates.push(Candidate {
            start: whole.start(),
            end: whole.end(),
            path: m[1].to_string(),
            line,
            col: None,
        });
    }

    candidates
}

/// Given the full `path:line[:col]` match text, find where the path
/// component ends (i.e. before the last one or two `:digits` groups).
fn path_end_before_line_col(matched: &str, has_col: bool) -> usize {
    let mut end = matched.len();
    let strip_one = |s: &str| -> usize { s.rfind(':').unwrap_or(s.len()) };
    end = strip_one(&matched[..end]);
    if has_col {
        end = strip_one(&matched[..end]);
    }
    end
}

/// Resolve `path` against `cwd` the way a shell would: absolute paths are
/// used as-is, relative ones join onto `cwd`. Backslashes are normalized to
/// forward slashes first, so a Windows-style path pasted into Linux output
/// (or vice versa) still joins onto a real `Path` instead of becoming one
/// unsplittable file-name component.
///
/// W5-2: when `cwd` is a WSL UNC path, `path` — absolute or relative — is a
/// Linux path regardless: the program that printed it ran inside the
/// distro. `ExecHost::to_remote`/`to_local` resolves it explicitly, the
/// same rule `build_core::diagnostics::resolve_path` applies to a
/// compiler's own file references, rather than trusting `PathBuf::join`'s
/// platform-dependent handling of a rooted-but-unprefixed argument.
fn resolve_path(path: &str, cwd: &Path) -> PathBuf {
    let host = process_exec::host::ExecHost::for_path(cwd);
    if host.is_remote() {
        let linux_path = if path.starts_with('/') {
            path.to_string()
        } else {
            format!("{}/{path}", host.to_remote(cwd))
        };
        return host.to_local(&linux_path);
    }
    let normalized = path.replace('\\', "/");
    let candidate = Path::new(&normalized);
    if candidate.is_absolute() {
        candidate.to_path_buf()
    } else {
        cwd.join(candidate)
    }
}

/// Find the `file:line[:col]` (or Python `File "...", line N`) candidate
/// touching `byte_offset` in `text`, resolve its path against `cwd` (the
/// run's own working directory — **not** the project root, since a
/// relative path in build output is relative to wherever the build ran),
/// and return `None` if the resolved file does not exist. A dead link is
/// worse than plain text, so a non-existent path is reported the same as no
/// link at all rather than as a link that goes nowhere.
pub fn resolve_link(text: &str, byte_offset: usize, cwd: &Path) -> Option<ResolvedLink> {
    let candidate = find_candidates(text)
        .into_iter()
        .find(|c| byte_offset >= c.start && byte_offset < c.end)?;

    let resolved_path = resolve_path(&candidate.path, cwd);
    if !resolved_path.is_file() {
        return None;
    }

    Some(ResolvedLink {
        path: resolved_path,
        line: candidate.line,
        col: candidate.col,
        start: candidate.start,
        end: candidate.end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// `resolve_link` requires the file to exist, so every positive test
    /// case needs a real file under a temp `cwd`. `path` is the location as
    /// it would appear in output; `create` is the file actually created
    /// (relative to the temp dir) so a resolvable case has something to
    /// resolve to.
    struct Case {
        text: &'static str,
        create: Option<&'static str>,
        expected: Option<(&'static str, u32, Option<u32>)>,
    }

    #[test]
    fn a_resolved_link_reports_the_span_it_matched() {
        // The terminal underlines the cells it is offering to open, and
        // cannot work out which ones without being told (R2-6).
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("main.rs"), "fn main() {}").expect("write");
        let text = "error at main.rs:42:5 there";
        let offset = text.find("main.rs").expect("the path is in the text");

        let link = resolve_link(text, offset, dir.path()).expect("a link");
        assert_eq!(&text[link.start..link.end], "main.rs:42:5");
    }

    #[test]
    fn table_driven_location_parsing() {
        let cases = [
            Case {
                text: "src/main.rs:42:5",
                create: Some("src/main.rs"),
                expected: Some(("src/main.rs", 42, Some(5))),
            },
            Case {
                text: "   --> src/main.rs:42:5",
                create: Some("src/main.rs"),
                expected: Some(("src/main.rs", 42, Some(5))),
            },
            Case {
                text: "thread 'main' panicked at 'boom', src/main.rs:12:5",
                create: Some("src/main.rs"),
                expected: Some(("src/main.rs", 12, Some(5))),
            },
            Case {
                text: r#"File "app/main.py", line 12"#,
                create: Some("app/main.py"),
                expected: Some(("app/main.py", 12, None)),
            },
            Case {
                text: "main.cpp:42:1: error: expected ';'",
                create: Some("main.cpp"),
                expected: Some(("main.cpp", 42, Some(1))),
            },
            Case {
                text: "Makefile:17: recipe for target 'build' failed",
                create: Some("Makefile"),
                expected: Some(("Makefile", 17, None)),
            },
            // Negative cases: must not match at all, or must not resolve
            // because the file does not exist.
            Case {
                text: "see http://example.com:8080 for details",
                create: None,
                expected: None,
            },
            Case {
                text: "started at 12:30:00 sharp",
                create: None,
                expected: None,
            },
            Case {
                text: "foo:bar:baz",
                create: None,
                expected: None,
            },
            Case {
                text: "-c:5",
                create: None,
                expected: None,
            },
            Case {
                text: "\x1b[38;5;196mred\x1b[0m",
                create: None,
                expected: None,
            },
            Case {
                // A real-looking location, but the file is never created —
                // a dead link must resolve to None, not a bogus Some.
                text: "src/does_not_exist.rs:1:1",
                create: None,
                expected: None,
            },
        ];

        for case in cases {
            let dir = tempfile::tempdir().expect("tempdir");
            if let Some(rel) = case.create {
                let full = dir.path().join(rel);
                if let Some(parent) = full.parent() {
                    fs::create_dir_all(parent).unwrap();
                }
                fs::write(&full, "").unwrap();
            }

            // For a case expected to resolve, probe a byte offset inside the
            // path itself — the match isn't always centered in the whole
            // string (e.g. a panic message has plenty of text before the
            // location). For a negative case there is no match anywhere in
            // the text, so the exact offset does not matter.
            let offset = match case.expected {
                Some((rel, ..)) => {
                    let start = case
                        .text
                        .find(rel)
                        .expect("fixture text must contain its own path");
                    start + rel.len() / 2
                }
                None => case.text.len() / 2,
            };
            let resolved = resolve_link(case.text, offset, dir.path());

            match case.expected {
                None => assert!(
                    resolved.is_none(),
                    "expected no link for {:?}, got {resolved:?}",
                    case.text
                ),
                Some((rel, line, col)) => {
                    let resolved =
                        resolved.unwrap_or_else(|| panic!("expected a link for {:?}", case.text));
                    assert_eq!(resolved.path, dir.path().join(rel));
                    assert_eq!(resolved.line, line);
                    assert_eq!(resolved.col, col);
                }
            }
        }
    }

    #[test]
    fn windows_style_path_on_linux_does_not_panic_and_resolves_when_the_file_exists() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::create_dir_all(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src/main.rs"), "").unwrap();

        let text = r"src\main.rs:12:5";
        let offset = text.len() / 2;
        let resolved = resolve_link(text, offset, dir.path()).expect("a link");
        assert_eq!(resolved.path, dir.path().join("src/main.rs"));
        assert_eq!(resolved.line, 12);
        assert_eq!(resolved.col, Some(5));
    }

    // W5-2: a Linux `file:line` in a WSL run's output opens the UNC path
    // the share actually serves, not a path pasted straight under the
    // (Windows-shaped) cwd.
    #[test]
    fn a_relative_linux_path_resolves_to_the_unc_path_on_a_wsl_cwd() {
        let cwd = PathBuf::from("//wsl.localhost/Ubuntu/tmp/links-e2e-relative");
        fs::create_dir_all(cwd.join("src")).unwrap();
        fs::write(cwd.join("src/main.rs"), "").unwrap();

        let text = "src/main.rs:12:5";
        let offset = text.len() / 2;
        let resolved = resolve_link(text, offset, &cwd).expect("a link");
        assert_eq!(resolved.path, cwd.join("src/main.rs"));
        assert_eq!(resolved.line, 12);
        assert_eq!(resolved.col, Some(5));
    }

    #[test]
    fn an_absolute_linux_path_resolves_to_the_unc_path_on_a_wsl_cwd() {
        let cwd = PathBuf::from("//wsl.localhost/Ubuntu/tmp/links-e2e-absolute");
        fs::create_dir_all(&cwd).unwrap();
        let target = PathBuf::from("//wsl.localhost/Ubuntu/tmp/elsewhere-abs");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("lib.rs"), "").unwrap();

        let text = "/tmp/elsewhere-abs/lib.rs:3:1";
        let offset = text.len() / 2;
        let resolved = resolve_link(text, offset, &cwd).expect("a link");
        assert_eq!(resolved.path, target.join("lib.rs"));
    }

    #[test]
    fn byte_offset_outside_the_span_does_not_match() {
        let dir = tempfile::tempdir().expect("tempdir");
        fs::write(dir.path().join("main.rs"), "").unwrap();
        let text = "before main.rs:1:1 after";
        assert!(resolve_link(text, 0, dir.path()).is_none());
        assert!(resolve_link(text, text.len() - 1, dir.path()).is_none());
        assert!(resolve_link(text, 10, dir.path()).is_some());
    }
}
