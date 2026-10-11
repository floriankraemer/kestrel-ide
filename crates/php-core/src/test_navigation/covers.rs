//! Go to Test's index of the project's test files (#426): every `*Test.php`
//! file under the test directories, with the classes it says it covers
//! (`#[CoversClass]`, `#[CoversMethod]`, `@covers`, Pest's `covers()`).
//!
//! Go to Test lists the convention-named tests and every test that covers
//! the class, as PhpStorm does; reading every test file per keypress would
//! be up to 2000 reads on a `\\wsl.localhost` share. [`CoversIndex`] reads
//! them once, within the PHPUnit model's scan budget, and [`CoversCache`]
//! keeps it per project: built on a thread of its own on first use,
//! refreshed for the files the project watcher reports, and never touching
//! the disk on the caller's thread.
//!
//! Not `index-core`: its text index is language-agnostic, and answering
//! "which tests cover `App\Foo\Bar`" from it would still re-read and parse
//! every candidate file on each request, since a covered name only means
//! a class once the test's namespace and imports resolve it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use super::{covered_classes, relative, test_dirs, Declarations, TEST_SUFFIX};
use crate::composer::Psr4Root;
use crate::phpunit_xml::{self, Budget};

/// File changes kept for the next refresh; past this many the index is
/// rebuilt instead (a branch switch, a mass rename).
const MAX_PENDING_CHANGES: usize = 1000;

/// The project's test files and the classes each says it covers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoversIndex {
    project: PathBuf,
    /// The test directories scanned, absolute.
    dirs: Vec<PathBuf>,
    /// Every `*Test.php` file found -> the fully qualified classes it
    /// covers (often none).
    files: BTreeMap<PathBuf, Vec<String>>,
}

fn is_test_file_name(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        name.to_string_lossy()
            .ends_with(&format!("{TEST_SUFFIX}.php"))
    })
}

/// The classes the test file at `path` covers; `None` when it is unreadable
/// or over the scan's size cap. Only a file that mentions covering at all
/// is parsed.
fn read_covered(path: &Path) -> Option<Vec<String>> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > phpunit_xml::MAX_FILE_BYTES {
        return None;
    }
    let text = std::fs::read_to_string(path).ok()?;
    if !text.contains("covers") && !text.contains("Covers") {
        return Some(Vec::new());
    }
    Some(covered_classes(&text, &Declarations::read(&text)))
}

impl CoversIndex {
    /// Scan the test directories of `project` (every `autoload-dev` root,
    /// else `tests/`) within the PHPUnit model's budget. Blocking: call it
    /// off the GUI thread.
    pub fn build(project: &Path, roots: &[Psr4Root]) -> Self {
        let mut index = Self {
            project: project.to_path_buf(),
            dirs: test_dirs(roots, project),
            files: BTreeMap::new(),
        };
        let mut budget = Budget {
            files: phpunit_xml::MAX_SCANNED_FILES,
            entries: phpunit_xml::MAX_VISITED_ENTRIES,
        };
        let mut dirs = index.dirs.clone();
        dirs.sort();
        dirs.dedup();
        for dir in dirs {
            index.scan(dir, &mut budget);
        }
        index
    }

    /// Read every test file below `dir` the index does not hold yet: one it
    /// holds is re-read on its own watcher report.
    fn scan(&mut self, dir: PathBuf, budget: &mut Budget) {
        let mut pending = vec![dir];
        while let Some(dir) = pending.pop() {
            if budget.spent() {
                return;
            }
            for (path, is_dir, is_file) in phpunit_xml::entries_of(&dir, budget) {
                if is_dir && !phpunit_xml::is_skipped(&path) {
                    pending.push(path);
                } else if is_file
                    && is_test_file_name(&path)
                    && budget.files > 0
                    && !self.files.contains_key(&path)
                {
                    budget.files -= 1;
                    if let Some(covered) = read_covered(&path) {
                        self.files.insert(path, covered);
                    }
                }
            }
        }
    }

