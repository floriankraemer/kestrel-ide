//! Go-to-definition must stop before the expensive `tags` walk when the
//! caret is not on an identifier (issue #21).
//!
//! Asserted on the work actually done -- `syntax_core::QUERY_WALKS` counts
//! query walks -- rather than on wall-clock time, which is flaky. One test
//! function on purpose: the counter is process-wide, so two tests reading
//! it would race under the default parallel harness.

use std::path::Path;
use std::sync::atomic::Ordering;

const SOURCE: &str = r#"
fn helper() -> usize { 1 }

fn caller() -> usize {
    helper()
}
"#;

fn walks_during(f: impl FnOnce()) -> usize {
    let before = syntax_core::QUERY_WALKS.load(Ordering::Relaxed);
    f();
    syntax_core::QUERY_WALKS.load(Ordering::Relaxed) - before
}

#[test]
fn a_caret_off_any_identifier_skips_every_query_but_the_first() {
    let path = Path::new("lib.rs");
    let on_identifier = SOURCE.find("helper()").unwrap() + 1;
    let off_identifier = SOURCE.find("fn helper").unwrap() + 2; // the space

    // Warm the lazily compiled queries so their one-time cost is not
    // mistaken for a walk.
    let _ = index_core::resolve_declaration_in_buffer(path, SOURCE, on_identifier);

    let hit_walks = walks_during(|| {
        let hit = index_core::resolve_declaration_in_buffer(path, SOURCE, on_identifier);
        assert_eq!(
            hit.name, "helper",
            "test source must resolve on the hit path"
        );
    });
    let miss_walks = walks_during(|| {
        let miss = index_core::resolve_declaration_in_buffer(path, SOURCE, off_identifier);
        assert!(miss.name.is_empty(), "caret was not on an identifier");
    });

    assert_eq!(
        miss_walks, 1,
        "the miss path must run the `locals` walk and nothing else"
    );
    assert_eq!(
        hit_walks, 2,
        "the hit path must run `locals` and `tags`, but never `inherits`"
    );
}
