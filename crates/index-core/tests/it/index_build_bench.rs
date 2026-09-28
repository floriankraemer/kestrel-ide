//! Full-project index build benchmark: how long `TextIndex::build` (cold) and
//! `TextIndex::open_or_build` (warm, nothing changed) take on a real tree, and
//! how big the resulting index is.
//!
//! `cargo test --release -p index-core --test it index_build_bench:: -- --ignored --nocapture`
//!
//! Set `IDE_BENCH_ROOT=/path/to/repo` to bench a different tree; the default is
//! this workspace's own `crates/` directory.
//!
//! The tree is copied into a temp directory first, so the benchmark never
//! writes an `.ide-index/` into the source. `target/`, `.git/` and any existing
//! `.ide-index/` are left out of the copy, and a `.ignore` file is written at
//! the copy root so the `ignore` crate skips build output the same way it would
//! in the original checkout (a copy is not a git repository, so `.gitignore`
//! files in it are not honored on their own).

use std::path::{Path, PathBuf};
use std::time::Instant;

const SKIP_DIRS: [&str; 3] = ["target", ".git", ".ide-index"];

fn bench_root() -> PathBuf {
    match std::env::var_os("IDE_BENCH_ROOT") {
        Some(path) => PathBuf::from(path),
        None => Path::new(env!("CARGO_MANIFEST_DIR")).join("../../crates"),
    }
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for entry in std::fs::read_dir(from)?.flatten() {
        let name = entry.file_name();
        if SKIP_DIRS.iter().any(|skip| name == *skip) {
            continue;
        }
        let source = entry.path();
        let target = to.join(&name);
        if entry.file_type()?.is_dir() {
            copy_tree(&source, &target)?;
        } else if entry.file_type()?.is_file() {
            std::fs::copy(&source, &target)?;
        }
    }
    Ok(())
}

fn dir_size(dir: &Path) -> u64 {
    let mut total = 0;
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            total += dir_size(&entry.path());
        } else if let Ok(meta) = entry.metadata() {
            total += meta.len();
        }
    }
    total
}

#[test]
#[ignore]
fn bench_index_build() {
    let source = bench_root();
    assert!(
        source.is_dir(),
        "IDE_BENCH_ROOT is not a directory: {source:?}"
    );

    let temp = tempfile::TempDir::new().expect("temp dir");
    let root = temp.path().join("project");
    let copy_start = Instant::now();
    copy_tree(&source, &root).expect("copy tree");
    std::fs::write(root.join(".ignore"), "target\n").expect("write .ignore");
    eprintln!(
        "corpus: {} ({} KiB) copied in {:?}",
        source.display(),
        dir_size(&root) / 1024,
        copy_start.elapsed()
    );

    let cold_start = Instant::now();
    let index = index_core::TextIndex::build(&root).expect("cold build");
    let cold = cold_start.elapsed();
    let file_count = index.indexed_file_count();
    drop(index);

    let warm_start = Instant::now();
    let index = index_core::TextIndex::open_or_build(&root).expect("warm open");
    let warm = warm_start.elapsed();
    assert_eq!(index.indexed_file_count(), file_count);
    drop(index);

    let index_bytes = dir_size(&index_core::index_dir_for(&root));
    eprintln!("indexed files: {file_count}");
    eprintln!("cold build:    {cold:?}");
    eprintln!("warm open:     {warm:?}");
    eprintln!("index size:    {} KiB", index_bytes / 1024);
}
