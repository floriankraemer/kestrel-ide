//! The Files tab (C3): list a directory inside a container (`exec <id> ls
//! -la`) and read a single small file out of it (`cp <id>:<path> -`, a tar
//! stream on stdout).
//!
//! GNU `ls` (every Debian/Ubuntu/Fedora/Alpine-with-coreutils base image)
//! accepts `--time-style=+%s`, which turns its mtime column into a plain
//! Unix epoch — no locale-dependent month name to parse. A busybox base
//! image's `ls` (Alpine's default, before `coreutils` is installed) does
//! not recognise that flag at all and exits nonzero with "unrecognized
//! option", so [`ls_args`]/[`parse_ls`] fall back to plain `ls -la` and a
//! best-effort month/day/time split — the same limitation JetBrains'
//! Docker plugin's Files tab documents for busybox images: no year in the
//! busybox timestamp, so `mtime_epoch` there is `None` rather than a
//! guessed year.

use std::path::Path;

use crate::connection::Invocation;
use crate::ops::{run_op, OpError, OpErrorCode};

/// `exec <id> ls -la --time-style=+%s <dir>` — the GNU-coreutils attempt.
/// [`ls_args_busybox`] is the fallback once this comes back with
/// busybox's "unrecognized option" refusal.
pub fn ls_args(id: &str, dir: &str) -> Vec<String> {
    vec![
        "exec".to_string(),
        id.to_string(),
        "ls".to_string(),
        "-la".to_string(),
        "--time-style=+%s".to_string(),
        dir.to_string(),
    ]
}

/// `exec <id> ls -la <dir>` — plain long format, for a busybox `ls` that
/// refused `--time-style`.
pub fn ls_args_busybox(id: &str, dir: &str) -> Vec<String> {
    vec![
        "exec".to_string(),
        id.to_string(),
        "ls".to_string(),
        "-la".to_string(),
        dir.to_string(),
    ]
}

/// Whether `stderr` is busybox `ls` rejecting a GNU-only flag — the signal
/// [`list_dir`] uses to retry with [`ls_args_busybox`] instead of treating
/// the whole call as failed.
fn is_unrecognized_option(stderr: &str) -> bool {
    let lower = stderr.to_lowercase();
    lower.contains("unrecognized option") || lower.contains("invalid option")
}

/// One entry of a directory listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub name: String,
    pub kind: EntryKind,
    pub size: u64,
    /// `None` for a busybox listing, which prints no year — parsing one in
    /// would silently guess wrong for anything older than ~6 months
    /// (documented limitation, see the module doc comment).
    pub mtime_epoch: Option<i64>,
    /// The permission string verbatim (`drwxr-xr-x`), since the view shows
    /// it as-is rather than this crate deciding a rendering for it.
    pub mode: String,
    /// A symlink's target (`ls -la`'s ` -> target` suffix); empty
    /// otherwise.
    pub target: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    Dir,
    File,
    Symlink,
    Other,
}

fn kind_from_mode(mode: &str) -> EntryKind {
    match mode.chars().next() {
        Some('d') => EntryKind::Dir,
        Some('l') => EntryKind::Symlink,
        Some('-') => EntryKind::File,
        _ => EntryKind::Other,
    }
}

/// Parse GNU `ls -la --time-style=+%s`'s output: `total N` (skipped), then
/// one line per entry —
/// `mode links owner group size epoch name[ -> target]`. `.`/`..` are
/// dropped, the same as the Files tab wants (no "go to parent" row from
/// the listing itself).
pub fn parse_ls_gnu(output: &str) -> Vec<FileEntry> {
    output
        .lines()
        .filter(|line| !line.starts_with("total "))
        .filter_map(|line| parse_ls_line(line, true))
        .filter(|entry| entry.name != "." && entry.name != "..")
        .collect()
}

