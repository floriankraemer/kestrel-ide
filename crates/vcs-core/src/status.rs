//! `git status --porcelain=v2 -z --branch --untracked-files=all --renames`
//! parsing (G1).
//!
//! One command replaces the `gix` walk `Repository::status` used to do: it
//! shells out through [`crate::cli`] (ADR-0031 — status honours
//! `.gitattributes`, `core.autocrlf`, sparse-checkout and, on a
//! `\\wsl.localhost\...` project, is computed by the *distro's* git against
//! the *distro's* index, which is the Windows fix this task exists for) and
//! reports everything the Changes panel needs in one pass: the branch name,
//! upstream, ahead/behind counts, ordinary changes, renames with their
//! original path, unmerged (conflict) entries and untracked files.
//!
//! `-z` NUL-terminates every record instead of newline-terminating it, and a
//! rename/copy record carries an extra NUL-separated token (the original
//! path) after its own fields — a path may itself contain a space, so this
//! parses the NUL-delimited token stream directly rather than splitting on
//! lines, following the style [`crate::blame::parse_porcelain`] (`blame.rs`)
//! already establishes for a pure, fixture-tested parser in this crate.

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::repo::{ChangeKind, FileStatus, RepoStatus};

/// Parse `git status --porcelain=v2 -z --branch --untracked-files=all
/// --renames`'s stdout.
///
/// Record shapes, one NUL-terminated token each except `2` (rename/copy),
/// which is followed by one further token holding the original path:
///
/// - `# branch.head <name>` — `<name>` is `(detached)` when `HEAD` is not on
///   a branch.
/// - `# branch.upstream <name>` — absent when there is no upstream.
/// - `# branch.ab +<ahead> -<behind>`
/// - `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>` — ordinary change.
/// - `2 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>\0<origPath>`
///   — rename/copy.
/// - `u <XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>` — unmerged.
/// - `? <path>` — untracked.
/// - `! <path>` — ignored; git does not emit these without `--ignored`, but
///   a record this parser does not recognize is skipped rather than
///   mis-parsed.
pub fn parse_porcelain_v2(output: &str) -> RepoStatus {
    let mut branch = None;
    let mut upstream = None;
    let mut ahead = 0u32;
    let mut behind = 0u32;
    let mut by_path: BTreeMap<PathBuf, FileStatus> = BTreeMap::new();

    let mut tokens = output.split('\0').filter(|t| !t.is_empty());
    while let Some(token) = tokens.next() {
        if let Some(header) = token.strip_prefix("# ") {
            parse_branch_header(header, &mut branch, &mut upstream, &mut ahead, &mut behind);
            continue;
        }
        match token.split_once(' ') {
            Some(("1", rest)) => {
                if let Some(entry) = parse_ordinary(rest) {
                    apply_ordinary(&mut by_path, entry);
                }
            }
            Some(("2", rest)) => {
                if let Some(entry) = parse_rename(rest) {
                    // The original path is the next NUL-delimited token, not
                    // part of this one — that is exactly what `-z` adds for
                    // a `2` record.
                    let orig_path = tokens.next().map(PathBuf::from);
                    apply_rename(&mut by_path, entry, orig_path);
                }
            }
            Some(("u", rest)) => {
                if let Some(path) = parse_unmerged(rest) {
                    apply_conflict(&mut by_path, path);
                }
            }
            Some(("?", path)) => {
                apply_untracked(&mut by_path, PathBuf::from(path));
            }
            // `!` (ignored) and anything unrecognized: not shown.
            _ => {}
        }
    }

    RepoStatus {
        files: by_path.into_values().collect(),
        branch,
        upstream,
        ahead,
        behind,
    }
}

fn parse_branch_header(
    header: &str,
    branch: &mut Option<String>,
    upstream: &mut Option<String>,
    ahead: &mut u32,
    behind: &mut u32,
) {
    if let Some(name) = header.strip_prefix("branch.head ") {
        *branch = Some(name.to_string());
    } else if let Some(name) = header.strip_prefix("branch.upstream ") {
        *upstream = Some(name.to_string());
    } else if let Some(counts) = header.strip_prefix("branch.ab ") {
        let mut parts = counts.split_whitespace();
        if let Some(a) = parts.next().and_then(|s| s.strip_prefix('+')) {
            *ahead = a.parse().unwrap_or(0);
        }
        if let Some(b) = parts.next().and_then(|s| s.strip_prefix('-')) {
            *behind = b.parse().unwrap_or(0);
        }
    }
    // `branch.oid`: the commit HEAD points at, unused today.
}

/// A parsed `1` (ordinary) entry: the path and its staged/unstaged letters.
struct OrdinaryEntry {
    path: PathBuf,
    x: char,
    y: char,
}

