//! Branches: list (via `gix`, no subprocess), create/checkout/delete (via
//! `git`, since checkout may run hooks) (F3-8).

use crate::cli::{self, argv};
use crate::error::VcsError;
use crate::repo::{HeadInfo, Repository};

impl Repository {
    /// Local branch names, sorted, via `gix` ref listing — no subprocess.
    pub fn branches(&self) -> Result<Vec<String>, VcsError> {
        let platform = self
            .inner
            .references()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let iter = platform
            .local_branches()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let mut names: Vec<String> = iter
            .map(|r| {
                r.map(|reference| reference.name().shorten().to_string())
                    .map_err(|e| VcsError::Read(e.to_string()))
            })
            .collect::<Result<_, _>>()?;
        names.sort();
        Ok(names)
    }

    /// Every tag name, sorted — `gix`'s `references().tags()`, the same
    /// in-process read [`Self::branches`] is (ADR-0031 §1).
    pub fn tags(&self) -> Result<Vec<String>, VcsError> {
        let platform = self
            .inner
            .references()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let iter = platform.tags().map_err(|e| VcsError::Read(e.to_string()))?;
        let mut names: Vec<String> = iter
            .map(|r| {
                r.map(|reference| reference.name().shorten().to_string())
                    .map_err(|e| VcsError::Read(e.to_string()))
            })
            .collect::<Result<_, _>>()?;
        names.sort();
        Ok(names)
    }

    /// What "Compare with Branch, Tag or Revision…" offers (R6): local
    /// branches first, then tags, each group sorted. A raw revision is
    /// typed rather than picked, so it is not listed here — every name
    /// here is one `git rev-parse` resolves as-is.
    pub fn ref_names(&self) -> Result<Vec<String>, VcsError> {
        let mut names = self.branches()?;
        names.extend(self.tags()?);
        Ok(names)
    }

    /// The current branch's name, or `None` for a detached or unborn `HEAD`
    /// (a caller wanting to distinguish those calls [`Repository::head`]
    /// directly).
    pub fn current_branch(&self) -> Result<Option<String>, VcsError> {
        Ok(match self.head()? {
            HeadInfo::Branch(name) => Some(name),
            HeadInfo::Detached(_) | HeadInfo::Unborn(_) => None,
        })
    }

    /// `git branch <name> [<start_point>]`.
    pub fn create_branch(&self, name: &str, start_point: Option<&str>) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &argv::branch_create(name, start_point))?;
        Ok(())
    }

    /// `git checkout <name>`. May run hooks (`post-checkout`), which is
    /// exactly why this shells out instead of moving `HEAD` via `gix`.
    pub fn checkout(&self, name: &str) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &argv::checkout(name))?;
        Ok(())
    }

    /// `git branch -d <name>`, or `-D` if `force`. Mirrors `git`'s own
    /// distinction rather than inventing a new one: a plain delete refuses
    /// a branch with commits not merged anywhere else, and that refusal
    /// comes back as [`VcsError::UnmergedBranch`] so a caller can offer a
    /// deliberate, explicit retry with `force: true` — never an automatic
    /// one.
    pub fn delete_branch(&self, name: &str, force: bool) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        match cli::run(&work_dir, &argv::branch_delete(name, force)) {
            Ok(_) => Ok(()),
            Err(VcsError::GitFailed { stderr, .. }) if is_unmerged_branch_refusal(&stderr) => {
                Err(VcsError::UnmergedBranch {
                    branch: name.to_string(),
                })
            }
            Err(e) => Err(e),
        }
    }
}

/// `git branch -d` on a branch with unmerged commits fails with a message
/// starting "error: The branch '<name>' is not fully merged." — recognized
/// by its "not fully merged" phrase rather than an exit code, since `git`
/// gives this refusal no distinct one (it shares `128`/`1` with every other
/// `branch -d` failure).
fn is_unmerged_branch_refusal(stderr: &str) -> bool {
    stderr.contains("is not fully merged")
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
        git(dir, &["branch", "-M", "main"]);
    }

    #[test]
    fn branches_lists_local_branches_sorted() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["branch", "zzz"]);
        git(dir.path(), &["branch", "aaa"]);

        let repo = open(dir.path());
        assert_eq!(repo.branches().unwrap(), vec!["aaa", "main", "zzz"]);
    }

    #[test]
    fn ref_names_lists_branches_then_tags() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["branch", "feature"]);
        git(dir.path(), &["tag", "v1"]);
        git(dir.path(), &["tag", "v0"]);

        let repo = open(dir.path());
        assert_eq!(repo.tags().unwrap(), vec!["v0", "v1"]);
        assert_eq!(
            repo.ref_names().unwrap(),
            vec!["feature", "main", "v0", "v1"]
        );
    }

    #[test]
    fn current_branch_names_the_checked_out_branch() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        let repo = open(dir.path());
        assert_eq!(repo.current_branch().unwrap(), Some("main".to_string()));
    }

    #[test]
    fn current_branch_is_none_when_detached() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["checkout", "--detach", "--quiet", "HEAD"]);
        let repo = open(dir.path());
        assert_eq!(repo.current_branch().unwrap(), None);
    }

    #[test]
    fn create_and_checkout_a_branch() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        let repo = open(dir.path());
        repo.create_branch("feature", None).unwrap();
        repo.checkout("feature").unwrap();
        assert_eq!(repo.current_branch().unwrap(), Some("feature".to_string()));
    }

    #[test]
    fn delete_a_merged_branch_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        let repo = open(dir.path());
        repo.create_branch("feature", None).unwrap();
        repo.delete_branch("feature", false).unwrap();
        assert_eq!(repo.branches().unwrap(), vec!["main"]);
    }

    #[test]
    fn delete_an_unmerged_branch_is_refused_without_force() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-m", "unmerged work"]);
        git(dir.path(), &["checkout", "--quiet", "main"]);

        let repo = open(dir.path());
        let err = repo.delete_branch("feature", false).unwrap_err();
        assert!(matches!(err, VcsError::UnmergedBranch { .. }));
        // Refused, not deleted.
        assert!(repo.branches().unwrap().contains(&"feature".to_string()));
    }

    #[test]
    fn force_delete_removes_an_unmerged_branch() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-m", "unmerged work"]);
        git(dir.path(), &["checkout", "--quiet", "main"]);

        let repo = open(dir.path());
        repo.delete_branch("feature", true).unwrap();
        assert!(!repo.branches().unwrap().contains(&"feature".to_string()));
    }
}
