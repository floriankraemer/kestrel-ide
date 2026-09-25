//! Candidates for ADR-0064's "Found N ignored but not excluded folders"
//! notification (T6 of the project-scope plan): gitignored directories the
//! scope has not already ruled on, offered to the user on project open.
//!
//! The rule itself never runs `git` — it is handed each repository's
//! already-collected `git status --ignored` output (`vcs_core::Repository`
//! is below `project-model` in the layering, ADR-0064's own crate table)
//! and decides what, if anything, is worth asking about.

use std::fs;
use std::path::{Path, PathBuf};

use crate::ProjectScope;

/// How deep [`discover_nested_repos`] walks from a directory looking for a
/// `.git` before giving up on that branch.
///
/// A full [`ProjectScope::walk`] would find every nested repository too,
/// but at the cost of reading every in-scope directory on the whole tree —
/// fine for the index (which is doing that read anyway) and wasteful for a
/// notification that runs once per project open. Three levels covers a
/// workspace's own `projects/<name>/.git` shape without that cost.
pub const NESTED_REPO_DISCOVERY_DEPTH: u32 = 3;

/// One folder the notification may offer to exclude.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScopeCandidate {
    /// Project-relative, `/`-separated, no trailing slash.
    pub relative_path: String,
    /// Pre-checked state for the review dialog's checkbox: `false` when the
    /// folder itself holds a nested `.git` (a workspace directory grouping
    /// its own sub-repositories, e.g. a gitignored `projects/` holding
    /// checkouts of its own) — excluding it would take those repositories
    /// out of scope too, so the default is to leave it unchecked. `true`
    /// otherwise, an ordinary generated-output folder such as `dist/`.
    pub suggest_exclude: bool,
}

/// Walk up to [`NESTED_REPO_DISCOVERY_DEPTH`] levels under `root` (which is
/// itself assumed already known to be, or not be, a repository — this never
/// returns `root`) for directories holding their own `.git`, pruned by
/// `scope` so an excluded subtree (already off limits) is never read.
/// Bounded and cheap, not exhaustive: see [`NESTED_REPO_DISCOVERY_DEPTH`]'s
/// doc comment for why a full project walk is not used here.
pub fn discover_nested_repos(root: &Path, scope: &ProjectScope) -> Vec<PathBuf> {
    let mut found = Vec::new();
    scan_for_git_dirs(root, 0, scope, &mut found);
    found
}

/// Whether `dir` holds a nested `.git` within [`NESTED_REPO_DISCOVERY_DEPTH`]
/// levels of itself — the same bounded scan [`discover_nested_repos`] uses,
/// answering [`ScopeCandidate::suggest_exclude`] for one candidate folder.
fn contains_nested_git(dir: &Path, scope: &ProjectScope) -> bool {
    let mut found = Vec::new();
    scan_for_git_dirs(dir, 0, scope, &mut found);
    !found.is_empty()
}

fn scan_for_git_dirs(dir: &Path, depth: u32, scope: &ProjectScope, found: &mut Vec<PathBuf>) {
    if depth >= NESTED_REPO_DISCOVERY_DEPTH {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        let path = entry.path();
        if scope.is_excluded(&path, true) {
            continue;
        }
        if path.join(".git").exists() {
            found.push(path.clone());
        }
        scan_for_git_dirs(&path, depth + 1, scope, found);
    }
}

/// Build the candidate list for the notification.
///
/// `ignored_by_repo` is one entry per repository already discovered under
/// `root` — `root` itself plus whatever [`discover_nested_repos`] found —
/// pairing each repository's absolute path with the repository-relative
/// paths its own `git status --ignored` reported.
///
/// A path is offered when it: is a directory that still exists on disk
/// (`git status --ignored=matching` reports whichever of a folder or its
/// files matched, and only a folder is worth an *Excluded* entry); is
/// inside `root`; is not already out of scope (`scope`'s *Excluded*/
/// *Ignored names* lists — a name like `node_modules` is never offered,
/// since it is already skipped); and is not in `reviewed_not_excluded`
/// (the user already said no to it). Folders nested inside another
/// candidate are dropped — only the top-most is offered, since excluding
/// it excludes everything under it too.
pub fn candidate_folders(
    root: &Path,
    ignored_by_repo: &[(PathBuf, Vec<PathBuf>)],
    scope: &ProjectScope,
    reviewed_not_excluded: &[String],
) -> Vec<ScopeCandidate> {
    let mut relative_paths: Vec<String> = ignored_by_repo
        .iter()
        .flat_map(|(repo_root, ignored)| {
            ignored
                .iter()
                .filter_map(move |entry| project_relative_dir(root, repo_root, entry, scope))
        })
        .filter(|relative| !reviewed_not_excluded.iter().any(|r| r == relative))
        .collect();
    relative_paths.sort();
    relative_paths.dedup();

    top_most(relative_paths)
        .into_iter()
        .map(|relative_path| {
            let suggest_exclude = !contains_nested_git(&root.join(&relative_path), scope);
            ScopeCandidate {
                relative_path,
                suggest_exclude,
            }
        })
        .collect()
}

