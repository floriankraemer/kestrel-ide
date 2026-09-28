//! ADR-0064's project-scope rules, end to end: `.gitignore` no longer
//! decides what is indexed, `IndexOptions`'s two lists do, and a rescope on
//! reopen purges what a new exclude newly covers.
//!
//! An integration test rather than a unit one because `lib.rs` is at its
//! size baseline and may only shrink (`scripts/check-file-size.sh`).

use std::fs;
use std::path::Path;

use index_core::{IndexOptions, TextIndex};

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

// `.gitignore` only affects version control now, not what is indexed. A
// gitignored file and a dotfile under a dot-directory are both searchable —
// the opposite of the rule before ADR-0064.
#[test]
fn a_gitignored_file_and_a_dotfile_under_a_dotdir_are_both_indexed() {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("git")
        .arg("init")
        .arg("-q")
        .arg(dir.path())
        .status()
        .unwrap();
    write(dir.path(), ".gitignore", "formerly.txt\n");
    write(dir.path(), "formerly.txt", "needle here");
    write(dir.path(), ".github/workflows/ci.yml", "needle here too");
    let index = TextIndex::build(dir.path()).unwrap();

    assert_eq!(index.search("needle", false, true).unwrap().len(), 2);
}

// ADR-0064's own rule: an *Excluded* folder and a global *Ignored names*
// entry are what the index skips now, not a `.gitignore`.
#[test]
fn an_excluded_folder_and_an_ignored_name_are_skipped_by_the_build() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "generated/out.txt", "needle here");
    write(dir.path(), "node_modules/pkg/index.js", "needle here too");
    write(dir.path(), "src/kept.txt", "needle here three");

    let options = IndexOptions {
        excluded: vec!["generated".to_string()],
        ignored_names: vec!["node_modules".to_string()],
    };
    let index = TextIndex::build_with_progress(dir.path(), &options, &|_| {}).unwrap();

    let matches = index.search("needle", false, true).unwrap();
    assert_eq!(matches.len(), 1, "{matches:?}");
    assert!(matches[0].path.ends_with("kept.txt"));
}

// Reopening with a scope that newly excludes an already-indexed folder
// purges those files — the same known/live diff a deleted file already goes
// through.
#[test]
fn rescoping_on_reopen_purges_files_a_new_exclude_covers() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "generated/out.txt", "needle here");
    write(dir.path(), "src/kept.txt", "needle here too");
    let index = TextIndex::build(dir.path()).unwrap();
    assert_eq!(index.search("needle", false, true).unwrap().len(), 2);
    drop(index);

    let options = IndexOptions {
        excluded: vec!["generated".to_string()],
        ignored_names: vec![],
    };
    let index = TextIndex::open_or_build_with_progress(dir.path(), &options, &|_| {}).unwrap();

    let matches = index.search("needle", false, true).unwrap();
    assert_eq!(matches.len(), 1, "{matches:?}");
    assert!(matches[0].path.ends_with("kept.txt"));
    assert_eq!(index.indexed_file_count(), 1);
}
