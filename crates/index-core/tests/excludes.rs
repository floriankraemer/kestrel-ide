//! What the index skips per its `IndexOptions` (ADR-0064: an *Excluded*
//! folder and a global *Ignored name*, not `.gitignore`).
//!
//! An integration test rather than a unit one because it is about the whole
//! build — patterns in, a search that cannot find the file out — and because
//! `lib.rs` is at its size baseline and may only shrink.

use std::fs;
use std::path::Path;

use index_core::{IndexOptions, TextIndex};

fn write(root: &Path, relative: &str, contents: &str) {
    let path = root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn index_with(root: &Path, excluded: &[&str], ignored_names: &[&str]) -> TextIndex {
    let options = IndexOptions {
        excluded: excluded.iter().map(|s| (*s).to_string()).collect(),
        ignored_names: ignored_names.iter().map(|s| (*s).to_string()).collect(),
    };
    TextIndex::build_with_progress(root, &options, &|_| {}).expect("index built")
}

#[test]
fn a_configured_exclude_keeps_a_directory_out_while_its_sibling_stays() {
    // Which layer the patterns came from is `settings_model::scope`'s
    // answer; all this crate promises is that the ones it is handed reach
    // the walker.
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "generated/out.txt", "needle here");
    write(dir.path(), "src/kept.txt", "needle here too");

    let index = index_with(dir.path(), &["generated"], &[]);

    let matches = index.search("needle", false, true).unwrap();
    assert_eq!(matches.len(), 1, "only the file outside the exclude");
    assert!(matches[0].path.ends_with("kept.txt"));
}

#[test]
fn an_ignored_name_is_skipped_at_any_depth() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "node_modules/pkg/index.js", "needle here");
    write(
        dir.path(),
        "src/node_modules/pkg/index.js",
        "needle here too",
    );
    write(dir.path(), "src/kept.txt", "needle here three");

    let index = index_with(dir.path(), &[], &["node_modules"]);

    let matches = index.search("needle", false, true).unwrap();
    assert_eq!(matches.len(), 1, "{matches:?}");
    assert!(matches[0].path.ends_with("kept.txt"));
}

#[test]
fn a_malformed_pattern_does_not_cost_you_the_whole_index() {
    // These patterns are typed into a settings page. Refusing to index the
    // project because one of them is a broken glob would turn a typo into
    // "search stopped working".
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "generated/out.txt", "needle here");
    write(dir.path(), "src/kept.txt", "needle here too");

    let index = index_with(dir.path(), &["generated", "["], &["["]);

    let matches = index.search("needle", false, true).unwrap();
    assert_eq!(matches.len(), 1, "the good pattern still applied");
    assert!(matches[0].path.ends_with("kept.txt"));
}

#[test]
fn no_excludes_or_ignored_names_indexes_everything_the_walker_would_have() {
    let dir = tempfile::tempdir().unwrap();
    write(dir.path(), "generated/out.txt", "needle here");
    write(dir.path(), "src/kept.txt", "needle here too");

    let index = index_with(dir.path(), &[], &[]);

    assert_eq!(index.search("needle", false, true).unwrap().len(), 2);
}
