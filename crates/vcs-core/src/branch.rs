//! Branches: list (via `gix`, no subprocess), create/checkout/delete (via
//! `git`, since checkout may run hooks) (F3-8).

use std::collections::HashMap;

use crate::cli::{self, argv};
use crate::error::VcsError;
use crate::repo::{HeadInfo, Repository};

/// What kind of ref one [`RefDecoration`] names — R7's log-row chips
/// distinguish a local branch from a remote-tracking one and a tag, and
/// highlight the branch `HEAD` currently sits on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefKind {
    Head,
    Local,
    Remote,
    Tag,
}

/// One ref pointing at a commit — [`Repository::refs_by_commit`]'s answer,
/// grouped by the commit id it decorates.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefDecoration {
    pub name: String,
    pub kind: RefKind,
}

/// How far back `git reset` moves the change, per
/// [`Repository::reset_to`]'s three modes — `git reset`'s own vocabulary,
/// not renamed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResetMode {
    /// Moves the branch pointer only; the index and working tree are
    /// untouched, so every change since stays staged.
    Soft,
    /// Moves the branch pointer and resets the index; the working tree is
    /// untouched, so every change since stays unstaged. `git reset`'s own
    /// default.
    Mixed,
    /// Moves the branch pointer, the index and the working tree —
    /// discards every change since outright.
    Hard,
}