/// Shared by the GNU and busybox parsers: split the fixed leading columns
/// on whitespace runs, then treat everything after the epoch/date column
/// as the name (and an optional ` -> target`), so a name containing
/// spaces survives.
fn parse_ls_line(line: &str, gnu_epoch: bool) -> Option<FileEntry> {
    let trimmed = line.trim_end();
    if trimmed.is_empty() {
        return None;
    }
    let mut rest = trimmed;
    let mode = take_token(&mut rest)?;
    if !mode.starts_with(['d', '-', 'l', 'b', 'c', 'p', 's']) {
        return None; // Not an entry line (e.g. a stray "total 12").
    }
    let _links = take_token(&mut rest)?;
    let _owner = take_token(&mut rest)?;
    let _group = take_token(&mut rest)?;
    let size_token = take_token(&mut rest)?;
    let size: u64 = size_token.parse().unwrap_or(0);

    let mtime_epoch = if gnu_epoch {
        let epoch_token = take_token(&mut rest)?;
        epoch_token.trim_end_matches(['+', '-']).parse().ok()
    } else {
        // Busybox: `Mon DD HH:MM` or `Mon DD  YYYY`, three tokens, no
        // parseable epoch — see the module doc comment's documented
        // limitation.
        let _month = take_token(&mut rest)?;
        let _day = take_token(&mut rest)?;
        let _time_or_year = take_token(&mut rest)?;
        None
    };

    let name_field = rest.trim_start();
    if name_field.is_empty() {
        return None;
    }
    let (name, target) = match name_field.split_once(" -> ") {
        Some((name, target)) => (name.to_string(), target.to_string()),
        None => (name_field.to_string(), String::new()),
    };

    Some(FileEntry {
        kind: kind_from_mode(mode),
        mode: mode.to_string(),
        size,
        mtime_epoch,
        name,
        target,
    })
}

/// Pop the next whitespace-delimited token off the front of `rest`,
/// advancing `rest` past it and the whitespace that followed. `None` once
/// nothing is left.
fn take_token<'a>(rest: &mut &'a str) -> Option<&'a str> {
    let s = rest.trim_start();
    if s.is_empty() {
        return None;
    }
    match s.find(char::is_whitespace) {
        Some(index) => {
            let (token, remainder) = s.split_at(index);
            *rest = remainder;
            Some(token)
        }
        None => {
            *rest = "";
            Some(s)
        }
    }
}

/// Parse a busybox `ls -la`'s output (no `--time-style`, so no parseable
/// epoch — see [`FileEntry::mtime_epoch`]'s doc comment).
pub fn parse_ls_busybox(output: &str) -> Vec<FileEntry> {
    output
        .lines()
        .filter(|line| !line.starts_with("total "))
        .filter_map(|line| parse_ls_line(line, false))
        .filter(|entry| entry.name != "." && entry.name != "..")
        .collect()
}

/// List `dir` inside `id`: try the GNU form first, fall back to busybox's
/// plain `ls -la` on an "unrecognized option" refusal.
pub fn list_dir(
    invocation: &Invocation,
    id: &str,
    dir: &str,
    work_dir: &Path,
) -> Result<Vec<FileEntry>, OpError> {
    match run_op(invocation, &ls_args(id, dir), work_dir) {
        Ok(output) => Ok(parse_ls_gnu(&String::from_utf8_lossy(&output.stdout))),
        Err(err) if is_unrecognized_option(&err.message) => {
            let output = run_op(invocation, &ls_args_busybox(id, dir), work_dir)?;
            Ok(parse_ls_busybox(&String::from_utf8_lossy(&output.stdout)))
        }
        Err(err) => Err(err),
    }
}

/// `cp <id>:<path> <host_dest>` — Download…
pub fn download_args(id: &str, path: &str, host_dest: &str) -> Vec<String> {
    vec![
        "cp".to_string(),
        format!("{id}:{path}"),
        host_dest.to_string(),
    ]
}

