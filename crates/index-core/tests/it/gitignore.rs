//! The index's own directory ignores itself, `target/`-style (#345): built
//! before this existed, `.ide-index` showed up in `git status` as ordinary
//! untracked content, and a real `git add -A`/"Stage all" would stage its
//! (binary) segment files — which the Changes dock then tried to diff.
//!
//! An integration test rather than a unit one for the same reason
//! `excludes.rs` is: it is about the whole build (and, for the last test
//! here, a real `git` process reading what the build left behind), not an
//! internal detail, and `lib.rs` is at its size baseline and may only
//! shrink.

use std::fs;
use std::path::Path;
use std::process::Command;

use index_core::TextIndex;

const GITIGNORE_FILE: &str = ".gitignore";

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn git(root: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .current_dir(root)
        .status()
        .unwrap_or_else(|e| panic!("running git {args:?}: {e}"));
    assert!(status.success(), "git {args:?} failed");
}

#[test]
fn a_build_writes_a_self_ignoring_gitignore_into_the_index_directory() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/main.rs", "fn main() {}\n");

    TextIndex::build(dir.path()).unwrap();

    let index_dir = index_core::index_dir_for(dir.path());
    assert_eq!(
        fs::read_to_string(index_dir.join(GITIGNORE_FILE)).unwrap(),
        "*\n",
        "the index's own directory must ignore itself, `target/`-style",
    );
}

#[test]
fn a_rebuild_still_has_the_gitignore_after_wiping_the_directory() {
    // `build_with_progress` `remove_dir_all`s an existing index directory
    // before it writes anything new into it — the fix must survive that,
    // not just a first build.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    TextIndex::build(dir.path()).unwrap();

    TextIndex::build(dir.path()).unwrap();

    let index_dir = index_core::index_dir_for(dir.path());
    assert!(index_dir.join(GITIGNORE_FILE).exists());
}

#[test]
fn an_opened_index_backfills_a_gitignore_it_was_built_without() {
    // Simulates an index directory a pre-fix build left behind: opening it
    // (not rebuilding) must still heal it, or every project indexed before
    // this fix stays unignored until its next full rebuild.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    TextIndex::build(dir.path()).unwrap();
    let index_dir = index_core::index_dir_for(dir.path());
    fs::remove_file(index_dir.join(GITIGNORE_FILE)).unwrap();

    TextIndex::open_or_build(dir.path()).unwrap();

    assert!(
        index_dir.join(GITIGNORE_FILE).exists(),
        "opening an index built before this fix must still add the gitignore",
    );
}

#[test]
fn a_gitignore_under_the_index_directory_is_index_internal() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    let index = TextIndex::build(dir.path()).unwrap();

    let gitignore = index_core::index_dir_for(dir.path()).join(GITIGNORE_FILE);
    assert!(
        index.is_index_internal(&gitignore),
        "the index's own .gitignore must never be treated as project content",
    );
}

#[test]
fn a_built_index_is_invisible_to_git_status() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "src/main.rs", "fn main() {}\n");
    git(dir.path(), &["init", "--quiet"]);

    TextIndex::build(dir.path()).unwrap();

    let output = Command::new("git")
        .args(["status", "--porcelain"])
        .current_dir(dir.path())
        .output()
        .expect("running git status");
    let status = String::from_utf8_lossy(&output.stdout);
    assert!(
        !status.contains(".ide-index"),
        "a freshly built index must not show up in git status, got:\n{status}",
    );
}
