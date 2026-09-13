//! Stash: push, pop, list, drop (R7). Shells out for the same reason every
//! other write in this crate does (ADR-0031): a stash push/pop can run
//! hooks and always touches the working tree and index together.

use crate::cli;
use crate::error::VcsError;
use crate::repo::Repository;

/// One entry in the stash list — `git stash list`'s own `stash@{n}`
/// indexing, newest (`stash@{0}`) first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StashEntry {
    /// The `n` in `stash@{n}`.
    pub index: usize,
    pub message: String,
}

impl Repository {
    /// `git stash push [-m <message>]`. An empty or absent message lets
    /// `git` generate its own default ("WIP on <branch>: ...").
    pub fn stash_push(&self, message: Option<&str>) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        let mut args = vec!["stash", "push"];
        if let Some(message) = message.filter(|m| !m.is_empty()) {
            args.push("-m");
            args.push(message);
        }
        cli::run(&work_dir, &args)?;
        Ok(())
    }

    /// `git stash pop stash@{<index>}`.
    pub fn stash_pop(&self, index: usize) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &["stash", "pop", &stash_ref(index)])?;
        Ok(())
    }

    /// `git stash drop stash@{<index>}`.
    pub fn stash_drop(&self, index: usize) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &["stash", "drop", &stash_ref(index)])?;
        Ok(())
    }

    /// `git stash list`, newest first — the same order `git` itself lists
    /// them in.
    pub fn stash_list(&self) -> Result<Vec<StashEntry>, VcsError> {
        let Some(work_dir) = self.work_dir() else {
            return Ok(Vec::new());
        };
        let output = cli::run(&work_dir, &["stash", "list", "--format=%gd%x00%s"])?;
        Ok(output.lines().filter_map(parse_stash_line).collect())
    }
}

fn stash_ref(index: usize) -> String {
    format!("stash@{{{index}}}")
}

/// One `%gd%x00%s` line, e.g. `stash@{0}\0WIP on main: ...`.
fn parse_stash_line(line: &str) -> Option<StashEntry> {
    let mut fields = line.split('\0');
    let reflog = fields.next()?;
    let message = fields.next()?.to_string();
    let index: usize = reflog
        .strip_prefix("stash@{")?
        .strip_suffix('}')?
        .parse()
        .ok()?;
    Some(StashEntry { index, message })
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

    fn init_with_first_commit(dir: &Path) {
        git(dir, &["init", "--quiet"]);
        std::fs::write(dir.join("a.txt"), "one\n").unwrap();
        git(dir, &["add", "a.txt"]);
        git(dir, &["commit", "-m", "first"]);
    }

    #[test]
    fn stash_list_on_a_clean_tree_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        let repo = open(dir.path());
        assert!(repo.stash_list().unwrap().is_empty());
    }

    #[test]
    fn stash_push_then_list_reports_the_message() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();

        let repo = open(dir.path());
        repo.stash_push(Some("my stash")).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );

        let list = repo.stash_list().unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].index, 0);
        assert!(list[0].message.contains("my stash"));
    }

    #[test]
    fn stash_pop_restores_the_working_tree_change() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();

        let repo = open(dir.path());
        repo.stash_push(None).unwrap();
        repo.stash_pop(0).unwrap();

        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "two\n"
        );
        assert!(repo.stash_list().unwrap().is_empty());
    }

    #[test]
    fn stash_drop_removes_the_entry_without_restoring_it() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();

        let repo = open(dir.path());
        repo.stash_push(None).unwrap();
        repo.stash_drop(0).unwrap();

        assert!(repo.stash_list().unwrap().is_empty());
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
    }
}
