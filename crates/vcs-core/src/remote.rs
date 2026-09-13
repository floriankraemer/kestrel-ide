//! Remotes: fetch, pull, push, push with `--set-upstream` (F3-9).
//!
//! All three shell out (ADR-0031): they touch credentials, and fetch/push
//! may run `pre-push`/`post-fetch` hooks. Every failure `git` reports comes
//! back as a sentence — [`crate::error::VcsError::GitFailed`]'s `stderr` is
//! `git`'s own message verbatim, so an auth failure, a network failure or a
//! host that does not exist are each already explained in English without
//! this crate inventing its own wording for them. `push` always names an
//! explicit remote and branch (never a bare `git push`), so a missing
//! upstream is not a failure mode this crate's own argv can hit; the one
//! push failure worth a typed distinction is a non-fast-forward rejection —
//! see [`crate::error::VcsError::PushRejected`].

use gix::bstr::ByteSlice;

use crate::cli::{self, argv};
use crate::error::VcsError;
use crate::repo::Repository;

/// One configured remote (R7's remote picker, replacing the hard-coded
/// `origin` every push/pull/fetch call used before).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteInfo {
    pub name: String,
    pub url: String,
}

impl Repository {
    /// Every configured remote, name and fetch URL, sorted by name — a
    /// config read, so this goes through `gix` in-process (ADR-0031 §1)
    /// rather than shelling out for something no credential, hook or
    /// network call is involved in.
    pub fn remotes(&self) -> Result<Vec<RemoteInfo>, VcsError> {
        let mut names: Vec<_> = self.inner.remote_names().into_iter().collect();
        names.sort();
        let mut remotes = Vec::with_capacity(names.len());
        for name in names {
            let Ok(remote) = self.inner.find_remote(name.as_bstr()) else {
                continue;
            };
            let url = remote
                .url(gix::remote::Direction::Fetch)
                .map(|u| u.to_string())
                .unwrap_or_default();
            remotes.push(RemoteInfo {
                name: name.to_string(),
                url,
            });
        }
        Ok(remotes)
    }

    /// `git fetch <remote>`.
    pub fn fetch(&self, remote: &str) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &argv::fetch(remote))?;
        Ok(())
    }

    /// `git pull <remote> <branch>`.
    pub fn pull(&self, remote: &str, branch: &str) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &argv::pull(remote, branch))?;
        Ok(())
    }

    /// `git push [-u] <remote> <branch>`. Recognizes a non-fast-forward
    /// rejection as a typed error ([`VcsError::PushRejected`]) rather than
    /// leaving a caller to parse `stderr` itself.
    pub fn push(&self, remote: &str, branch: &str, set_upstream: bool) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        match cli::run(&work_dir, &argv::push(remote, branch, set_upstream)) {
            Ok(_) => Ok(()),
            Err(VcsError::GitFailed { stderr, .. }) if is_non_fast_forward(&stderr) => {
                Err(VcsError::PushRejected { stderr })
            }
            Err(e) => Err(e),
        }
    }
}

impl Repository {
    /// A bare `git push`, relying entirely on the configured upstream (or
    /// `remote.pushDefault`) rather than naming a remote and branch — the
    /// VCS menu's plain "Push" action (R7), which has no branch-popup
    /// selection to read a remote from. Unlike [`Self::push`], this *can*
    /// hit `git`'s own "no upstream branch" refusal, translated to
    /// [`VcsError::NoUpstream`] so the caller can fall back to the
    /// branch-popup's explicit remote picker instead of failing silently.
    pub fn push_tracking(&self) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        match cli::run(&work_dir, &["push"]) {
            Ok(_) => Ok(()),
            Err(VcsError::GitFailed { stderr, .. }) if is_no_upstream(&stderr) => {
                let branch = self.current_branch()?.unwrap_or_default();
                Err(VcsError::NoUpstream { branch })
            }
            Err(VcsError::GitFailed { stderr, .. }) if is_non_fast_forward(&stderr) => {
                Err(VcsError::PushRejected { stderr })
            }
            Err(e) => Err(e),
        }
    }
}

/// `git push` with no upstream configured and none named refuses with
/// "fatal: The current branch <name> has no upstream branch." on stderr.
fn is_no_upstream(stderr: &str) -> bool {
    stderr.contains("has no upstream branch")
}

