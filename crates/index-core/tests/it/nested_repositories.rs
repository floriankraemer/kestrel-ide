//! Search Everywhere over a workspace that checks several repositories out
//! side by side.
//!
//! ADR-0064 deleted ADR-0063's depth-bounded search for nested `.git`
//! directories: a nested repository is simply part of the project unless
//! explicitly excluded, `.gitignore` no longer hides it (or its own
//! `vendor/`) from the index at all.
//!
//! An integration test because the promise is about the whole build — a
//! layout on disk in, a file and a symbol Search Everywhere can find out.

use std::fs;
use std::path::Path;

use index_core::TextIndex;

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

#[test]
fn files_and_symbols_of_a_nested_repository_are_found_gitignore_notwithstanding() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    fs::create_dir_all(root.join(".git")).unwrap();
    write(root, ".gitignore", "/projects/\n");
    fs::create_dir_all(root.join("projects/backend/.git")).unwrap();
    write(root, "projects/backend/.gitignore", "vendor/\n");
    write(
        root,
        "projects/backend/src/SortOrder.php",
        "<?php\nenum SortOrder: string {\n    case Asc = 'asc';\n}\n",
    );
    write(root, "projects/backend/vendor/Dependency.php", "<?php\n");

    let index = TextIndex::build(root).expect("index built");

    let files = index.find_files("SortOrder.php", 10);
    assert_eq!(files.len(), 1, "{files:?}");
    assert_eq!(files[0].relative, "projects/backend/src/SortOrder.php");
    assert_eq!(
        index.find_files("Dependency.php", 10).len(),
        1,
        "a nested repository's own .gitignore no longer hides its files"
    );
    let symbols = index.find_definitions_exact("SortOrder").unwrap();
    assert_eq!(symbols.len(), 1, "{symbols:?}");
}