/// A file this large is refused for the read-only preview (Download…
/// instead) — 8 MiB, generous for source/config text, small next to a
/// `tar` stream this crate reads fully into memory.
pub const MAX_READ_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// Read a single regular file out of a container via `cp <id>:<path> -`
/// (a `tar` stream on stdout) for the read-only Files-tab preview.
/// Refuses anything over [`MAX_READ_FILE_BYTES`] before even running the
/// command — Download… is the answer for a large file, not a preview tab.
pub fn read_file(
    invocation: &Invocation,
    id: &str,
    path: &str,
    work_dir: &Path,
) -> Result<String, OpError> {
    let args = vec!["cp".to_string(), format!("{id}:{path}"), "-".to_string()];
    let output = run_op(invocation, &args, work_dir)?;
    let entry = read_single_tar_entry(&output.stdout)?;
    Ok(String::from_utf8_lossy(&entry).into_owned())
}

/// A minimal single-entry tar reader: no crate, since `cp`'s stdout tar
/// stream here only ever needs its first (and only, for a single-file
/// `cp`) regular-file entry's bytes. USTAR/POSIX and legacy v7 headers are
/// both 512 bytes with the same `size` field at offset 124 (12 octal
/// ASCII digits, `\0`- or space-terminated) and `typeflag` at offset 156
/// (`'0'`/`'\0'` = regular file; anything else — a directory from a `cp`
/// of one, e.g. — is refused rather than silently returning garbage).
fn read_single_tar_entry(tar: &[u8]) -> Result<Vec<u8>, OpError> {
    const HEADER_SIZE: usize = 512;
    if tar.len() < HEADER_SIZE {
        return Err(OpError {
            code: OpErrorCode::Other,
            message: "empty or truncated archive".to_string(),
        });
    }
    let header = &tar[..HEADER_SIZE];
    let typeflag = header[156];
    if typeflag != b'0' && typeflag != 0 {
        return Err(OpError {
            code: OpErrorCode::Other,
            message: "not a regular file".to_string(),
        });
    }
    let size_field = &header[124..136];
    let size = parse_octal(size_field).ok_or_else(|| OpError {
        code: OpErrorCode::Other,
        message: "malformed tar header".to_string(),
    })?;
    if size > MAX_READ_FILE_BYTES {
        return Err(OpError {
            code: OpErrorCode::Other,
            message: "file is too large, download instead".to_string(),
        });
    }
    let start = HEADER_SIZE;
    let end = start + size as usize;
    if tar.len() < end {
        return Err(OpError {
            code: OpErrorCode::Other,
            message: "truncated archive".to_string(),
        });
    }
    Ok(tar[start..end].to_vec())
}