/// `<XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>` — 8 space-separated fields;
/// `splitn` caps the split count so a path containing a space stays intact
/// as the final field, and `.last()` on what remains after `xy` always
/// lands on that final field regardless of how many fields precede it.
fn parse_ordinary(rest: &str) -> Option<OrdinaryEntry> {
    let mut fields = rest.splitn(8, ' ');
    let xy = fields.next()?;
    let path = fields.last()?;
    let mut chars = xy.chars();
    Some(OrdinaryEntry {
        path: PathBuf::from(path),
        x: chars.next()?,
        y: chars.next()?,
    })
}

fn apply_ordinary(by_path: &mut BTreeMap<PathBuf, FileStatus>, entry: OrdinaryEntry) {
    let status = entry_for(by_path, entry.path);
    status.staged = letter_to_kind(entry.x);
    status.unstaged = letter_to_kind(entry.y);
}

struct RenameEntry {
    path: PathBuf,
    x: char,
    y: char,
}

/// `<XY> <sub> <mH> <mI> <mW> <hH> <hI> <X><score> <path>` — 9 fields, one
/// more (the rename score) than the ordinary shape has ahead of `path`.
fn parse_rename(rest: &str) -> Option<RenameEntry> {
    let mut fields = rest.splitn(9, ' ');
    let xy = fields.next()?;
    let path = fields.last()?;
    let mut chars = xy.chars();
    Some(RenameEntry {
        path: PathBuf::from(path),
        x: chars.next()?,
        y: chars.next()?,
    })
}

fn apply_rename(
    by_path: &mut BTreeMap<PathBuf, FileStatus>,
    entry: RenameEntry,
    orig_path: Option<PathBuf>,
) {
    let status = entry_for(by_path, entry.path);
    status.staged = letter_to_kind(entry.x);
    status.unstaged = letter_to_kind(entry.y);
    status.orig_path = orig_path;
}

/// `<XY> <sub> <m1> <m2> <m3> <mW> <h1> <h2> <h3> <path>` — 10 fields;
/// everything but the path is submodule/mode plumbing this crate has no
/// reader for, and an unmerged entry is a conflict regardless of which
/// side(s) changed it.
fn parse_unmerged(rest: &str) -> Option<PathBuf> {
    let path = rest.splitn(10, ' ').last()?;
    Some(PathBuf::from(path))
}

fn apply_conflict(by_path: &mut BTreeMap<PathBuf, FileStatus>, path: PathBuf) {
    let status = entry_for(by_path, path);
    status.staged = None;
    status.unstaged = Some(ChangeKind::Conflicted);
}

fn apply_untracked(by_path: &mut BTreeMap<PathBuf, FileStatus>, path: PathBuf) {
    let status = entry_for(by_path, path);
    status.unstaged = Some(ChangeKind::Untracked);
}

fn entry_for(by_path: &mut BTreeMap<PathBuf, FileStatus>, path: PathBuf) -> &mut FileStatus {
    by_path.entry(path.clone()).or_insert_with(|| FileStatus {
        path,
        staged: None,
        unstaged: None,
        orig_path: None,
    })
}

