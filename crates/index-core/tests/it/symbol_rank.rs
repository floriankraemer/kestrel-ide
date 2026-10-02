//! Go to Class / Symbol ranking. An integration test rather than a unit one
//! because `lib.rs` is at its size baseline and may only shrink
//! (`scripts/check-file-size.sh`).

use std::fs;

use index_core::TextIndex;

fn ranked_names(source: &str, query: &str) -> Vec<String> {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.rs"), source).unwrap();
    let index = TextIndex::build(dir.path()).unwrap();
    index
        .find_definitions_ranked(query, 10)
        .unwrap()
        .into_iter()
        .map(|m| m.name)
        .collect()
}

#[test]
fn the_best_fuzzy_hit_comes_first() {
    let names = ranked_names(
        "fn open_file() {}\nfn open_project_file_dialog() {}\n",
        "openfile",
    );
    assert_eq!(names[0], "open_file");
}

#[test]
fn an_empty_query_lists_definitions_up_to_the_limit() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.rs"), "fn one() {}\nfn two() {}\n").unwrap();
    let index = TextIndex::build(dir.path()).unwrap();
    assert_eq!(index.find_definitions_ranked("", 1).unwrap().len(), 1);
}

#[test]
fn prefix_and_substring_matches_rank_before_loose_subsequences() {
    let names = ranked_names(
        "struct GreaterToGreaterOrEqual;\nstruct MyGreeterHelper;\nstruct GreeterTest;\nstruct Greeter;\n",
        "Greeter",
    );
    assert_eq!(
        names,
        [
            "Greeter",
            "GreeterTest",
            "MyGreeterHelper",
            "GreaterToGreaterOrEqual"
        ]
    );
}