/// One `git status --ignored` entry, resolved and filtered: `None` unless
/// it is an existing directory under `root` that the scope has not already
/// excluded.
fn project_relative_dir(
    root: &Path,
    repo_root: &Path,
    repo_relative: &Path,
    scope: &ProjectScope,
) -> Option<String> {
    // `git status --ignored`'s directory entries carry a trailing `/`
    // (`"dist/"`); joining that onto `repo_root` verbatim leaves a
    // trailing-slash `PathBuf` that fails `Gitignore`'s own path
    // comparisons downstream (`ProjectScope::is_excluded`), so it is
    // stripped by rebuilding from components before anything else here
    // touches the path.
    let repo_relative: PathBuf = repo_relative.components().collect();
    if repo_relative.as_os_str().is_empty() {
        return None;
    }
    let absolute = repo_root.join(&repo_relative);
    if !absolute.is_dir() {
        return None;
    }
    if scope.is_excluded(&absolute, true) {
        return None;
    }
    let project_relative = absolute.strip_prefix(root).ok()?;
    if project_relative.as_os_str().is_empty() {
        return None;
    }
    Some(to_slash_string(project_relative))
}

fn to_slash_string(path: &Path) -> String {
    path.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Keep only the top-most entries of a sorted, deduplicated path list: an
/// entry is dropped when an already-kept entry is one of its ancestors.
/// Relies on lexicographic order putting a parent directly before every
/// path nested under it (`"a"` sorts before `"a/b"` since it is a strict
/// prefix), so one pass with the last kept entry is enough.
fn top_most(sorted_relative_paths: Vec<String>) -> Vec<String> {
    let mut kept: Vec<String> = Vec::new();
    for path in sorted_relative_paths {
        let nested_in_kept = kept.last().is_some_and(|parent| {
            path.starts_with(parent.as_str()) && path.as_bytes().get(parent.len()) == Some(&b'/')
        });
        if !nested_in_kept {
            kept.push(path);
        }
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn dir(root: &Path, relative: &str) -> PathBuf {
        let path = root.join(relative);
        fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(root: &Path, relative: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "").unwrap();
    }

    #[test]
    fn offers_an_ignored_directory_not_yet_reviewed_or_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        dir(root, "dist");
        file(root, "dist/bundle.js");
        let scope = ProjectScope::new(root, &[], &[]);

        let candidates = candidate_folders(
            root,
            &[(root.to_path_buf(), vec![PathBuf::from("dist/")])],
            &scope,
            &[],
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].relative_path, "dist");
        assert!(candidates[0].suggest_exclude);
    }

    #[test]
    fn skips_a_path_that_is_already_excluded_or_an_ignored_name() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        dir(root, "dist");
        dir(root, "node_modules");
        let scope = ProjectScope::new(root, &["dist".to_string()], &["node_modules".to_string()]);

        let candidates = candidate_folders(
            root,
            &[(
                root.to_path_buf(),
                vec![PathBuf::from("dist/"), PathBuf::from("node_modules/")],
            )],
            &scope,
            &[],
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn skips_a_path_already_reviewed_and_not_excluded() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        dir(root, "tmp");
        let scope = ProjectScope::new(root, &[], &[]);

        let candidates = candidate_folders(
            root,
            &[(root.to_path_buf(), vec![PathBuf::from("tmp/")])],
            &scope,
            &["tmp".to_string()],
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn only_the_top_most_of_a_nested_pair_is_offered() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        dir(root, "build");
        dir(root, "build/output");
        let scope = ProjectScope::new(root, &[], &[]);

        let candidates = candidate_folders(
            root,
            &[(
                root.to_path_buf(),
                vec![PathBuf::from("build/"), PathBuf::from("build/output/")],
            )],
            &scope,
            &[],
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].relative_path, "build");
    }

    #[test]
    fn a_file_entry_is_not_offered_only_directories_are() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        file(root, "generated.log");
        let scope = ProjectScope::new(root, &[], &[]);

        let candidates = candidate_folders(
            root,
            &[(root.to_path_buf(), vec![PathBuf::from("generated.log")])],
            &scope,
            &[],
        );

        assert!(candidates.is_empty());
    }

    #[test]
    fn a_folder_holding_a_nested_git_is_not_pre_checked() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        dir(root, "projects/sub-repo/.git");
        let scope = ProjectScope::new(root, &[], &[]);

        let candidates = candidate_folders(
            root,
            &[(root.to_path_buf(), vec![PathBuf::from("projects/")])],
            &scope,
            &[],
        );

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].relative_path, "projects");
        assert!(!candidates[0].suggest_exclude);
    }

    #[test]
    fn nested_repos_are_discovered_within_the_depth_bound_and_no_further() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        // depth 1: found. depth 4 (one past the bound): not found.
        dir(root, "a/repo-a/.git");
        dir(root, "x/y/z/too-deep/.git");
        let scope = ProjectScope::new(root, &[], &[]);

        let found = discover_nested_repos(root, &scope);

        assert!(found.contains(&root.join("a/repo-a")));
        assert!(!found.iter().any(|p| p.ends_with("too-deep")));
    }

    #[test]
    fn discovery_prunes_an_excluded_subtree() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        dir(root, "vendor/repo/.git");
        let scope = ProjectScope::new(root, &["vendor".to_string()], &[]);

        let found = discover_nested_repos(root, &scope);

        assert!(found.is_empty());
    }
}
