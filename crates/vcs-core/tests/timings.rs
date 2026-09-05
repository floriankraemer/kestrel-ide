//! Timing harness for the operations behind the "git feels slow" report.
//!
//! Not a correctness test and not part of `make test`: `#[ignore]`d, run
//! deliberately inside the builder image.
//!
//! ```sh
//! make shell
//! cargo test -p vcs-core --test timings -- --ignored --nocapture
//! ```
//!
//! Deliberately not `criterion`. These cases run from a millisecond to
//! several seconds, where a p50/p95 over twenty iterations says everything
//! there is to say, and criterion's warm-up would fight the very caches the
//! measurements exist to characterise.

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use vcs_core::{DiscoverResult, HistoryCache, HunkCache, Repository};

/// How many timed iterations each case runs after its warm-up.
const ITERATIONS: usize = 20;

fn open(dir: &Path) -> Repository {
    match Repository::discover(dir).unwrap() {
        DiscoverResult::Found(repo) => *repo,
        DiscoverResult::NotARepository => panic!("{} is not a repository", dir.display()),
    }
}

/// Run `case` once to warm whatever it warms, then [`ITERATIONS`] times, and
/// print one CSV row.
fn time(name: &str, fixture: &str, mut case: impl FnMut()) {
    case();
    let mut samples = Vec::with_capacity(ITERATIONS);
    for _ in 0..ITERATIONS {
        let start = Instant::now();
        case();
        samples.push(start.elapsed());
    }
    samples.sort();
    println!(
        "{name},{fixture},{},{:.3},{:.3},{:.3}",
        ITERATIONS,
        ms(percentile(&samples, 50)),
        ms(percentile(&samples, 95)),
        ms(*samples.last().unwrap()),
    );
}

fn percentile(sorted: &[Duration], p: usize) -> Duration {
    let index = (sorted.len() * p / 100).min(sorted.len() - 1);
    sorted[index]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1_000.0
}

#[test]
#[ignore = "timing harness: run deliberately, not as part of `make test`"]
fn timings() {
    println!("case,fixture,n,p50_ms,p95_ms,max_ms");

    let small = common::small();
    let wide = common::wide();
    let deep = common::deep();
    let deep_nograph = common::deep_nograph();

    // Row B — the per-tick blob read. One call here; the bridge currently
    // makes two per debounce tick.
    for fixture in [&small, &deep] {
        let repo = open(&fixture.root);
        let name = fixture
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        time("head_blob", &name, || {
            repo.head_blob(&fixture.target).unwrap();
        });
    }

    // Row A — the gutter's cache. The first case asks twice with the same
    // revision (a hit, if the cache works at all); the second bumps it every
    // call, which is what the view does today.
    {
        let repo = open(&deep.root);
        let working = std::fs::read_to_string(deep.root.join(&deep.target)).unwrap() + "edited\n";
        let cache = HunkCache::new();
        time("hunks_same_revision", "deep", || {
            cache.hunks(&repo, &deep.target, &working, 1).unwrap();
        });
        let mut revision = 100u64;
        time("hunks_bumped_revision", "deep", || {
            revision += 1;
            cache
                .hunks(&repo, &deep.target, &working, revision)
                .unwrap();
        });
    }

    // Row E — whole-worktree status, as `refreshStatus` calls it today.
    for fixture in [&small, &wide] {
        let repo = open(&fixture.root);
        let name = fixture
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        time("status", &name, || {
            repo.status().unwrap();
        });
    }

    // Row F — the ancestry walk. `deep`'s target is touched in three of
    // fifty thousand commits, so capping matches cannot cap the walk.
    for fixture in [&deep, &deep_nograph] {
        let repo = open(&fixture.root);
        let name = fixture
            .root
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        time("file_history", &name, || {
            repo.file_history(&fixture.target, Some(200)).unwrap();
        });
        let cache = HistoryCache::new();
        time("file_history_cached", &name, || {
            cache
                .file_history(&repo, &fixture.target, Some(200))
                .unwrap();
        });
        time("log_200", &name, || {
            repo.log(Some(200)).unwrap();
        });
        // The escalation ADR-0031 left on the table for exactly this case:
        // `git log` has changed-path bloom filters and a pathspec, neither
        // of which `gix`'s revwalk exposes. Measured, not assumed.
        time("file_history_subprocess", &name, || {
            vcs_core::cli::run(
                &fixture.root,
                &[
                    "log",
                    "--format=%H%x00%an%x00%ae%x00%at%x00%s",
                    "-n",
                    "200",
                    "--",
                    common::TARGET_PATH,
                ],
            )
            .unwrap();
        });
    }

    // Row G — blame's cost split: the subprocess against the parse of its
    // own output, to settle whether the seam is worth touching.
    {
        let repo = open(&deep.root);
        time("blame", "deep", || {
            repo.blame(&deep.target).unwrap();
        });
        let porcelain = vcs_core::cli::run(
            &deep.root,
            &["blame", "--porcelain", "--", common::TARGET_PATH],
        )
        .unwrap();
        time("blame_parse_only", "deep", || {
            vcs_core::blame::parse_porcelain(&porcelain);
        });
    }

    // The spawn-overhead baseline that settles the library question
    // empirically: whatever a `git` process costs to start, it costs this.
    time("cli_spawn_status_porcelain", "small", || {
        vcs_core::cli::run(&small.root, &["status", "--porcelain"]).unwrap();
    });
}
