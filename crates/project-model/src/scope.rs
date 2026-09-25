//! What is part of the project, independent of `.gitignore` (ADR-0064).
//!
//! [`ProjectScope`] is the one Qt-free rule every project walk goes through:
//! a folder the user marked *Excluded* (root-anchored, gitignore syntax) or
//! a name on the global *Ignored names* list (matched at any depth, the
//! usual gitignore basename rule for a pattern with no slash) is out of
//! scope. `.gitignore`, `.ignore`, git's exclude files and the blanket
//! dotfile rule play no part here — `T1`'s ADR spells out why the IDE stops
//! reading version-control ignore rules as project-scope rules.

use std::path::{Path, PathBuf};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use ignore::{DirEntry, WalkBuilder};

/// Decides whether a path is part of the project: everything under `root`
/// except the *Excluded* folders and *Ignored names* it was built from.
///
/// Both lists are folded into one [`Gitignore`] matcher at construction time
/// rather than checked separately per call — `is_excluded` and `walk` both
/// need "does this path or any ancestor match either list", and the
/// `ignore` crate already answers that question efficiently for one
/// matcher.
#[derive(Clone)]
pub struct ProjectScope {
    root: PathBuf,
    matcher: Gitignore,
}

impl ProjectScope {
    /// Build the scope for `root`: `excluded` are root-anchored gitignore
    /// patterns (a leading `!` or `/` is stripped before anchoring, since
    /// this list has no negation concept — every entry excludes), and
    /// `ignored_names` are gitignore patterns matched as given, so a bare
    /// name like `node_modules` matches at any depth (gitignore's own
    /// basename rule for a pattern with no slash) while a glob like `*.pyc`
    /// still works the same way.
    ///
    /// A pattern the `ignore` crate cannot parse is skipped rather than
    /// failing construction — a typo in a name a user typed into a settings
    /// field must not make the whole project unopenable.
    pub fn new(root: &Path, excluded: &[String], ignored_names: &[String]) -> Self {
        let mut builder = GitignoreBuilder::new(root);
        for entry in excluded {
            if let Some(pattern) = anchor_excluded(entry) {
                let _ = builder.add_line(None, &pattern);
            }
        }
        for name in ignored_names {
            if let Some(pattern) = normalize(name) {
                let _ = builder.add_line(None, &pattern);
            }
        }
        let matcher = builder.build().unwrap_or_else(|_| Gitignore::empty());
        Self {
            root: root.to_path_buf(),
            matcher,
        }
    }

    /// The root this scope was built for.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Whether `path` (an absolute path; `is_dir` says which kind of
    /// pattern it may match) is out of scope: it, or any ancestor of it
    /// below `root`, matches the excluded or ignored-names list.
    ///
    /// A path outside `root` is never excluded (there is nothing here to
    /// exclude it from), and neither is `root` itself — a project can never
    /// exclude its own root.
    pub fn is_excluded(&self, path: &Path, is_dir: bool) -> bool {
        if path == self.root || !path.starts_with(&self.root) {
            return false;
        }
        matches(&self.matcher, path, is_dir)
    }