/// `.` means "no change in this dimension" — not a [`ChangeKind`] at all.
fn letter_to_kind(letter: char) -> Option<ChangeKind> {
    match letter {
        'M' => Some(ChangeKind::Modified),
        'A' => Some(ChangeKind::Added),
        'D' => Some(ChangeKind::Deleted),
        'R' => Some(ChangeKind::Renamed),
        'C' => Some(ChangeKind::Copied),
        'T' => Some(ChangeKind::TypeChanged),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a `-z` fixture from `\0`-separated records, the way this
    /// module's doc comment describes them (readable in source, `\0` in the
    /// actual byte stream `git` would produce).
    fn fixture(records: &[&str]) -> String {
        let mut out = records.join("\0");
        out.push('\0');
        out
    }

    #[test]
    fn a_modified_unstaged_file_reports_only_unstaged() {
        let status = parse_porcelain_v2(&fixture(&["1 .M N... 100644 100644 100644 hH hI a.txt"]));
        assert_eq!(status.files.len(), 1);
        let file = &status.files[0];
        assert_eq!(file.path, PathBuf::from("a.txt"));
        assert_eq!(file.staged, None);
        assert_eq!(file.unstaged, Some(ChangeKind::Modified));
        assert_eq!(file.orig_path, None);
    }

    #[test]
    fn a_staged_addition_reports_only_staged() {
        let status = parse_porcelain_v2(&fixture(&["1 A. N... 000000 100644 100644 hH hI b.txt"]));
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].staged, Some(ChangeKind::Added));
        assert_eq!(status.files[0].unstaged, None);
    }

    #[test]
    fn a_deleted_unstaged_file() {
        let status = parse_porcelain_v2(&fixture(&["1 .D N... 100644 100644 000000 hH hI c.txt"]));
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Deleted));
    }

    #[test]
    fn staged_and_unstaged_combo_on_one_path() {
        // Staged an addition, then edited again in the working tree.
        let status = parse_porcelain_v2(&fixture(&["1 AM N... 000000 100644 100644 hH hI d.txt"]));
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].staged, Some(ChangeKind::Added));
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Modified));
    }

    #[test]
    fn a_path_with_a_space_is_not_split() {
        let status = parse_porcelain_v2(&fixture(&[
            "1 .M N... 100644 100644 100644 hH hI my file.txt",
        ]));
        assert_eq!(status.files[0].path, PathBuf::from("my file.txt"));
    }

    #[test]
    fn rename_carries_score_and_original_path() {
        let status = parse_porcelain_v2(&fixture(&[
            "2 R. N... 100644 100644 100644 hH hI R100 new.txt",
            "old.txt",
        ]));
        assert_eq!(status.files.len(), 1);
        let file = &status.files[0];
        assert_eq!(file.path, PathBuf::from("new.txt"));
        assert_eq!(file.orig_path, Some(PathBuf::from("old.txt")));
        assert_eq!(file.staged, Some(ChangeKind::Renamed));
        assert_eq!(file.unstaged, None);
    }

    #[test]
    fn a_copy_is_distinguished_from_a_rename() {
        let status = parse_porcelain_v2(&fixture(&[
            "2 C. N... 100644 100644 100644 hH hI C100 copy.txt",
            "source.txt",
        ]));
        assert_eq!(status.files[0].staged, Some(ChangeKind::Copied));
        assert_eq!(status.files[0].orig_path, Some(PathBuf::from("source.txt")));
    }

    #[test]
    fn a_rename_further_modified_in_the_working_tree() {
        let status = parse_porcelain_v2(&fixture(&[
            "2 RM N... 100644 100644 100644 hH hI R087 new.txt",
            "old.txt",
        ]));
        let file = &status.files[0];
        assert_eq!(file.staged, Some(ChangeKind::Renamed));
        assert_eq!(file.unstaged, Some(ChangeKind::Modified));
        assert_eq!(file.orig_path, Some(PathBuf::from("old.txt")));
    }

    #[test]
    fn an_unmerged_entry_is_a_conflict_with_no_staged_state() {
        let status = parse_porcelain_v2(&fixture(&[
            "u UU N... 100644 100644 100644 100644 h1 h2 h3 parser.rs",
        ]));
        assert_eq!(status.files.len(), 1);
        let file = &status.files[0];
        assert_eq!(file.path, PathBuf::from("parser.rs"));
        assert_eq!(file.staged, None);
        assert_eq!(file.unstaged, Some(ChangeKind::Conflicted));
    }

    #[test]
    fn an_untracked_file_reports_as_untracked() {
        let status = parse_porcelain_v2(&fixture(&["? scratch.md"]));
        assert_eq!(status.files.len(), 1);
        assert_eq!(status.files[0].path, PathBuf::from("scratch.md"));
        assert_eq!(status.files[0].staged, None);
        assert_eq!(status.files[0].unstaged, Some(ChangeKind::Untracked));
    }

    #[test]
    fn an_ignored_record_is_not_listed() {
        let status = parse_porcelain_v2(&fixture(&["! ignored.log"]));
        assert!(status.files.is_empty());
    }

    #[test]
    fn branch_header_reports_name_and_ahead_behind() {
        let status = parse_porcelain_v2(&fixture(&[
            "# branch.oid abc123",
            "# branch.head main",
            "# branch.upstream origin/main",
            "# branch.ab +2 -1",
        ]));
        assert_eq!(status.branch, Some("main".to_string()));
        assert_eq!(status.upstream, Some("origin/main".to_string()));
        assert_eq!(status.ahead, 2);
        assert_eq!(status.behind, 1);
    }

    #[test]
    fn detached_head_reports_the_literal_marker() {
        let status = parse_porcelain_v2(&fixture(&["# branch.head (detached)"]));
        assert_eq!(status.branch, Some("(detached)".to_string()));
    }

    #[test]
    fn no_upstream_leaves_it_none_with_ahead_behind_zero() {
        let status = parse_porcelain_v2(&fixture(&["# branch.head main"]));
        assert_eq!(status.branch, Some("main".to_string()));
        assert_eq!(status.upstream, None);
        assert_eq!(status.ahead, 0);
        assert_eq!(status.behind, 0);
    }

    #[test]
    fn a_clean_repository_with_only_branch_headers_has_no_files() {
        let status = parse_porcelain_v2(&fixture(&[
            "# branch.oid abc123",
            "# branch.head main",
            "# branch.ab +0 -0",
        ]));
        assert!(status.files.is_empty());
    }

    #[test]
    fn empty_input_parses_to_an_empty_status() {
        let status = parse_porcelain_v2("");
        assert_eq!(status, RepoStatus::default());
    }
}