impl ResetMode {
    fn flag(self) -> &'static str {
        match self {
            ResetMode::Soft => "--soft",
            ResetMode::Mixed => "--mixed",
            ResetMode::Hard => "--hard",
        }
    }
}

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

    /// Every remote-tracking branch name (e.g. `origin/main`), sorted —
    /// `gix`'s `references().remote_branches()`, the same in-process read
    /// [`Self::branches`] is. R7's branch popup Remote section.
    ///
    /// `<remote>/HEAD` — the symbolic ref `git clone`/`git remote set-head`
    /// creates pointing at the remote's default branch — is filtered out:
    /// it is not a branch a user can check out or compare against by that
    /// name, and showing it as a sibling of `origin/main` would read as a
    /// real, distinct branch.
    pub fn remote_branches(&self) -> Result<Vec<String>, VcsError> {
        let platform = self
            .inner
            .references()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let iter = platform
            .remote_branches()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        let mut names: Vec<String> = iter
            .map(|r| {
                r.map(|reference| reference.name().shorten().to_string())
                    .map_err(|e| VcsError::Read(e.to_string()))
            })
            .collect::<Result<_, _>>()?;
        names.retain(|name| !name.ends_with("/HEAD"));
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

    /// `git branch -m <old> <new>`.
    pub fn rename_branch(&self, old: &str, new: &str) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &["branch", "-m", old, new])?;
        Ok(())
    }

    /// `HEAD`, every local branch and every tag, grouped by the commit id
    /// each points at (R7's log-row ref chips) — one `gix` ref listing, no
    /// subprocess, the same in-process read [`Self::branches`]/[`Self::tags`]
    /// already are.
    pub fn refs_by_commit(&self) -> Result<HashMap<String, Vec<RefDecoration>>, VcsError> {
        let mut by_commit: HashMap<String, Vec<RefDecoration>> = HashMap::new();
        let mut push = |id: Option<String>, name: String, kind: RefKind| {
            if let Some(id) = id {
                by_commit
                    .entry(id)
                    .or_default()
                    .push(RefDecoration { name, kind });
            }
        };

        if let Ok(HeadInfo::Branch(name)) = self.head() {
            if let Ok(id) = self.inner.rev_parse_single("HEAD") {
                push(Some(id.to_hex().to_string()), name, RefKind::Head);
            }
        }

        let platform = self
            .inner
            .references()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        for reference in platform
            .local_branches()
            .map_err(|e| VcsError::Read(e.to_string()))?
        {
            let mut reference = reference.map_err(|e| VcsError::Read(e.to_string()))?;
            let name = reference.name().shorten().to_string();
            let id = reference
                .peel_to_id()
                .ok()
                .map(|id| id.to_hex().to_string());
            push(id, name, RefKind::Local);
        }

        let platform = self
            .inner
            .references()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        for reference in platform
            .remote_branches()
            .map_err(|e| VcsError::Read(e.to_string()))?
        {
            let mut reference = reference.map_err(|e| VcsError::Read(e.to_string()))?;
            let name = reference.name().shorten().to_string();
            let id = reference
                .peel_to_id()
                .ok()
                .map(|id| id.to_hex().to_string());
            push(id, name, RefKind::Remote);
        }

        let platform = self
            .inner
            .references()
            .map_err(|e| VcsError::Read(e.to_string()))?;
        for reference in platform.tags().map_err(|e| VcsError::Read(e.to_string()))? {
            let mut reference = reference.map_err(|e| VcsError::Read(e.to_string()))?;
            let name = reference.name().shorten().to_string();
            let id = reference
                .peel_to_id()
                .ok()
                .map(|id| id.to_hex().to_string());
            push(id, name, RefKind::Tag);
        }

        Ok(by_commit)
    }

    /// `git merge <branch>`. A conflicted merge is
    /// [`VcsError::MergeConflict`], not a bare [`VcsError::GitFailed`] — see
    /// [`conflicted_paths`].
    pub fn merge(&self, branch: &str) -> Result<(), VcsError> {
        self.run_integration(&["merge", branch])
    }

    /// `git rebase <onto>`. Conflict handling matches [`Self::merge`].
    pub fn rebase(&self, onto: &str) -> Result<(), VcsError> {
        self.run_integration(&["rebase", onto])
    }

    /// `git cherry-pick <id>`. Conflict handling matches [`Self::merge`].
    pub fn cherry_pick(&self, id: &str) -> Result<(), VcsError> {
        self.run_integration(&["cherry-pick", id])
    }

    /// `git revert --no-edit <id>` — reverts one commit as a new commit,
    /// distinct from [`crate::revert_hunk`]'s in-buffer, uncommitted
    /// reverts. Conflict handling matches [`Self::merge`].
    pub fn revert_commit(&self, id: &str) -> Result<(), VcsError> {
        self.run_integration(&["revert", "--no-edit", id])
    }

    /// `git reset --soft|--mixed|--hard <id>` — moves the current branch to
    /// `id`, per `mode`.
    pub fn reset_to(&self, id: &str, mode: ResetMode) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        cli::run(&work_dir, &["reset", mode.flag(), id])?;
        Ok(())
    }

    /// Runs one of the integration commands (`merge`/`rebase`/
    /// `cherry-pick`/`revert`) that can legitimately leave the working tree
    /// with unmerged paths, and turns that specific failure into
    /// [`VcsError::MergeConflict`] rather than the bare
    /// [`VcsError::GitFailed`] every other write returns. `git`'s exit code
    /// for a conflict is the same `1` a dozen unrelated refusals share, so
    /// this checks for unmerged paths after the fact instead of trying to
    /// read the refusal out of `stderr`.
    fn run_integration(&self, args: &[&str]) -> Result<(), VcsError> {
        let work_dir = self.work_dir().ok_or(VcsError::OutsideWorkingTree)?;
        match cli::run(&work_dir, args) {
            Ok(_) => Ok(()),
            Err(err @ VcsError::GitFailed { .. }) => match conflicted_paths(&work_dir) {
                Ok(paths) if !paths.is_empty() => Err(VcsError::MergeConflict { paths }),
                _ => Err(err),
            },
            Err(err) => Err(err),
        }
    }
}

