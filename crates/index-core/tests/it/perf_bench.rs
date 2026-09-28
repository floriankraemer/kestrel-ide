//! Benchmark behind issue #21, kept so the numbers in that PR can be re-taken.
//! `cargo test --release -p index-core --test it perf_bench:: -- --ignored --nocapture`
use std::path::{Path, PathBuf};
use std::time::Instant;

fn corpus() -> Vec<(PathBuf, String)> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut files: Vec<(PathBuf, u64)> = Vec::new();
    fn walk(dir: &Path, out: &mut Vec<(PathBuf, u64)>) {
        for e in std::fs::read_dir(dir).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().map(|n| n == "target").unwrap_or(false) {
                    continue;
                }
                walk(&p, out);
            } else if p.extension().map(|x| x == "rs").unwrap_or(false) {
                let len = e.metadata().unwrap().len();
                out.push((p, len));
            }
        }
    }
    walk(&root.join("crates"), &mut files);
    files.sort_by_key(|(p, len)| (std::cmp::Reverse(*len), p.clone()));
    files.truncate(30);
    let total: u64 = files.iter().map(|(_, l)| l).sum();
    eprintln!("corpus: {} files, {} KiB", files.len(), total / 1024);
    files
        .into_iter()
        .map(|(p, _)| {
            let c = std::fs::read_to_string(&p).unwrap();
            (p, c)
        })
        .collect()
}

fn median(mut v: Vec<u128>) -> u128 {
    v.sort();
    v[v.len() / 2]
}

/// Offset of a byte that is whitespace (no identifier there).
fn whitespace_offset(content: &str) -> usize {
    content
        .char_indices()
        .find(|(_, c)| *c == ' ' || *c == '\n')
        .map(|(i, _)| i)
        .unwrap_or(0)
}

/// Offset inside the first `fn NAME` token.
fn identifier_offset(content: &str) -> usize {
    content.find("fn ").map(|i| i + 4).unwrap_or(0)
}

#[test]
#[ignore]
fn bench() {
    let files = corpus();

    let mut miss = Vec::new();
    let mut hit = Vec::new();
    let mut index = Vec::new();
    for _ in 0..7 {
        let t = Instant::now();
        for (p, c) in &files {
            let off = whitespace_offset(c);
            std::hint::black_box(index_core::resolve_declaration_in_buffer(p, c, off));
        }
        miss.push(t.elapsed().as_millis());

        let t = Instant::now();
        for (p, c) in &files {
            let off = identifier_offset(c);
            std::hint::black_box(index_core::resolve_declaration_in_buffer(p, c, off));
        }
        hit.push(t.elapsed().as_millis());

        let t = Instant::now();
        for (p, c) in &files {
            let language = syntax_core::language_for_path(p);
            std::hint::black_box(syntax_core::analyze_file(language, c));
        }
        index.push(t.elapsed().as_millis());
    }
    let (miss, hit, control) = (median(miss), median(hit), median(index));
    eprintln!("goto-def MISS (caret off identifier): {miss} ms");
    eprintln!("goto-def HIT  (caret on identifier):  {hit} ms");
    eprintln!("analyze_file (indexing path):         {control} ms  [control]");
    // The host clock rate swings by ~2x between runs, so the ratio to the
    // unchanged full-analysis path is what is comparable across builds.
    eprintln!(
        "MISS/control {:.2}   HIT/control {:.2}",
        miss as f64 / control as f64,
        hit as f64 / control as f64
    );
}
