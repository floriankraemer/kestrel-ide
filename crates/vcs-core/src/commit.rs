//! Commit: message, amend, and only the staged selection (F3-7).

use crate::cli::{self, argv};
use crate::error::VcsError;
use crate::repo::Repository;

/// What a commit is made with beyond its message (R6): `--amend`, an
/// author override, and a `Signed-off-by:` trailer. `author` is the literal
/// `Name <email>` `git --author` takes; an empty override is `None`, never
/// `Some("")`, so `git` is never handed an `--author=` it would reject.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CommitOptions {
    pub amend: bool,
    pub author: Option<String>,
    pub signoff: bool,
}

impl Repository {
    /// `git commit -m <message> [--amend]`. Deliberately never `-a`: this
    /// commits exactly what staging (F3-6) put in the index, nothing the
    /// working tree still holds unstaged.
    ///
    /// A pre-commit or commit-msg hook that rejects the commit surfaces as
    /// [`VcsError::GitFailed`] with the hook's own stderr verbatim — see
    /// that variant's doc comment for why this crate does not try to
    /// distinguish "a hook said no" from `git commit`'s other failure
    /// modes.
    pub fn commit(&self, message: &str, amend: bool) -> Result<(), VcsError> {
        self.commit_with(
            message,
            &CommitOptions {
                amend,
                ..CommitOptions::default()
            },
        )
    }

    /// [`Self::commit`] with the full option set: `--amend`, an
    /// `--author=<Name <email>>` override, and `--signoff` (R6). Never
    /// `-a`, for the same reason `commit` is not.
    pub fn commit_with(&self, message: &str, options: &CommitOptions) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        let args = argv::commit_with(
            message,
            options.amend,
            options.author.as_deref(),
            options.signoff,
        );
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        cli::run(&work_dir, &args)?;
        Ok(())
    }

    /// `HEAD`'s full commit message, for prefilling Amend — a pure object
    /// read, so this goes through `gix` in-process (ADR-0031 §1) rather
    /// than shelling out for something [`Self::commit`] itself already
    /// needs no subprocess to answer.
    ///
    /// `None` for an unborn `HEAD` (no commits yet, so there is nothing to
    /// amend) — every other read failure is a real [`VcsError::Read`].
    pub fn head_message(&self) -> Result<Option<String>, VcsError> {
        use gix::bstr::ByteSlice;
        let commit = match self.inner.head_commit() {
            Ok(commit) => commit,
            Err(gix::reference::head_commit::Error::PeelToCommit(
                gix::head::peel::to_commit::Error::PeelToObject(
                    gix::head::peel::to_object::Error::Unborn { .. },
                ),
            )) => return Ok(None),
            Err(e) => return Err(VcsError::Read(e.to_string())),
        };
        let message = commit
            .message_raw_sloppy()
            .to_str()
            .map_err(|e| VcsError::Read(e.to_string()))?
            .to_string();
        Ok(Some(message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::repo::DiscoverResult;
    use std::path::Path;
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .env("GIT_AUTHOR_NAME", "Test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "Test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    fn open(dir: &Path) -> Repository {
        match Repository::discover(dir).unwrap() {
            DiscoverResult::Found(repo) => *repo,
            DiscoverResult::NotARepository => panic!("expected a repository"),
        }
    }

    fn log_subjects(dir: &Path) -> Vec<String> {
        let out = Command::new("git")
            .args(["log", "--format=%s"])
            .current_dir(dir)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn commit_only_takes_what_was_staged() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        // Stage a.txt's edit, but leave a second, unstaged edit to b.txt.
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        std::fs::write(dir.path().join("b.txt"), "untracked\n").unwrap();

        let repo = open(dir.path());
        repo.commit("second", false).unwrap();

        assert_eq!(log_subjects(dir.path()), vec!["second", "first"]);
        // b.txt was never staged, so it must still be untracked, not
        // swept in by an accidental `-a`.
        let status = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&status.stdout), "?? b.txt\n");
    }

    #[test]
    fn amend_replaces_the_previous_commit_rather_than_adding_one() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "first"]);

        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);

        let repo = open(dir.path());
        repo.commit("first, amended", true).unwrap();

        assert_eq!(log_subjects(dir.path()), vec!["first, amended"]);
    }

    #[test]
    fn commit_with_author_and_signoff_reaches_the_commit() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);

        let repo = open(dir.path());
        repo.commit_with(
            "signed",
            &CommitOptions {
                amend: false,
                author: Some("Ada Lovelace <ada@example.com>".to_string()),
                signoff: true,
            },
        )
        .unwrap();

        let out = Command::new("git")
            .args(["log", "-1", "--format=%an <%ae>%n%B"])
            .current_dir(dir.path())
            .output()
            .unwrap();
        let shown = String::from_utf8_lossy(&out.stdout);
        assert!(
            shown.starts_with("Ada Lovelace <ada@example.com>"),
            "{shown}"
        );
        assert!(
            shown.contains("Signed-off-by: Test <test@example.com>"),
            "{shown}"
        );
    }

    #[test]
    fn head_message_reads_the_full_commit_message() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);
        git(dir.path(), &["commit", "-m", "subject\n\nbody line"]);

        let repo = open(dir.path());
        assert_eq!(
            repo.head_message().unwrap(),
            Some("subject\n\nbody line\n".to_string())
        );
    }

    #[test]
    fn head_message_is_none_for_an_unborn_head() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);

        let repo = open(dir.path());
        assert_eq!(repo.head_message().unwrap(), None);
    }

    #[test]
    fn a_rejecting_pre_commit_hook_surfaces_its_own_stderr() {
        let dir = tempfile::tempdir().unwrap();
        git(dir.path(), &["init", "--quiet"]);
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);

        let hooks_dir = dir.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks_dir).unwrap();
        let hook_path = hooks_dir.join("pre-commit");
        std::fs::write(
            &hook_path,
            "#!/bin/sh\necho 'no commits today' >&2\nexit 1\n",
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&hook_path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        std::fs::write(dir.path().join("a.txt"), "one\n").unwrap();
        git(dir.path(), &["add", "a.txt"]);

        let repo = open(dir.path());
        let err = repo.commit("first", false).unwrap_err();
        let message = err.to_string();
        assert!(
            message.contains("no commits today"),
            "hook's stderr was not surfaced verbatim: {message}"
        );
    }
}