/// Parse a tar header's octal, `NUL`/space-terminated numeric field.
fn parse_octal(field: &[u8]) -> Option<u64> {
    let text = std::str::from_utf8(field).ok()?;
    let trimmed = text.trim_matches(|c: char| c == '\0' || c.is_whitespace());
    if trimmed.is_empty() {
        return Some(0);
    }
    u64::from_str_radix(trimmed, 8).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ls_argv() {
        assert_eq!(
            ls_args("abc123", "/etc"),
            vec!["exec", "abc123", "ls", "-la", "--time-style=+%s", "/etc"]
        );
        assert_eq!(
            ls_args_busybox("abc123", "/etc"),
            vec!["exec", "abc123", "ls", "-la", "/etc"]
        );
    }

    #[test]
    fn download_argv() {
        assert_eq!(
            download_args("abc123", "/etc/hosts", "/tmp/hosts"),
            vec!["cp", "abc123:/etc/hosts", "/tmp/hosts"]
        );
    }

    #[test]
    fn parses_a_gnu_listing_with_epoch_time_and_a_symlink() {
        let sample = "\
total 12
drwxr-xr-x  2 root root 4096 1700000000 .
drwxr-xr-x 20 root root 4096 1699999999 ..
-rw-r--r--  1 root root  220 1700000001 .bashrc
lrwxrwxrwx  1 root root    7 1700000002 bin -> usr/bin
drwxr-xr-x  2 root root 4096 1700000003 my folder";
        let entries = parse_ls_gnu(sample);
        assert_eq!(entries.len(), 3);
        assert_eq!(entries[0].name, ".bashrc");
        assert_eq!(entries[0].kind, EntryKind::File);
        assert_eq!(entries[0].size, 220);
        assert_eq!(entries[0].mtime_epoch, Some(1700000001));
        assert_eq!(entries[1].name, "bin");
        assert_eq!(entries[1].kind, EntryKind::Symlink);
        assert_eq!(entries[1].target, "usr/bin");
        assert_eq!(entries[2].name, "my folder");
        assert_eq!(entries[2].kind, EntryKind::Dir);
    }

    #[test]
    fn parses_a_busybox_listing_with_no_epoch() {
        let sample = "\
drwxr-xr-x    2 root     root          4096 Jan  1 00:00 etc
-rw-r--r--    1 root     root           220 Jan  1  2023 .bashrc";
        let entries = parse_ls_busybox(sample);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "etc");
        assert_eq!(entries[0].mtime_epoch, None);
        assert_eq!(entries[1].name, ".bashrc");
        assert_eq!(entries[1].size, 220);
        assert_eq!(entries[1].mtime_epoch, None);
    }

    #[test]
    fn detects_busybox_unrecognized_option_stderr() {
        assert!(is_unrecognized_option(
            "ls: unrecognized option '--time-style=+%s'"
        ));
        assert!(is_unrecognized_option("ls: invalid option -- 'time-style'"));
        assert!(!is_unrecognized_option("ls: cannot access '/nope'"));
    }

    fn tar_entry(name: &str, content: &[u8]) -> Vec<u8> {
        let mut header = vec![0u8; 512];
        let name_bytes = name.as_bytes();
        header[..name_bytes.len()].copy_from_slice(name_bytes);
        let size_octal = format!("{:011o}\0", content.len());
        header[124..124 + size_octal.len()].copy_from_slice(size_octal.as_bytes());
        header[156] = b'0'; // regular file
        let mut tar = header;
        tar.extend_from_slice(content);
        let padding = (512 - (content.len() % 512)) % 512;
        tar.extend(std::iter::repeat_n(0u8, padding));
        tar.extend(std::iter::repeat_n(0u8, 1024)); // two zero end-of-archive blocks
        tar
    }

    #[test]
    fn reads_a_single_regular_file_entry_out_of_a_tar_stream() {
        let tar = tar_entry("hosts", b"127.0.0.1 localhost\n");
        let bytes = read_single_tar_entry(&tar).unwrap();
        assert_eq!(bytes, b"127.0.0.1 localhost\n");
    }

    #[test]
    fn refuses_a_non_regular_entry() {
        let mut header = vec![0u8; 512];
        header[156] = b'5'; // directory
        header.extend(std::iter::repeat_n(0u8, 512));
        let err = read_single_tar_entry(&header).unwrap_err();
        assert_eq!(err.code, OpErrorCode::Other);
        assert!(err.message.contains("not a regular file"));
    }

    #[test]
    fn refuses_a_file_over_the_size_cap() {
        let mut header = vec![0u8; 512];
        let size_octal = format!("{:011o}\0", MAX_READ_FILE_BYTES + 1);
        header[124..124 + size_octal.len()].copy_from_slice(size_octal.as_bytes());
        header[156] = b'0';
        let err = read_single_tar_entry(&header).unwrap_err();
        assert!(err.message.contains("too large"));
    }

    #[test]
    fn refuses_a_truncated_archive() {
        let err = read_single_tar_entry(&[0u8; 100]).unwrap_err();
        assert_eq!(err.code, OpErrorCode::Other);
    }
}
