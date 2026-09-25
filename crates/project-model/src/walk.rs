//! Project traversal that treats a nested git repository as a root of its
//! own.
//!
//! A workspace that checks several repositories out side by side routinely
//! lists them in its own `.gitignore` (`/projects/`) — that line only keeps
//! them out of the *outer* repository, the same way git itself stops at a
//! nested `.git`. A plain `ignore::WalkBuilder` walk reads it as "not part of
//! the project" and never descends, so the index, the watcher and the MCP
//! tree walk all lost every file of every nested repository.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ignore::{DirEntry, WalkBuilder};

/// How far below the project root a nested repository is looked for:
/// `root/projects/backend` (depth 2) and one grouping level more.
///
/// ponytail: fixed depth, so a repository nested deeper inside an ignored
/// directory stays unindexed; make it a setting if someone needs one.
const NESTED_REPOSITORY_MAX_DEPTH: usize = 3;

/// Dependency trees are huge and occasionally contain checkouts with their
/// own `.git`; neither is a repository the user works in.
const SKIPPED_BY_NAME: &[&str] = &["node_modules"];

/// Walk `root` with the walker `configure` sets up, then walk each nested
/// git repository that walk never reached (because an ancestor's ignore
/// rules hid it) as a root of its own, with the same configuration.
///
/// Every entry is handed to `visit` once. A nested repository the outer walk
/// already covered is not walked again. `configure`'s overrides and
/// `filter_entry` also bound the nested-repository search, so a directory a
/// caller explicitly excludes keeps its repositories out too.
pub fn walk_project(
    root: &Path,
    configure: impl Fn(&mut WalkBuilder),
    mut visit: impl FnMut(DirEntry),
) {
    let mut walked_dirs = HashSet::new();
    walk_one(root, &configure, &mut walked_dirs, &mut visit);
    for repository in nested_repositories(root, &configure) {
        if !walked_dirs.contains(&repository) {
            walk_one(&repository, &configure, &mut walked_dirs, &mut visit);
        }
    }
}

fn walk_one(
    start: &Path,
    configure: &impl Fn(&mut WalkBuilder),
    walked_dirs: &mut HashSet<PathBuf>,
    visit: &mut impl FnMut(DirEntry),
) {
    let mut builder = WalkBuilder::new(start);
    configure(&mut builder);
    for entry in builder.build().filter_map(Result::ok) {
        if entry.file_type().is_some_and(|kind| kind.is_dir()) {
            walked_dirs.insert(entry.path().to_path_buf());
        }
        visit(entry);
    }
}

/// Directories below `root` (never `root` itself) holding a `.git`, found by
/// a depth-bounded walk that ignores `.gitignore` rules — those rules are
/// exactly what hides the repositories being looked for — outermost first.
fn nested_repositories(root: &Path, configure: &impl Fn(&mut WalkBuilder)) -> Vec<PathBuf> {
    let mut builder = WalkBuilder::new(root);
    configure(&mut builder);
    builder
        .standard_filters(false)
        .hidden(true)
        .max_depth(Some(NESTED_REPOSITORY_MAX_DEPTH))
        .filter_entry(|entry| {
            !SKIPPED_BY_NAME
                .iter()
                .any(|name| entry.file_name() == *name)
        });
    builder
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0 && entry.file_type().is_some_and(|kind| kind.is_dir()))
        .map(DirEntry::into_path)
        .filter(|dir| dir.join(".git").exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(root: &Path, relative: &str, content: &str) {
        let path = root.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    fn repo(dir: &Path) {
        fs::create_dir_all(dir.join(".git")).unwrap();
    }

    fn files(root: &Path, configure: impl Fn(&mut WalkBuilder)) -> Vec<String> {
        let mut found = Vec::new();
        walk_project(root, configure, |entry| {
            if entry.file_type().is_some_and(|kind| kind.is_file()) {
                let relative = entry.path().strip_prefix(root).unwrap();
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        });
        found.sort();
        found
    }

    #[test]
    fn a_nested_repository_the_outer_gitignore_hides_is_walked_with_its_own_rules() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repo(root);
        write(root, ".gitignore", "/projects/\n");
        write(root, "README.md", "");
        repo(&root.join("projects/backend"));
        write(root, "projects/backend/.gitignore", "vendor/\n");
        write(root, "projects/backend/src/Kernel.php", "");
        write(root, "projects/backend/vendor/lib.php", "");

        assert_eq!(
            files(root, |_| {}),
            vec!["README.md", "projects/backend/src/Kernel.php"],
            "the nested repository is walked, and its own .gitignore still applies"
        );
    }

    #[test]
    fn an_ignored_directory_without_a_repository_stays_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repo(root);
        write(root, ".gitignore", "target/\n");
        write(root, "target/debug/out.txt", "");
        write(root, "src/main.rs", "");

        assert_eq!(files(root, |_| {}), vec!["src/main.rs"]);
    }

    #[test]
    fn a_nested_repository_the_outer_walk_reaches_is_not_walked_twice() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repo(root);
        repo(&root.join("vendored"));
        write(root, "vendored/lib.rs", "");

        assert_eq!(files(root, |_| {}), vec!["vendored/lib.rs"]);
    }

    #[test]
    fn a_callers_override_keeps_its_nested_repositories_out() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repo(root);
        write(root, ".gitignore", "/projects/\n");
        repo(&root.join("projects/backend"));
        write(root, "projects/backend/src/Kernel.php", "");
        let root_path = root.to_path_buf();

        let found = files(root, |builder| {
            let mut overrides = ignore::overrides::OverrideBuilder::new(&root_path);
            overrides.add("!projects").unwrap();
            builder.overrides(overrides.build().unwrap());
        });
        assert!(found.is_empty(), "excluded, yet walked: {found:?}");
    }

    #[test]
    fn repositories_inside_node_modules_are_not_nested_projects() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        repo(root);
        write(root, ".gitignore", "node_modules/\n");
        repo(&root.join("node_modules/left-pad"));
        write(root, "node_modules/left-pad/index.js", "");

        assert!(files(root, |_| {}).is_empty());
    }
}
