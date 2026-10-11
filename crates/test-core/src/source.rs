//! Where Jump to Source goes for a test node: its file, and the line of its
//! class, method or Pest call in that file's current text.

use std::path::{Path, PathBuf};

use process_exec::host::ExecHost;

use crate::location::{TestLocation, TestSymbol};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTarget {
    pub path: PathBuf,
    /// 1-based.
    pub line: u32,
}

/// The place `location` names on this machine. A line in the hint wins;
/// otherwise `symbol_line` (language knowledge, supplied by the caller) finds
/// the symbol in the text of the file, preferring `open_text` — the editor's
/// unsaved buffer — over the disk, and the file's first line is the fallback.
/// `None` when the location names no file, or the file does not exist.
pub fn source_target(
    location: &TestLocation,
    host: &ExecHost,
    work_dir: &Path,
    open_text: impl Fn(&Path) -> Option<String>,
    symbol_line: impl Fn(&str, &TestSymbol) -> Option<u32>,
) -> Option<SourceTarget> {
    if location.file.is_empty() {
        return None;
    }
    let path = location.local_path(host, work_dir);
    let open = open_text(&path);
    if open.is_none() && !path.is_file() {
        return None;
    }
    let line = location.line.or_else(|| {
        let symbol = location.symbol.as_ref()?;
        let text = open.or_else(|| std::fs::read_to_string(&path).ok())?;
        symbol_line(&text, symbol).map(|zero_based| zero_based + 1)
    });
    Some(SourceTarget {
        path,
        line: line.unwrap_or(1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::location::parse_location_hint;

    fn find(text: &str, symbol: &TestSymbol) -> Option<u32> {
        let TestSymbol::Method { method, .. } = symbol else {
            return None;
        };
        text.lines()
            .position(|l| l.contains(method.as_str()))
            .map(|n| n as u32)
    }

    fn target(hint: &str, open: Option<&str>, work: &Path) -> Option<SourceTarget> {
        let location = parse_location_hint(hint).unwrap();
        source_target(
            &location,
            &ExecHost::Local,
            work,
            |_| open.map(str::to_string),
            find,
        )
    }

    #[test]
    fn the_symbols_line_comes_from_the_file_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("T.php"), "<?php\n\nfunction testA() {}\n").unwrap();
        let got = target(r"php_qn://T.php::\T::testA", None, dir.path()).unwrap();
        assert_eq!((got.path, got.line), (dir.path().join("T.php"), 3));
    }

    #[test]
    fn the_open_buffer_beats_the_disk() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("T.php"), "testA\n").unwrap();
        let got = target(
            r"php_qn://T.php::\T::testA",
            Some("\n\n\ntestA"),
            dir.path(),
        );
        assert_eq!(got.unwrap().line, 4);
    }

    #[test]
    fn an_unknown_symbol_falls_back_to_line_one() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("T.php"), "nothing\n").unwrap();
        assert_eq!(
            target(r"php_qn://T.php::\T::testA", None, dir.path())
                .unwrap()
                .line,
            1
        );
    }

    #[test]
    fn a_hint_line_wins_without_reading_the_text() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("T.php"), "x\n").unwrap();
        assert_eq!(
            target("file://T.php:42", None, dir.path()).unwrap().line,
            42
        );
    }

    #[test]
    fn a_missing_file_or_an_unresolved_class_is_not_found() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(target(r"php_qn://Gone.php::\T", None, dir.path()), None);
        assert_eq!(
            target(r"pest_qn://Greeter (Tests\Greeter)", None, dir.path()),
            None
        );
    }
}