    /// Whether `path` lies in a scanned test directory, outside the
    /// dependency directories the scan never enters, or is a test
    /// directory's ancestor (a package renamed or deleted with its tests).
    fn holds(&self, path: &Path) -> bool {
        self.dirs.iter().any(|dir| {
            dir.starts_with(path)
                || path.strip_prefix(dir).is_ok_and(|rest| {
                    rest.parent()
                        .is_none_or(|parent| !parent.ancestors().any(phpunit_xml::is_skipped))
                })
        })
    }

    /// Bring `path` up to date after the watcher reported it: a test file is
    /// re-read or dropped, a new directory scanned, a removed one dropped.
    /// The index never grows past the scan budget's file count.
    pub fn refresh(&mut self, path: &Path) {
        if !self.holds(path) {
            return;
        }
        if path.is_dir() {
            let mut budget = Budget {
                files: phpunit_xml::MAX_SCANNED_FILES.saturating_sub(self.files.len()),
                entries: phpunit_xml::MAX_VISITED_ENTRIES,
            };
            let below: Vec<PathBuf> = self
                .dirs
                .iter()
                .filter(|dir| dir.starts_with(path))
                .cloned()
                .collect();
            // An ancestor of the test directories that still holds indexed
            // tests was only modified (Windows reports the project root on
            // every child write, `.phpunit.result.cache` included): its tests
            // arrive as reports of their own. Only a new one is scanned (a
            // package created or renamed into place, #434 review L7).
            let is_ancestor = !below.is_empty() && below.iter().all(|dir| dir != path);
            if is_ancestor && self.files.keys().any(|file| file.starts_with(path)) {
                return;
            }
            let targets = if below.is_empty() {
                vec![path.to_path_buf()]
            } else {
                below
            };
            for dir in targets {
                if !phpunit_xml::is_skipped(&dir) {
                    self.scan(dir, &mut budget);
                }
            }
            return;
        }
        let covered = is_test_file_name(path)
            .then(|| read_covered(path))
            .flatten();
        match covered {
            Some(covered) => {
                let room = self.files.len() < phpunit_xml::MAX_SCANNED_FILES;
                if room || self.files.contains_key(path) {
                    self.files.insert(path.to_path_buf(), covered);
                }
            }
            // Gone: the file, or a directory and everything indexed below it.
            None => self.files.retain(|file, _| !file.starts_with(path)),
        }
    }

    /// The project-relative test files that say they cover `subject`
    /// (fully qualified), sorted.
    // ponytail: a linear pass over at most the scan budget's 2000 files per
    // request; a subject -> files map if the budget grows.
    pub fn covering(&self, subject: &str) -> Vec<String> {
        self.files
            .iter()
            .filter(|(_, covered)| covered.iter().any(|class| class == subject))
            // A file deleted since the last watcher report is not offered.
            .filter(|(file, _)| file.is_file())
            .filter_map(|(file, _)| relative(&self.project, file))
            .collect()
    }

    /// The project-relative test files named `file_name`, sorted.
    pub fn named(&self, file_name: &str) -> Vec<String> {
        self.files
            .keys()
            .filter(|file| file.file_name().is_some_and(|name| name == file_name))
            .filter(|file| file.is_file())
            .filter_map(|file| relative(&self.project, file))
            .collect()
    }
}

/// One project's [`CoversIndex`], shared between the thread that keeps it
/// current and the Go to Test searches that read it.
#[derive(Debug, Default)]
pub struct CoversCache {
    state: Mutex<CacheState>,
    published: Condvar,
}

#[derive(Debug, Default)]
struct CacheState {
    project: PathBuf,
    roots: Vec<Psr4Root>,
    /// Bumped when the project changes: a worker of an older one stops.
    generation: u64,
    /// The last index the worker finished; read without the lock held.
    index: Option<Arc<CoversIndex>>,
    /// Watcher reports not yet applied to `index`.
    pending: Vec<PathBuf>,
    /// The index must be rebuilt: the roots changed, or too many files did.
    stale: bool,
    worker_running: bool,
    /// Searches blocked until the worker publishes.
    waiters: usize,
    /// The worker is building the index from scratch, not refreshing it.
    rebuilding: bool,
}

