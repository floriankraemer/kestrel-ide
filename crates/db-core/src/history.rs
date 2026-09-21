//! Per-source execution history (database-tools.md §8):
//! `<config_dir>/database/history/<id>.jsonl`, capped at `history_cap`
//! entries. Stores the statement *text* a user ran, never bound parameter
//! values (ADR-0061 §1) — a logged `SELECT * FROM users WHERE ssn = ?`
//! never becomes a logged SSN.

use std::fs::{File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub fn history_file(config_dir: &Path, source_id: &str) -> PathBuf {
    config_dir
        .join("database")
        .join("history")
        .join(format!("{source_id}.jsonl"))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: DateTime<Utc>,
    /// The statement text only — never a bound parameter value.
    pub statement: String,
}

/// Append one entry, then keep only the last `cap` entries.
///
/// Reads the whole file back to re-cap it: history is bounded by
/// `history_cap` (default 1000 entries), so this is at most a few thousand
/// short lines — not a hot path worth a smarter ring-buffer-on-disk
/// scheme.
/// ponytail: full read-rewrite per append, revisit with a ring file if
/// `history_cap` ever grows past a few thousand.
pub fn append(path: &Path, entry: &HistoryEntry, cap: u32) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut entries = load(path)?;
    entries.push(entry.clone());
    let cap = cap as usize;
    if entries.len() > cap {
        entries.drain(0..entries.len() - cap);
    }
    let mut file = File::create(path)?;
    for entry in &entries {
        let line = serde_json::to_string(entry)?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

/// Every entry currently on disk, oldest first. An absent file is an
/// empty history, not an error — the same "unset is not a failure"
/// convention every other optional store in this codebase follows.
pub fn load(path: &Path) -> io::Result<Vec<HistoryEntry>> {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error),
    };
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        entries.push(serde_json::from_str(&line)?);
    }
    Ok(entries)
}

/// Truncate the file to zero entries (a user's "Clear history" action).
pub fn clear(path: &Path) -> io::Result<()> {
    OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(text: &str) -> HistoryEntry {
        HistoryEntry {
            timestamp: DateTime::from_timestamp(0, 0).unwrap(),
            statement: text.to_string(),
        }
    }

    #[test]
    fn loading_a_missing_file_is_an_empty_history_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_file(dir.path(), "abc");
        assert_eq!(load(&path).unwrap(), Vec::new());
    }

    #[test]
    fn append_then_load_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_file(dir.path(), "abc");
        append(&path, &entry("SELECT 1"), 1000).unwrap();
        append(&path, &entry("SELECT 2"), 1000).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded, vec![entry("SELECT 1"), entry("SELECT 2")]);
    }

    #[test]
    fn appending_past_the_cap_drops_the_oldest_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_file(dir.path(), "abc");
        for i in 0..5 {
            append(&path, &entry(&format!("SELECT {i}")), 3).unwrap();
        }
        let loaded = load(&path).unwrap();
        assert_eq!(
            loaded,
            vec![entry("SELECT 2"), entry("SELECT 3"), entry("SELECT 4")]
        );
    }

    #[test]
    fn clear_empties_the_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = history_file(dir.path(), "abc");
        append(&path, &entry("SELECT 1"), 1000).unwrap();
        clear(&path).unwrap();
        assert_eq!(load(&path).unwrap(), Vec::new());
    }
}