/// A rejected push names the reason in a `[rejected]` line and/or the
/// canonical "Updates were rejected" hint, depending on `git` version and
/// whether the ref update was a fast-forward failure specifically.
fn is_non_fast_forward(stderr: &str) -> bool {
    stderr.contains("[rejected]") || stderr.contains("Updates were rejected")
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

    /// A bare "remote" repo and a "local" clone of it, both real, so fetch/
    /// pull/push exercise the real protocol over the filesystem transport
    /// rather than mocking git out.
    fn remote_and_clone() -> (tempfile::TempDir, tempfile::TempDir) {
        let remote_dir = tempfile::tempdir().unwrap();
        git(remote_dir.path(), &["init", "--quiet", "--bare"]);
        // The bare repo starts with no commits, so its HEAD points at
        // whatever the local default branch name is — which may not be
        // "main". Point it there explicitly so a later `git clone` checks
        // out "main" instead of failing with "remote HEAD refers to
        // nonexistent ref".
        git(
            remote_dir.path(),
            &["symbolic-ref", "HEAD", "refs/heads/main"],
        );

        let seed = tempfile::tempdir().unwrap();
        git(seed.path(), &["init", "--quiet"]);
        std::fs::write(seed.path().join("a.txt"), "one\n").unwrap();
        git(seed.path(), &["add", "a.txt"]);
        git(seed.path(), &["commit", "-m", "first"]);
        git(seed.path(), &["branch", "-M", "main"]);
        git(
            seed.path(),
            &["push", remote_dir.path().to_str().unwrap(), "main"],
        );

        let local_dir = tempfile::tempdir().unwrap();
        git(
            local_dir.path(),
            &["clone", "--quiet", remote_dir.path().to_str().unwrap(), "."],
        );
        git(
            local_dir.path(),
            &["config", "user.email", "test@example.com"],
        );
        git(local_dir.path(), &["config", "user.name", "Test"]);

        (remote_dir, local_dir)
    }

    #[test]
    fn remotes_lists_the_configured_origin_with_its_url() {
        let (remote_dir, local_dir) = remote_and_clone();
        let repo = open(local_dir.path());
        let remotes = repo.remotes().unwrap();
        assert_eq!(remotes.len(), 1);
        assert_eq!(remotes[0].name, "origin");
        assert!(remotes[0].url.contains(remote_dir.path().to_str().unwrap()));
    }

    #[test]
    fn push_tracking_on_a_branch_with_no_upstream_is_a_typed_error() {
        let (_remote_dir, local_dir) = remote_and_clone();
        git(local_dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(local_dir.path().join("c.txt"), "three\n").unwrap();
        git(local_dir.path(), &["add", "c.txt"]);
        git(local_dir.path(), &["commit", "-m", "feature work"]);

        let repo = open(local_dir.path());
        let err = repo.push_tracking().unwrap_err();
        match err {
            VcsError::NoUpstream { branch } => assert_eq!(branch, "feature"),
            other => panic!("expected NoUpstream, got {other:?}"),
        }
    }

    #[test]
    fn push_tracking_succeeds_once_an_upstream_is_set() {
        let (_remote_dir, local_dir) = remote_and_clone();
        git(local_dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(local_dir.path().join("c.txt"), "three\n").unwrap();
        git(local_dir.path(), &["add", "c.txt"]);
        git(local_dir.path(), &["commit", "-m", "feature work"]);

        let repo = open(local_dir.path());
        repo.push("origin", "feature", true).unwrap();
        repo.push_tracking().unwrap();
    }

    #[test]
    fn fetch_and_pull_bring_in_new_remote_commits() {
        let (remote_dir, local_dir) = remote_and_clone();

        // A second clone commits and pushes a change to the remote.
        let other = tempfile::tempdir().unwrap();
        git(
            other.path(),
            &["clone", "--quiet", remote_dir.path().to_str().unwrap(), "."],
        );
        git(other.path(), &["config", "user.email", "test@example.com"]);
        git(other.path(), &["config", "user.name", "Test"]);
        std::fs::write(other.path().join("b.txt"), "two\n").unwrap();
        git(other.path(), &["add", "b.txt"]);
        git(other.path(), &["commit", "-m", "second"]);
        git(other.path(), &["push", "origin", "main"]);

        let repo = open(local_dir.path());
        repo.fetch("origin").unwrap();
        // Fetched but not merged: b.txt must not be in the working tree yet.
        assert!(!local_dir.path().join("b.txt").exists());

        repo.pull("origin", "main").unwrap();
        assert!(local_dir.path().join("b.txt").exists());
    }

    #[test]
    fn push_with_set_upstream_creates_the_tracking_branch() {
        let (_remote_dir, local_dir) = remote_and_clone();
        git(local_dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(local_dir.path().join("c.txt"), "three\n").unwrap();
        git(local_dir.path(), &["add", "c.txt"]);
        git(local_dir.path(), &["commit", "-m", "feature work"]);

        let repo = open(local_dir.path());
        repo.push("origin", "feature", true).unwrap();

        let out = Command::new("git")
            .args(["rev-parse", "--abbrev-ref", "feature@{upstream}"])
            .current_dir(local_dir.path())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim(),
            "origin/feature"
        );
    }

    #[test]
    fn push_without_set_upstream_still_succeeds_with_an_explicit_branch() {
        // Unlike a bare `git push`, naming the remote and branch explicitly
        // never depends on a configured upstream — so this crate's push()
        // has no "no upstream configured" failure mode of its own to
        // report, only `git push`'s other outcomes.
        let (_remote_dir, local_dir) = remote_and_clone();
        git(local_dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(local_dir.path().join("c.txt"), "three\n").unwrap();
        git(local_dir.path(), &["add", "c.txt"]);
        git(local_dir.path(), &["commit", "-m", "feature work"]);

        let repo = open(local_dir.path());
        repo.push("origin", "feature", false).unwrap();
    }

    #[test]
    fn a_non_fast_forward_push_is_a_typed_error() {
        let (remote_dir, local_dir) = remote_and_clone();

        // Someone else pushes to origin/main first.
        let other = tempfile::tempdir().unwrap();
        git(
            other.path(),
            &["clone", "--quiet", remote_dir.path().to_str().unwrap(), "."],
        );
        git(other.path(), &["config", "user.email", "test@example.com"]);
        git(other.path(), &["config", "user.name", "Test"]);
        std::fs::write(other.path().join("b.txt"), "two\n").unwrap();
        git(other.path(), &["add", "b.txt"]);
        git(other.path(), &["commit", "-m", "second"]);
        git(other.path(), &["push", "origin", "main"]);

        // Local main is now behind; pushing without pulling first must be
        // rejected as non-fast-forward.
        std::fs::write(local_dir.path().join("c.txt"), "three\n").unwrap();
        git(local_dir.path(), &["add", "c.txt"]);
        git(local_dir.path(), &["commit", "-m", "diverged"]);

        let repo = open(local_dir.path());
        let err = repo.push("origin", "main", false).unwrap_err();
        assert!(matches!(err, VcsError::PushRejected { .. }));
    }
}