impl CoversCache {
    fn lock(&self) -> MutexGuard<'_, CacheState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The project watcher reported `path`. Only notes it, so the GUI thread
    /// that calls this never waits for a disk: the next [`Self::index`]
    /// applies it.
    pub fn file_changed(&self, path: &Path) {
        let mut state = self.lock();
        let relevant = match &state.index {
            Some(index) => index.holds(path),
            None => path.starts_with(&state.project),
        };
        // A PHP file, or what may be a directory.
        let php_or_dir = path
            .extension()
            .is_none_or(|extension| extension.eq_ignore_ascii_case("php"));
        if !relevant || !php_or_dir || state.stale {
            return;
        }
        state.pending.push(path.to_path_buf());
        if state.pending.len() > MAX_PENDING_CHANGES {
            state.pending.clear();
            state.stale = true;
        }
    }

    /// The index for `project` with the PSR-4 `roots` its `composer.json`
    /// declares, current with every change reported so far. When it is not
    /// built yet, a build starts on a thread of its own and the answer is
    /// `None`, unless `wait` asks to block until it is done. Never call it
    /// with `wait` on the GUI thread.
    pub fn index(
        self: &Arc<Self>,
        project: &Path,
        roots: &[Psr4Root],
        wait: bool,
    ) -> Option<Arc<CoversIndex>> {
        let mut state = self.lock();
        if state.project != project {
            *state = CacheState {
                project: project.to_path_buf(),
                generation: state.generation + 1,
                // The old project's searches still wait, to give up below.
                waiters: state.waiters,
                ..CacheState::default()
            };
            // A search still waiting for the old project's index gives up.
            self.published.notify_all();
        }
        // The roots only grow: a jump from another monorepo package adds its
        // roots (#428), and alternating between two packages must not
        // rebuild the index each time.
        // ponytail: a root dropped from a composer.json stays indexed until
        // the project is reopened; harmless, its tests still exist.
        let added: Vec<Psr4Root> = roots
            .iter()
            .filter(|root| !state.roots.contains(root))
            .cloned()
            .collect();
        if !added.is_empty() {
            state.roots.extend(added);
            // Even while the first build runs: it reads the old roots.
            state.stale = true;
        }
        let current = |state: &CacheState| {
            let settled = !state.stale && state.pending.is_empty() && !state.worker_running;
            settled.then(|| state.index.clone()).flatten()
        };
        if let Some(index) = current(&state) {
            return Some(index);
        }
        if !state.worker_running {
            state.worker_running = true;
            let cache = Arc::clone(self);
            let generation = state.generation;
            std::thread::spawn(move || cache.keep_current_or_give_up(generation));
        }
        // A refresh of a few files is quick and answers better than the
        // index before it; only a build from scratch is worth not waiting for.
        if !wait && (state.index.is_none() || state.stale || state.rebuilding) {
            return None;
        }
        let generation = state.generation;
        state.waiters += 1;
        while state.worker_running && state.generation == generation {
            state = self
                .published
                .wait(state)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        state.waiters -= 1;
        // Another project's index is no answer for this one.
        if state.generation != generation {
            return None;
        }
        current(&state).or_else(|| state.index.clone())
    }

    /// [`Self::keep_current`], ending the wait of every search if it panics
    /// (an unreadable file tree-sitter chokes on), so none hangs forever.
    fn keep_current_or_give_up(&self, generation: u64) {
        let worked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self.keep_current(generation)
        }));
        if worked.is_err() {
            let mut state = self.lock();
            if state.generation == generation {
                state.worker_running = false;
                state.rebuilding = false;
            }
            self.published.notify_all();
        }
    }

    /// The worker: build or refresh the index without the lock held, then
    /// publish it, until nothing is left to apply.
    fn keep_current(&self, generation: u64) {
        let mut working: Option<CoversIndex> = None;
        loop {
            let mut state = self.lock();
            if state.generation != generation {
                return;
            }
            let rebuild = state.stale || (working.is_none() && state.index.is_none());
            state.rebuilding = rebuild;
            if rebuild {
                state.stale = false;
                state.pending.clear();
                let (project, roots) = (state.project.clone(), state.roots.clone());
                drop(state);
                working = Some(CoversIndex::build(&project, &roots));
                continue;
            }
            let pending = std::mem::take(&mut state.pending);
            let mut index = working
                .take()
                .unwrap_or_else(|| state.index.as_deref().cloned().unwrap_or_default());
            if pending.is_empty() {
                state.index = Some(Arc::new(index));
                state.worker_running = false;
                self.published.notify_all();
                return;
            }
            drop(state);
            for path in pending {
                index.refresh(&path);
            }
            working = Some(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(project: &Path, file: &str, text: &str) {
        let path = project.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    fn roots() -> Vec<Psr4Root> {
        vec![
            Psr4Root {
                prefix: "App\\".into(),
                dir: "src/".into(),
                dev: false,
            },
            Psr4Root {
                prefix: "Tests\\".into(),
                dir: "tests/".into(),
                dev: true,
            },
        ]
    }

    const COVERS_BAR: &str =
        "<?php\nnamespace Tests;\nuse App\\Bar;\n#[CoversClass(Bar::class)]\nfinal class CheckoutTest {}\n";

    #[test]
    fn the_index_records_what_each_test_file_covers() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        write(
            project,
            "tests/Unit/BarTest.php",
            "<?php\n/** @covers \\App\\Bar */\nit('x', fn () => 1);\n",
        );
        write(
            project,
            "tests/Unit/OtherTest.php",
            "<?php\nit('y', fn () => 1);\n",
        );
        write(project, "tests/Support/Helper.php", COVERS_BAR);
        write(project, "tests/vendor/x/FooTest.php", COVERS_BAR);

        let index = CoversIndex::build(project, &roots());

        assert_eq!(
            index.covering("App\\Bar"),
            ["tests/CheckoutTest.php", "tests/Unit/BarTest.php"]
        );
        assert!(index.covering("App\\Baz").is_empty());
        assert_eq!(index.named("OtherTest.php"), ["tests/Unit/OtherTest.php"]);
        assert!(index.named("Helper.php").is_empty());
    }

    #[test]
    fn a_refresh_rereads_adds_and_drops_files_and_directories() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        let mut index = CoversIndex::build(project, &roots());

        // Edited to cover nothing.
        write(
            project,
            "tests/CheckoutTest.php",
            "<?php\nfinal class CheckoutTest {}\n",
        );
        index.refresh(&project.join("tests/CheckoutTest.php"));
        assert!(index.covering("App\\Bar").is_empty());

        // A new directory with a test in it.
        write(project, "tests/Api/OrderTest.php", COVERS_BAR);
        index.refresh(&project.join("tests/Api"));
        assert_eq!(index.covering("App\\Bar"), ["tests/Api/OrderTest.php"]);

        // The directory removed.
        std::fs::remove_dir_all(project.join("tests/Api")).unwrap();
        index.refresh(&project.join("tests/Api"));
        assert!(index.named("OrderTest.php").is_empty());

        // Outside the test directories, or a dependency: ignored.
        write(project, "src/BarTest.php", COVERS_BAR);
        index.refresh(&project.join("src/BarTest.php"));
        write(project, "tests/vendor/BazTest.php", COVERS_BAR);
        index.refresh(&project.join("tests/vendor/BazTest.php"));
        assert!(index.covering("App\\Bar").is_empty());
    }

    #[test]
    fn the_cache_builds_once_and_applies_reported_changes() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        let cache = Arc::new(CoversCache::default());

        let index = cache.index(project, &roots(), true).unwrap();
        assert_eq!(index.covering("App\\Bar"), ["tests/CheckoutTest.php"]);

        write(project, "tests/Unit/BarTest.php", COVERS_BAR);
        cache.file_changed(&project.join("tests/Unit/BarTest.php"));
        // Not a PHP file, outside the tests: never queued.
        cache.file_changed(&project.join("tests/notes.md"));
        cache.file_changed(&project.join("src/Bar.php"));
        assert_eq!(cache.lock().pending.len(), 1);

        // A refresh is waited for even without `wait`.
        let index = cache.index(project, &roots(), false).unwrap();
        assert_eq!(
            index.covering("App\\Bar"),
            ["tests/CheckoutTest.php", "tests/Unit/BarTest.php"]
        );
        let again = cache.index(project, &roots(), false).unwrap();
        assert!(
            Arc::ptr_eq(&index, &again),
            "nothing changed, nothing rebuilt"
        );
    }

    #[test]
    fn a_first_build_answers_none_unless_waited_for() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        let cache = Arc::new(CoversCache::default());

        // Either the build is still running, or (on a fast machine) the
        // worker already published: never a half-built index.
        if let Some(index) = cache.index(project, &roots(), false) {
            assert_eq!(index.covering("App\\Bar"), ["tests/CheckoutTest.php"]);
        }
        let index = cache.index(project, &roots(), true).unwrap();
        assert_eq!(index.covering("App\\Bar"), ["tests/CheckoutTest.php"]);
    }

    #[test]
    fn new_roots_join_the_index_too_many_changes_rebuild_and_another_project_starts_over() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        write(project, "spec/CartTest.php", COVERS_BAR);
        let cache = Arc::new(CoversCache::default());
        cache.index(project, &roots(), true).unwrap();

        let spec = vec![Psr4Root {
            prefix: "Spec\\".into(),
            dir: "spec/".into(),
            dev: true,
        }];
        // The roots only grow: a second package's roots join the first's.
        let index = cache.index(project, &spec, true).unwrap();
        assert_eq!(
            index.covering("App\\Bar"),
            ["spec/CartTest.php", "tests/CheckoutTest.php"]
        );
        // Asking with the first roots again rebuilds nothing.
        cache.index(project, &roots(), false).unwrap();

        for n in 0..=MAX_PENDING_CHANGES {
            cache.file_changed(&project.join(format!("spec/F{n}Test.php")));
        }
        assert!(cache.lock().stale);
        assert!(cache.lock().pending.is_empty());
        let index = cache.index(project, &spec, true).unwrap();
        assert_eq!(
            index.covering("App\\Bar"),
            ["spec/CartTest.php", "tests/CheckoutTest.php"]
        );

        let other = tempfile::tempdir().unwrap();
        write(other.path(), "tests/OtherTest.php", COVERS_BAR);
        let index = cache.index(other.path(), &roots(), true).unwrap();
        assert_eq!(index.covering("App\\Bar"), ["tests/OtherTest.php"]);
    }

    fn spec_roots() -> Vec<Psr4Root> {
        vec![Psr4Root {
            prefix: "Spec\\".into(),
            dir: "spec/".into(),
            dev: true,
        }]
    }

    #[test]
    fn roots_that_change_during_the_first_build_rebuild_the_index() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        write(project, "spec/CartTest.php", COVERS_BAR);
        let cache = Arc::new(CoversCache::default());
        // A first build with the old roots is under way.
        let generation = {
            let mut state = cache.lock();
            state.project = project.to_path_buf();
            state.roots = roots();
            state.worker_running = true;
            state.pending.push(project.join("tests/CheckoutTest.php"));
            state.generation
        };

        assert!(cache.index(project, &spec_roots(), false).is_none());
        assert!(
            cache.lock().stale,
            "the old roots' build is never published"
        );
        assert_eq!(cache.lock().pending.len(), 1, "queued changes are kept");

        cache.keep_current(generation);
        let index = cache.index(project, &spec_roots(), true).unwrap();
        assert_eq!(
            index.covering("App\\Bar"),
            ["spec/CartTest.php", "tests/CheckoutTest.php"]
        );
    }

    #[test]
    fn a_search_waiting_on_one_project_never_gets_the_next_ones_index() {
        let first = tempfile::tempdir().unwrap();
        let second = tempfile::tempdir().unwrap();
        write(first.path(), "tests/CheckoutTest.php", COVERS_BAR);
        write(second.path(), "tests/OtherTest.php", COVERS_BAR);
        let cache = Arc::new(CoversCache::default());
        {
            // A build of the first project that only a project switch ends.
            let mut state = cache.lock();
            state.project = first.path().to_path_buf();
            state.roots = roots();
            state.worker_running = true;
        }
        let waiter = {
            let cache = Arc::clone(&cache);
            let project = first.path().to_path_buf();
            std::thread::spawn(move || cache.index(&project, &roots(), true))
        };
        // Switch projects only once the search is blocked on the build.
        while cache.lock().waiters == 0 {
            std::thread::yield_now();
        }
        let _ = cache.index(second.path(), &roots(), true);

        let answer = waiter.join().unwrap();
        assert!(
            answer.is_none(),
            "the first project's search got the second project's index"
        );
    }

    #[test]
    fn a_modified_ancestor_rescans_nothing_and_a_new_or_removed_one_does() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        let package_roots = vec![Psr4Root {
            prefix: "Pkg\\".into(),
            dir: "packages/a/tests/".into(),
            dev: true,
        }];
        let all: Vec<Psr4Root> = roots().into_iter().chain(package_roots).collect();
        let mut index = CoversIndex::build(project, &all);

        // A test written without a report of its own: a report of the
        // project root (a parent-modified event) must not walk for it.
        write(project, "tests/UnreportedTest.php", COVERS_BAR);
        index.refresh(project);
        assert_eq!(index.covering("App\\Bar"), ["tests/CheckoutTest.php"]);

        // A package created with its tests: its directory is scanned.
        write(project, "packages/a/tests/NewTest.php", COVERS_BAR);
        index.refresh(&project.join("packages"));
        assert!(index
            .covering("App\\Bar")
            .contains(&"packages/a/tests/NewTest.php".to_string()));

        // Removed: its tests go with it.
        std::fs::remove_dir_all(project.join("packages")).unwrap();
        index.refresh(&project.join("packages"));
        assert_eq!(index.covering("App\\Bar"), ["tests/CheckoutTest.php"]);
    }

    #[test]
    fn a_refresh_never_grows_the_index_past_the_scan_budget() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        for n in 0..phpunit_xml::MAX_SCANNED_FILES {
            write(project, &format!("tests/a/F{n}Test.php"), "<?php");
        }
        let mut index = CoversIndex::build(project, &roots());
        assert_eq!(index.files.len(), phpunit_xml::MAX_SCANNED_FILES);

        write(project, "tests/b/NewTest.php", COVERS_BAR);
        index.refresh(&project.join("tests/b"));
        index.refresh(&project.join("tests/b/NewTest.php"));
        assert_eq!(index.files.len(), phpunit_xml::MAX_SCANNED_FILES);
        assert!(index.covering("App\\Bar").is_empty());
    }

    #[test]
    fn a_directory_report_reads_only_the_files_not_indexed_yet() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        let mut index = CoversIndex::build(project, &roots());
        // Changed, but its own report has not arrived.
        write(project, "tests/CheckoutTest.php", "<?php\n");
        write(project, "tests/Unit/BarTest.php", COVERS_BAR);
        index.refresh(&project.join("tests"));
        assert_eq!(
            index.covering("App\\Bar"),
            ["tests/CheckoutTest.php", "tests/Unit/BarTest.php"]
        );
    }

    #[test]
    fn a_removed_or_restored_package_takes_its_tests_along() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "packages/a/tests/BarTest.php", COVERS_BAR);
        let package_roots = vec![Psr4Root {
            prefix: "A\\Tests\\".into(),
            dir: "packages/a/tests/".into(),
            dev: true,
        }];
        let mut index = CoversIndex::build(project, &package_roots);
        assert_eq!(index.named("BarTest.php"), ["packages/a/tests/BarTest.php"]);

        std::fs::rename(project.join("packages"), project.join("moved")).unwrap();
        index.refresh(&project.join("packages"));
        assert!(index.files.is_empty());

        std::fs::rename(project.join("moved"), project.join("packages")).unwrap();
        index.refresh(&project.join("packages"));
        assert_eq!(index.named("BarTest.php"), ["packages/a/tests/BarTest.php"]);
    }

    #[test]
    fn a_file_deleted_before_its_report_is_not_listed() {
        let dir = tempfile::tempdir().unwrap();
        let project = dir.path();
        write(project, "tests/CheckoutTest.php", COVERS_BAR);
        let index = CoversIndex::build(project, &roots());
        std::fs::remove_file(project.join("tests/CheckoutTest.php")).unwrap();
        assert!(index.covering("App\\Bar").is_empty());
        assert!(index.named("CheckoutTest.php").is_empty());
    }
}
