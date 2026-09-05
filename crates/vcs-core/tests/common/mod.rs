//! Fixture repositories for the timing harness.
//!
//! Built once per `target/` volume and cached under `target/vcs-fixtures/`
//! behind a `.ready` marker: generating fifty thousand commits is a
//! multi-second job, and every timing run would otherwise pay it again.
//!
//! History is written with `git fast-import` rather than a `git commit` per
//! commit — the same repositories built one subprocess at a time would take
//! longer to generate than the measurements they exist to serve.
//!
//! The shapes are deliberately below kernel scale. What exposes the O(n) in
//! `status` and `file_history` is the *shape* — a wide worktree, a deep
//! history — not the absolute size, and a real kernel clone costs several
//! gigabytes of a disk this repository already keeps under pressure.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Where the generated repositories live. Under `target/` so a `make clean`
/// takes them with it, and so they are never mistaken for repository content.
fn fixture_root() -> PathBuf {
    // `CARGO_TARGET_TMPDIR` is inside `target/` and is created for us.
    PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("vcs-fixtures")
}

/// A generated repository: the path plus the paths inside it the timing
/// cases care about.
pub struct Fixture {
    pub root: PathBuf,
    /// A tracked, committed file large enough for a blob read and a diff to
    /// cost something — the gutter's own workload.
    pub target: PathBuf,
}

/// Build `name` if it isn't cached yet, then hand back its path.
///
/// The marker file is written last, so an interrupted generation is rebuilt
/// rather than served half-finished.
fn ensure(name: &str, build: impl FnOnce(&Path)) -> Fixture {
    let root = fixture_root().join(name);
    let marker = root.join(".ready");
    if !marker.exists() {
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("fixture root must be creatable");
        build(&root);
        fs::write(&marker, name).expect("marker must be writable");
    }
    Fixture {
        root: root.clone(),
        target: PathBuf::from(TARGET_PATH),
    }
}

/// The repository-relative path every fixture puts its large tracked file at.
pub const TARGET_PATH: &str = "src/target.txt";

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .args(args)
        .env("GIT_AUTHOR_NAME", "Test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "Test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .expect("git must be on PATH for the timing harness");
    assert!(status.success(), "git {args:?} failed");
}

/// Feed a `fast-import` stream into a fresh repository at `dir`.
fn fast_import(dir: &Path, stream: &str) {
    git(dir, &["init", "--quiet", "--initial-branch=main"]);
    let mut child = Command::new("git")
        .args(["fast-import", "--quiet"])
        .current_dir(dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("git fast-import must be available");
    child
        .stdin
        .take()
        .expect("stdin was piped")
        .write_all(stream.as_bytes())
        .expect("fast-import must accept the stream");
    let out = child.wait_with_output().expect("fast-import must finish");
    assert!(
        out.status.success(),
        "fast-import failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    // fast-import only writes objects and refs; the worktree and index are
    // still empty until this.
    git(dir, &["reset", "--hard", "--quiet", "main"]);
}

/// One `commit` command, appended to `stream`, modifying `files`
/// (path, contents) on `refs/heads/main`.
fn commit(stream: &mut String, n: usize, files: &[(String, String)]) {
    stream.push_str("commit refs/heads/main\n");
    stream.push_str("committer Test <test@example.com> ");
    // A fixed epoch plus the commit number: deterministic, and monotonic so
    // date-ordered walks see the history the author intended.
    stream.push_str(&(1_700_000_000u64 + n as u64).to_string());
    stream.push_str(" +0000\n");
    let message = format!("commit {n}\n");
    stream.push_str(&format!("data {}\n{}", message.len(), message));
    for (path, contents) in files {
        stream.push_str(&format!("M 100644 inline {path}\n"));
        stream.push_str(&format!("data {}\n{}", contents.len(), contents));
    }
    stream.push('\n');
}

/// `lines` lines of deterministic filler, seeded by `seed` so successive
/// revisions of the same file differ in a diffable way.
fn body(lines: usize, seed: usize) -> String {
    let mut text = String::with_capacity(lines * 32);
    for line in 0..lines {
        text.push_str(&format!("line {line} of revision {seed}\n"));
    }
    text
}

/// 50 files, 100 commits, one 200-line target. The constant-overhead floor:
/// whatever an operation costs here, it costs everywhere.
pub fn small() -> Fixture {
    ensure("small", |root| {
        let mut stream = String::new();
        for n in 0..100 {
            let mut files = vec![(format!("f{}.txt", n % 50), body(20, n))];
            if n == 0 {
                files.push((TARGET_PATH.to_string(), body(200, 0)));
            }
            commit(&mut stream, n, &files);
        }
        fast_import(root, &stream);
    })
}

/// 20 000 tracked files across 500 directories, plus 5 000 untracked ones.
/// The shape `Repository::status`'s dirwalk is O().
pub fn wide() -> Fixture {
    ensure("wide", |root| {
        let mut stream = String::new();
        let mut initial = vec![(TARGET_PATH.to_string(), body(200, 0))];
        for dir in 0..500 {
            for file in 0..40 {
                initial.push((format!("d{dir}/f{file}.txt"), body(5, dir + file)));
            }
        }
        commit(&mut stream, 0, &initial);
        for n in 1..300 {
            commit(
                &mut stream,
                n,
                &[(format!("d{}/f0.txt", n % 500), body(5, n))],
            );
        }
        fast_import(root, &stream);

        // Untracked files are a worktree fact, not a history one, so they go
        // in after the checkout rather than through fast-import.
        let untracked = root.join("untracked");
        fs::create_dir_all(&untracked).unwrap();
        for n in 0..5_000 {
            fs::write(untracked.join(format!("u{n}.txt")), "untracked\n").unwrap();
        }
    })
}

/// 200 files, 50 000 commits, and a target touched in only the first three.
///
/// `file_history` walking from `HEAD` therefore has to cross the entire
/// ancestry before it can report three matches — exactly the case where
/// capping *matches* rather than the walk does nothing.
pub fn deep() -> Fixture {
    ensure("deep", |root| {
        let mut stream = String::new();
        for n in 0..50_000usize {
            let mut files = vec![(format!("f{}.txt", n % 200), body(5, n))];
            if n < 3 {
                // A 2 000-line target: big enough that reading and diffing
                // its blob is a real cost, the way an open source file is.
                files.push((TARGET_PATH.to_string(), body(2_000, n)));
            }
            commit(&mut stream, n, &files);
        }
        fast_import(root, &stream);
        // Modern `git` writes this on `gc`/`fetch`; a freshly generated
        // repository has none until asked.
        git(root, &["commit-graph", "write", "--reachable"]);
    })
}

/// [`deep`] with its commit-graph removed, so `use_commit_graph(true)` can be
/// measured honestly against a repository that has never been `gc`'d.
pub fn deep_nograph() -> Fixture {
    let source = deep();
    ensure("deep-nograph", |root| {
        copy_dir(&source.root, root);
        let _ = fs::remove_file(root.join(".git/objects/info/commit-graph"));
        let _ = fs::remove_dir_all(root.join(".git/objects/info/commit-graphs"));
    })
}

fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_name() == ".ready" {
            // The marker is written by `ensure` once the copy is complete;
            // carrying the source's over would mark a half-built fixture
            // ready if this were interrupted.
            continue;
        }
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