/// Repository-relative paths `git diff --name-only --diff-filter=U` reports
/// — every path with unmerged (conflicted) stages in the index right now.
fn conflicted_paths(work_dir: &std::path::Path) -> Result<Vec<String>, VcsError> {
    let output = cli::run(work_dir, &["diff", "--name-only", "--diff-filter=U"])?;
    Ok(output.lines().map(str::to_string).collect())
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

    fn repo_head(dir: &Path) -> String {
        String::from_utf8(
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .current_dir(dir)
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string()
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
    fn remote_branches_lists_the_tracking_branch_after_a_clone() {
        let remote_dir = tempfile::tempdir().unwrap();
        git(remote_dir.path(), &["init", "--quiet", "--bare"]);
        git(
            remote_dir.path(),
            &["symbolic-ref", "HEAD", "refs/heads/main"],
        );
        let seed = tempfile::tempdir().unwrap();
        init_with_first_commit(seed.path());
        git(
            seed.path(),
            &["push", remote_dir.path().to_str().unwrap(), "main"],
        );
        let local_dir = tempfile::tempdir().unwrap();
        git(
            local_dir.path(),
            &["clone", "--quiet", remote_dir.path().to_str().unwrap(), "."],
        );

        let repo = open(local_dir.path());
        // `git clone` also creates `refs/remotes/origin/HEAD`, filtered out
        // — see `remote_branches`'s own doc comment.
        assert_eq!(
            repo.remote_branches().unwrap(),
            vec!["origin/main".to_string()]
        );
    }

    #[test]
    fn rename_branch_moves_the_current_branch() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        let repo = open(dir.path());
        repo.rename_branch("main", "trunk").unwrap();
        assert_eq!(repo.current_branch().unwrap(), Some("trunk".to_string()));
    }

    #[test]
    fn refs_by_commit_reports_head_and_local_branch_on_the_same_commit() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        let repo = open(dir.path());
        let head_id = repo_head(dir.path());

        let refs = repo.refs_by_commit().unwrap();
        let names: Vec<&str> = refs[&head_id].iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"main"));
        assert!(refs[&head_id].iter().any(|r| r.kind == RefKind::Head));
    }

    #[test]
    fn refs_by_commit_reports_a_tag() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["tag", "v1"]);
        let repo = open(dir.path());
        let head_id = repo_head(dir.path());

        let refs = repo.refs_by_commit().unwrap();
        assert!(refs[&head_id]
            .iter()
            .any(|r| r.name == "v1" && r.kind == RefKind::Tag));
    }

    #[test]
    fn merge_of_a_fast_forwardable_branch_succeeds() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-m", "feature work"]);
        git(dir.path(), &["checkout", "--quiet", "main"]);

        let repo = open(dir.path());
        repo.merge("feature").unwrap();
        assert!(dir.path().join("b.txt").exists());
    }

    #[test]
    fn merge_with_a_real_conflict_is_a_typed_error_naming_the_path() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(dir.path().join("a.txt"), "feature change\n").unwrap();
        git(dir.path(), &["commit", "-am", "feature change"]);
        git(dir.path(), &["checkout", "--quiet", "main"]);
        std::fs::write(dir.path().join("a.txt"), "main change\n").unwrap();
        git(dir.path(), &["commit", "-am", "main change"]);

        let repo = open(dir.path());
        let err = repo.merge("feature").unwrap_err();
        match err {
            VcsError::MergeConflict { paths } => assert_eq!(paths, vec!["a.txt".to_string()]),
            other => panic!("expected MergeConflict, got {other:?}"),
        }
    }

    #[test]
    fn cherry_pick_applies_the_named_commit_onto_head() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        git(dir.path(), &["checkout", "--quiet", "-b", "feature"]);
        std::fs::write(dir.path().join("b.txt"), "new\n").unwrap();
        git(dir.path(), &["add", "b.txt"]);
        git(dir.path(), &["commit", "-m", "feature work"]);
        let feature_head = repo_head(dir.path());
        git(dir.path(), &["checkout", "--quiet", "main"]);

        let repo = open(dir.path());
        repo.cherry_pick(&feature_head).unwrap();
        assert!(dir.path().join("b.txt").exists());
    }

    #[test]
    fn revert_commit_undoes_a_prior_commit_as_a_new_one() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        git(dir.path(), &["config", "user.email", "test@example.com"]);
        git(dir.path(), &["config", "user.name", "Test"]);
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);
        let second = repo_head(dir.path());

        let repo = open(dir.path());
        repo.revert_commit(&second).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
    }

    #[test]
    fn reset_soft_moves_head_but_keeps_the_change_staged() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);
        let first = std::str::from_utf8(
            &Command::new("git")
                .args(["rev-parse", "HEAD~1"])
                .current_dir(dir.path())
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .to_string();

        let repo = open(dir.path());
        repo.reset_to(&first, ResetMode::Soft).unwrap();
        assert_eq!(repo_head(dir.path()), first);
        // Soft: the content change since is still staged, not lost.
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "two\n"
        );
    }

    #[test]
    fn reset_hard_discards_the_working_tree_change_too() {
        let dir = tempfile::tempdir().unwrap();
        init_with_first_commit(dir.path());
        let first = repo_head(dir.path());
        std::fs::write(dir.path().join("a.txt"), "two\n").unwrap();
        git(dir.path(), &["commit", "-am", "second"]);

        let repo = open(dir.path());
        repo.reset_to(&first, ResetMode::Hard).unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("a.txt")).unwrap(),
            "one\n"
        );
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