    /// Walk `root`, handing every in-scope entry to `visit` — an excluded
    /// directory is pruned rather than merely skipped, so nothing under it
    /// is ever read off disk. Built with `standard_filters(false)`: none of
    /// `WalkBuilder`'s own `.gitignore`/hidden-file rules apply, only this
    /// scope's.
    pub fn walk(&self, mut visit: impl FnMut(DirEntry)) {
        let matcher = self.matcher.clone();
        let mut builder = WalkBuilder::new(&self.root);
        builder.standard_filters(false).filter_entry(move |entry| {
            // Depth 0 is the root itself — always kept, the same "a project
            // can never exclude its own root" rule `is_excluded` applies.
            if entry.depth() == 0 {
                return true;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            !matches(&matcher, entry.path(), is_dir)
        });
        for entry in builder.build().flatten() {
            visit(entry);
        }
    }
}

/// Whether `path` (absolute, under the root the matcher was built for) or
/// any of its ancestors down to that root matches the matcher — the one
/// check both [`ProjectScope::is_excluded`] and [`ProjectScope::walk`]'s
/// pruning rest on.
fn matches(matcher: &Gitignore, path: &Path, is_dir: bool) -> bool {
    matcher
        .matched_path_or_any_parents(path, is_dir)
        .is_ignore()
}

/// Normalise one *Excluded* entry into a root-anchored gitignore pattern:
/// trimmed, blank lines and comments (`#`) dropped, a leading `!` or `/`
/// stripped (this list has no negation concept), then re-anchored at the
/// root with a leading `/` — the same meaning gitignore syntax gives a
/// pattern written at the top of a `.gitignore` in `root`.
fn anchor_excluded(entry: &str) -> Option<String> {
    let normalized = normalize(entry)?;
    let stripped = normalized.trim_start_matches(['!', '/']);
    if stripped.is_empty() {
        return None;
    }
    Some(format!("/{stripped}"))
}

/// Trim one line from either list, dropping it if it is blank or a `#`
/// comment.
fn normalize(entry: &str) -> Option<String> {
    let trimmed = entry.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, relative: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, "").unwrap();
    }

    fn walked_paths(scope: &ProjectScope) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        scope.walk(|entry| paths.push(entry.into_path()));
        paths
    }

    #[test]
    fn an_anchored_excluded_folder_skips_its_subtree_but_not_a_same_named_folder_elsewhere() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "build/output.txt");
        write(root, "src/build/output.txt");

        let scope = ProjectScope::new(root, &["build".to_string()], &[]);

        assert!(scope.is_excluded(&root.join("build"), true));
        assert!(scope.is_excluded(&root.join("build/output.txt"), false));
        assert!(
            !scope.is_excluded(&root.join("src/build"), true),
            "the anchor is the root, not any directory named build"
        );

        let paths = walked_paths(&scope);
        assert!(!paths.contains(&root.join("build")));
        assert!(!paths.contains(&root.join("build/output.txt")));
        assert!(paths.contains(&root.join("src/build")));
        assert!(paths.contains(&root.join("src/build/output.txt")));
    }

    #[test]
    fn an_ignored_name_matches_at_any_depth() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "node_modules/pkg/index.js");
        write(root, "src/node_modules/pkg/index.js");
        write(root, "src/main.pyc");
        write(root, "vendor/lib.pyc");

        let scope = ProjectScope::new(
            root,
            &[],
            &["node_modules".to_string(), "*.pyc".to_string()],
        );

        assert!(scope.is_excluded(&root.join("node_modules"), true));
        assert!(scope.is_excluded(&root.join("src/node_modules"), true));
        assert!(scope.is_excluded(&root.join("src/main.pyc"), false));
        assert!(scope.is_excluded(&root.join("vendor/lib.pyc"), false));

        let paths = walked_paths(&scope);
        assert!(!paths.iter().any(|p| p.ends_with("node_modules")));
        assert!(!paths.iter().any(|p| p.ends_with("index.js")));
        assert!(!paths.iter().any(|p| p.ends_with(".pyc")));
    }

    #[test]
    fn ancestors_below_root_are_checked_not_just_the_path_itself() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a/node_modules/x/y.js");

        let scope = ProjectScope::new(root, &[], &["node_modules".to_string()]);

        assert!(scope.is_excluded(&root.join("a/node_modules/x/y.js"), false));
        assert!(!scope.is_excluded(&root.join("a"), true));
    }

    #[test]
    fn a_gitignore_in_the_tree_has_no_effect() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join(".gitignore"), "generated/\n").unwrap();
        write(root, "generated/file.txt");

        let scope = ProjectScope::new(root, &[], &[]);

        assert!(!scope.is_excluded(&root.join("generated"), true));
        let paths = walked_paths(&scope);
        assert!(paths.contains(&root.join("generated")));
        assert!(paths.contains(&root.join("generated/file.txt")));
    }

    #[test]
    fn dotfiles_and_dot_directories_are_walked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".github/ci.yml");
        write(root, ".env.example");

        let scope = ProjectScope::new(root, &[], &[]);

        assert!(!scope.is_excluded(&root.join(".github"), true));
        assert!(!scope.is_excluded(&root.join(".env.example"), false));
        let paths = walked_paths(&scope);
        assert!(paths.contains(&root.join(".github")));
        assert!(paths.contains(&root.join(".github/ci.yml")));
        assert!(paths.contains(&root.join(".env.example")));
    }

    #[test]
    fn dot_git_is_pruned_when_named_in_ignored_names() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, ".git/HEAD");
        write(root, "src/main.rs");

        let scope = ProjectScope::new(root, &[], &[".git".to_string()]);

        let paths = walked_paths(&scope);
        assert!(!paths.iter().any(|p| p.ends_with(".git")));
        assert!(!paths.iter().any(|p| p.ends_with("HEAD")));
        assert!(paths.contains(&root.join("src/main.rs")));
    }

    #[test]
    fn empty_lists_walk_everything() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "a/b.txt");
        write(root, ".dotfile");

        let scope = ProjectScope::new(root, &[], &[]);

        let paths = walked_paths(&scope);
        assert!(paths.contains(&root.join("a")));
        assert!(paths.contains(&root.join("a/b.txt")));
        assert!(paths.contains(&root.join(".dotfile")));
    }

    #[test]
    fn a_path_outside_root_is_never_excluded_and_the_root_itself_never_is() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("project");
        fs::create_dir_all(&root).unwrap();
        let outside = dir.path().join("elsewhere");

        let scope = ProjectScope::new(&root, &["project".to_string()], &["*".to_string()]);

        assert!(!scope.is_excluded(&outside, true));
        assert!(!scope.is_excluded(&root, true));
    }

    #[test]
    fn an_invalid_pattern_is_skipped_not_a_construction_failure() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(root, "kept.txt");

        // `[` opens an unterminated character class — invalid glob syntax.
        let scope = ProjectScope::new(root, &["[".to_string()], &["[".to_string()]);

        assert!(!scope.is_excluded(&root.join("kept.txt"), false));
    }
}
