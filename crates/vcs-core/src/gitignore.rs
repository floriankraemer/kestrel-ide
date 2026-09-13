//! "Add to .gitignore" (R6): append one repository-relative path to the
//! root `.gitignore`, creating the file if it is missing, idempotently.
//!
//! The root file rather than a nested one, for the reason
//! `app_config::project_settings::ensure_root_gitignore_pattern` gives for
//! its own (`.ide/`-scoped) sibling: a nested `.gitignore` only ever
//! matches paths inside its own directory, and the one file that covers
//! every location is the root's. That sibling is not reused here because
//! `vcs-core` sits below `app-config` in `docs/architecture/layering.md` —
//! and the rule that matters here, *which line* names a git path (leading
//! `/` so `foo.txt` never also ignores `sub/foo.txt`, always `/`-separated
//! whatever the host), is git's, not a settings-file concern.

use std::fs;
use std::io::Write;
use std::path::Path;

use crate::error::VcsError;
use crate::repo::Repository;

const GITIGNORE: &str = ".gitignore";

/// The `.gitignore` line for one repository-relative path: anchored with
/// a leading `/` (so `draft.txt` does not also ignore `docs/draft.txt`),
/// `/`-separated regardless of the host's own separator.
pub fn gitignore_pattern(relative_path: &Path) -> String {
    let joined = relative_path
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    format!("/{joined}")
}

impl Repository {
    /// Append [`gitignore_pattern`]`(relative_path)` to the root
    /// `.gitignore`, creating the file if there is none. `Ok(false)` when
    /// the exact line is already there (nothing written) — a second "Add to
    /// .gitignore" on the same path is a no-op, never a duplicate line.
    pub fn add_to_gitignore(&self, relative_path: &Path) -> Result<bool, VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        let pattern = gitignore_pattern(relative_path);
        let path = work_dir.join(GITIGNORE);
        let existing = fs::read_to_string(&path).unwrap_or_default();
        if existing.lines().any(|line| line == pattern) {
            return Ok(false);
        }
        let mut file = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .map_err(|e| VcsError::Read(format!("{}: {e}", path.display())))?;
        let io = |e: std::io::Error| VcsError::Read(format!("{}: {e}", path.display()));
        if !existing.is_empty() && !existing.ends_with('\n') {
            writeln!(file).map_err(io)?;
        }
        writeln!(file, "{pattern}").map_err(io)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DiscoverResult;
    use std::path::PathBuf;
    use std::process::Command;

    fn open(dir: &Path) -> Repository {
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(dir)
            .status()
            .unwrap();
        match Repository::discover(dir).unwrap() {
            DiscoverResult::Found(repo) => *repo,
            DiscoverResult::NotARepository => panic!("expected a repository"),
        }
    }

    #[test]
    fn pattern_is_anchored_and_slash_separated() {
        assert_eq!(gitignore_pattern(Path::new("draft.txt")), "/draft.txt");
        assert_eq!(
            gitignore_pattern(&PathBuf::from("docs").join("notes.md")),
            "/docs/notes.md"
        );
    }

    #[test]
    fn add_creates_the_file_when_missing() {
        let dir = tempfile::tempdir().unwrap();
        let repo = open(dir.path());
        assert!(repo.add_to_gitignore(Path::new("scratch.log")).unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            "/scratch.log\n"
        );
    }

    #[test]
    fn add_appends_on_its_own_line_and_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "target/").unwrap();
        let repo = open(dir.path());
        assert!(repo.add_to_gitignore(Path::new("scratch.log")).unwrap());
        assert!(!repo.add_to_gitignore(Path::new("scratch.log")).unwrap());
        assert_eq!(
            fs::read_to_string(dir.path().join(".gitignore")).unwrap(),
            "target/\n/scratch.log\n"
        );
    }

    #[test]
    fn an_added_path_is_then_reported_ignored() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("scratch.log"), "noise\n").unwrap();
        let repo = open(dir.path());
        repo.add_to_gitignore(Path::new("scratch.log")).unwrap();
        assert_eq!(
            repo.ignored_paths().unwrap(),
            vec![PathBuf::from("scratch.log")]
        );
    }
}
