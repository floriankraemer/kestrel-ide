//! `notify`-based filesystem watcher (Task 8, mvp-implementation-plan.md
//! §2). One instance watches one project root on a background thread,
//! directory by directory rather than one blanket recursive watch (see
//! [`ProjectWatcher::start`]); dropping it stops watching. No Qt
//! dependency — the callback is a plain `Fn(PathBuf)`, so `ui-shell`
//! supplies a closure that queues work onto the Qt thread via `cxx-qt`'s
//! `CxxQtThread`. This crate never touches Qt or knows what the callback
//! does with the path.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{Event, PollWatcher, RecursiveMode};

// Re-exported so downstream crates (`app-core`, `ui-shell`) can name the
// event kind in watcher callbacks without depending on `notify` themselves —
// the watcher backend stays this crate's implementation detail.
pub use notify::EventKind;

/// Whether a filesystem-watcher event actually shifts the project tree's
/// shape (a file/folder created, removed, or renamed) as opposed to a
/// content-only change to a path that's already in the tree (a plain write,
/// e.g. every `Ctrl+S` save — same rows, same structure, just different
/// bytes on disk). Only the former needs the tree model rebuilt and reset;
/// resetting for the latter is exactly what caused the sidebar to
/// re-collapse on every save (`beginResetModel`/`endResetModel` discards
/// Qt's per-item expand state for the whole tree).
pub fn is_structural_change(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(_)
            | EventKind::Remove(_)
            | EventKind::Modify(notify::event::ModifyKind::Name(_))
    )
}

/// Whether `path` should be excluded from the watch because it matches the
/// project's own `.gitignore` rules — `target/`, `node_modules/`, and
/// similar build/dependency output.
///
/// A project's own build output routinely runs into the tens of thousands
/// of directories (this repo's own `target/` alone is 10k+), and watching
/// all of it can exhaust the platform's watch budget — inotify's
/// `max_user_watches` on Linux, `ReadDirectoryChangesW`'s fixed event
/// buffer on Windows — after which *every* watch on the project, `.git`
/// included, silently stops delivering events. That is the root cause of
/// the Changes dock going stale under real build activity: nothing told it
/// its watch had been starved out. `.git` is never matched by a project's
/// own `.gitignore` (nothing excludes ignoring it there), so it needs no
/// special-casing to stay watched.
fn is_ignored(matcher: &ignore::gitignore::Gitignore, path: &Path) -> bool {
    matcher.matched(path, true).is_ignore()
}

/// A `.gitignore`-only matcher, built once per watcher and reused for every
/// directory later created under `root` (see the `Create` handling in
/// [`ProjectWatcher::start`]).
///
/// Deliberately narrower than [`ignore::WalkBuilder`]'s full semantics
/// (nested `.gitignore` files elsewhere in the tree, the global git
/// excludes file, `.git/info/exclude`) — the *initial* watch set below is
/// built with the fully correct `WalkBuilder`, which already keeps this
/// repo's `target/` and friends off the watch from the start. This
/// root-`.gitignore`-only matcher only has to cover the much smaller case
/// of a directory created *after* the watcher started; missing an edge
/// case there means, at worst, briefly over-watching one new directory
/// until the project is reopened, not the exhaustion this fix exists to
/// prevent.
fn root_gitignore_matcher(root: &Path) -> ignore::gitignore::Gitignore {
    let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
    let _ = builder.add(root.join(".gitignore"));
    builder
        .build()
        .unwrap_or_else(|_| ignore::gitignore::Gitignore::empty())
}

/// A running watcher for one project root. Kept alive for as long as the
/// project is open; dropping it (e.g. when a different project is opened)
/// stops watching — `notify`'s `Drop` impl tears down the OS-level watches.
pub struct ProjectWatcher {
    // Never read directly outside `start` itself — its only job is to stay
    // alive so the OS-level watches it owns keep running until this is
    // dropped, which also ends `start`'s "register a watch for a new
    // directory" thread (it holds its own clone of this `Arc`, but blocks
    // on a channel `start`'s callback closure owns the sending half of —
    // dropping the closure with this drops the sender, which ends that
    // thread's `recv` loop and its `Arc` clone with it).
    //
    // Boxed rather than the concrete `RecommendedWatcher`: W6-1 (ADR-0052)
    // needs this to hold either backend depending on `root`, and `notify`'s
    // own `Watcher` trait is what both `RecommendedWatcher` and
    // `PollWatcher` implement.
    _watcher: Arc<Mutex<Option<Box<dyn notify::Watcher + Send>>>>,
}

/// `ReadDirectoryChangesW`/`inotify`/`FSEvents` — whichever backend
/// `notify::recommended_watcher` picks — does not fire over a 9P share, so
/// a WSL project root gets silently no events at all from it. `PollWatcher`
/// stats every watched path on an interval instead, which does work over
/// the share. 4 s: frequent enough that a file created inside the distro
/// shows up in the tree without feeling broken, infrequent enough not to
/// mean a `stat()` storm across a project's directory count every tick.
/// `compare_contents: false` — content diffing an unbounded number of files
/// every tick would be the resource cost this ceiling exists to avoid; a
/// modify event still fires from the mtime change alone.
const POLL_INTERVAL: Duration = Duration::from_secs(4);

impl ProjectWatcher {
    /// Start watching `root`. Rather than one blanket
    /// `RecursiveMode::Recursive` watch on `root` (which asks the OS to
    /// watch literally everything below it, `target/` included — see
    /// [`is_ignored`]), this walks `root` once with `ignore::WalkBuilder`
    /// (the same gitignore-aware traversal `index-core` already uses for
    /// its own search index, kept non-hidden here since `.git` and other
    /// dotdirs must stay watched) and registers one non-recursive watch per
    /// directory found. A later `Create` of a new directory is checked
    /// against a lightweight root-`.gitignore` matcher and, if not ignored,
    /// gets its own watch added dynamically — so the watch set stays
    /// current without ever descending into a freshly created `target/`.
    ///
    /// `on_change` is invoked (on `notify`'s background thread, not the
    /// caller's thread) once per changed path reported by an event, along
    /// with the event's `EventKind` — callers need this to tell a
    /// structural change (file created/removed/renamed, which shifts the
    /// tree) apart from a content-only write to a file that already exists
    /// (which doesn't, and notably is what every `Ctrl+S` save looks like).
    /// Callers needing to touch Qt objects must marshal onto the Qt thread
    /// themselves (`ui-shell`'s job, not this crate's).
    ///
    /// `is_remote` (W6-1, ADR-0052) switches the backend to
    /// [`PollWatcher`] — see [`POLL_INTERVAL`]'s doc comment for why.
    /// Answered by the caller rather than computed here:
    /// `process_exec::host::ExecHost` lives in the support layer, and this
    /// crate is domain (`docs/architecture/layering.md`), so `ui-shell`
    /// (already past that boundary, ADR-0052's other seams) does the
    /// classification and hands back a plain bool.
    pub fn start(
        root: &Path,
        is_remote: bool,
        on_change: impl Fn(EventKind, PathBuf) + Send + 'static,
    ) -> notify::Result<Self> {
        let slot: Arc<Mutex<Option<Box<dyn notify::Watcher + Send>>>> = Arc::new(Mutex::new(None));
        let incremental_matcher = root_gitignore_matcher(root);

        // A newly created directory can't be handed to `Watcher::watch`
        // from inside this closure itself: this closure runs *on* the
        // backend's own event thread, and (at least on the `inotify`
        // backend) `watch` blocks sending a command back to that same
        // thread — calling it here deadlocks the watcher permanently after
        // the very first directory it tries to add, with no error and no
        // further events ever delivered again. A plain channel decouples
        // "notice a new directory" from "register a watch for it", with a
        // separate thread doing the latter.
        let (new_dirs_tx, new_dirs_rx) = std::sync::mpsc::channel::<PathBuf>();

        let handler = move |res: notify::Result<Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, EventKind::Create(_)) {
                for path in &event.paths {
                    if path.is_dir() && !is_ignored(&incremental_matcher, path) {
                        let _ = new_dirs_tx.send(path.clone());
                    }
                }
            }
            for path in event.paths {
                on_change(event.kind, path);
            }
        };
        // W6-1 (ADR-0052): a WSL root gets the polling backend, since the
        // OS-native one never fires over the 9P share (see `POLL_INTERVAL`).
        let mut watcher: Box<dyn notify::Watcher + Send> = if is_remote {
            Box::new(PollWatcher::new(
                handler,
                notify::Config::default()
                    .with_poll_interval(POLL_INTERVAL)
                    .with_compare_contents(false),
            )?)
        } else {
            Box::new(notify::recommended_watcher(handler)?)
        };

        // `WalkBuilder` itself already skips descending into an ignored
        // directory (`target/` never gets yielded at all), so every
        // directory it does yield is one to watch — no separate ignore
        // check needed here, unlike the single-path check the `new_dirs_rx`
        // loop below needs.
        let mut walker = ignore::WalkBuilder::new(root);
        walker
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true);
        for entry in walker.build().filter_map(Result::ok) {
            let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
            if is_dir {
                // One bad directory (permission denied, removed mid-walk)
                // must not stop the rest of the project from being watched.
                let _ = watcher.watch(entry.path(), RecursiveMode::NonRecursive);
            }
        }

        *slot.lock().expect("watcher slot poisoned") = Some(watcher);

        // Runs for as long as `new_dirs_tx` (owned by the closure above,
        // which the stored `RecommendedWatcher` outlives) is alive; once
        // `ProjectWatcher` is dropped, the watcher and its closure drop
        // with it, `recv` returns `Err`, and this thread ends on its own —
        // no join handle to keep, same as the callback closure itself
        // needing no explicit teardown.
        let watch_slot = Arc::clone(&slot);
        std::thread::spawn(move || {
            for new_dir in new_dirs_rx {
                if let Ok(mut guard) = watch_slot.lock() {
                    if let Some(watcher) = guard.as_mut() {
                        let _ = watcher.watch(&new_dir, RecursiveMode::NonRecursive);
                    }
                }
            }
        });

        Ok(Self { _watcher: slot })
    }
}

#[cfg(test)]
mod is_structural_change_tests {
    use super::is_structural_change;
    use notify::event::{CreateKind, ModifyKind, RemoveKind, RenameMode};
    use notify::EventKind;

    #[test]
    fn create_and_remove_are_structural() {
        assert!(is_structural_change(&EventKind::Create(CreateKind::File)));
        assert!(is_structural_change(&EventKind::Remove(RemoveKind::File)));
    }

    #[test]
    fn a_rename_is_structural() {
        assert!(is_structural_change(&EventKind::Modify(ModifyKind::Name(
            RenameMode::Both
        ))));
    }

    #[test]
    fn a_plain_content_write_is_not_structural() {
        // What `fs::write` on an already-existing file (every save)
        // reports under Linux's inotify backend.
        assert!(!is_structural_change(&EventKind::Modify(ModifyKind::Data(
            notify::event::DataChange::Any
        ))));
    }

    #[test]
    fn metadata_only_changes_are_not_structural() {
        assert!(!is_structural_change(&EventKind::Modify(
            ModifyKind::Metadata(notify::event::MetadataKind::Any)
        )));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::mpsc;
    use std::time::Duration;

    // `notify`'s OS-level event delivery is inherently timing-dependent, so
    // this polls with a short timeout rather than asserting on a fixed
    // sleep — tolerant of the raciness, per the task's own guidance.
    #[test]
    fn detects_a_new_file_under_the_watched_root() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel::<PathBuf>();

        let _watcher = ProjectWatcher::start(dir.path(), false, move |_kind, path| {
            let _ = tx.send(path);
        })
        .unwrap();

        let new_file = dir.path().join("new.txt");
        fs::write(&new_file, "hello").unwrap();

        let mut saw_it = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while std::time::Instant::now() < deadline {
            if let Ok(path) = rx.recv_timeout(Duration::from_millis(200)) {
                if path == new_file {
                    saw_it = true;
                    break;
                }
            }
        }
        assert!(saw_it, "expected a watcher event for the new file");
    }

    // W6-1: `is_remote: true` selects `PollWatcher`, which still has to
    // notice a new file — over a real (if generous, since the poll interval
    // is 4s) deadline rather than the OS-native backend's near-instant one.
    #[test]
    fn a_remote_watcher_still_detects_a_new_file_by_polling() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel::<PathBuf>();

        let _watcher = ProjectWatcher::start(dir.path(), true, move |_kind, path| {
            let _ = tx.send(path);
        })
        .unwrap();

        let new_file = dir.path().join("new.txt");
        fs::write(&new_file, "hello").unwrap();

        let mut saw_it = false;
        let deadline = std::time::Instant::now() + Duration::from_secs(12);
        while std::time::Instant::now() < deadline {
            if let Ok(path) = rx.recv_timeout(Duration::from_millis(200)) {
                if path == new_file {
                    saw_it = true;
                    break;
                }
            }
        }
        assert!(saw_it, "expected the poll watcher to notice the new file");
    }

    // Regression test for the bug where saving a file (Ctrl+S) collapsed
    // the whole sidebar tree: the watcher callback carries `EventKind`
    // precisely so a caller can tell a content-only write to an
    // already-existing file (what every save looks like) apart from a
    // structural change (create/remove/rename) that actually shifts the
    // tree. This asserts the watcher reports a non-structural `EventKind`
    // for a plain rewrite of an existing file's contents.
    #[test]
    fn rewriting_an_existing_files_contents_is_not_reported_as_a_structural_change() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("existing.txt");
        fs::write(&file, "original").unwrap();

        let (tx, rx) = mpsc::channel::<EventKind>();
        let _watcher = ProjectWatcher::start(dir.path(), false, move |kind, path| {
            if path == file {
                let _ = tx.send(kind);
            }
        })
        .unwrap();

        let watched_file = dir.path().join("existing.txt");
        fs::write(&watched_file, "changed content, same path").unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut saw_structural = false;
        let mut saw_any = false;
        while std::time::Instant::now() < deadline {
            if let Ok(kind) = rx.recv_timeout(Duration::from_millis(200)) {
                saw_any = true;
                let structural = matches!(
                    kind,
                    EventKind::Create(_)
                        | EventKind::Remove(_)
                        | EventKind::Modify(notify::event::ModifyKind::Name(_))
                );
                if structural {
                    saw_structural = true;
                }
            }
        }
        assert!(
            saw_any,
            "expected at least one watcher event for the rewrite"
        );
        assert!(
            !saw_structural,
            "a content-only rewrite of an existing file must not be reported as structural"
        );
    }

    // Polls for a specific path with a generous deadline, same tolerance
    // the tests above take for `notify`'s inherently timing-dependent OS
    // event delivery.
    fn wait_for_path(rx: &mpsc::Receiver<PathBuf>, target: &Path, timeout: Duration) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        while std::time::Instant::now() < deadline {
            if let Ok(path) = rx.recv_timeout(Duration::from_millis(200)) {
                if path == target {
                    return true;
                }
            }
        }
        false
    }

    // Regression test for the root cause of the Changes dock going stale:
    // a blanket recursive watch on the whole project root, `target/`
    // included, could exhaust the platform's watch budget. A directory a
    // `.gitignore` excludes must not get a watch registered for it at all.
    #[test]
    fn a_gitignored_directory_is_not_watched() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "ignored/\n").unwrap();
        fs::create_dir(dir.path().join("ignored")).unwrap();
        fs::create_dir(dir.path().join("tracked")).unwrap();

        let (tx, rx) = mpsc::channel::<PathBuf>();
        let _watcher = ProjectWatcher::start(dir.path(), false, move |_kind, path| {
            let _ = tx.send(path);
        })
        .unwrap();

        let ignored_file = dir.path().join("ignored").join("new.txt");
        fs::write(&ignored_file, "hello").unwrap();
        let tracked_file = dir.path().join("tracked").join("new.txt");
        fs::write(&tracked_file, "hello").unwrap();

        // The positive control has to fire first (or at all) to prove the
        // watcher itself is working — otherwise a silent failure to watch
        // *anything* would also "pass" the negative assertion below.
        assert!(
            wait_for_path(&rx, &tracked_file, Duration::from_secs(5)),
            "expected a watcher event for the non-ignored directory"
        );
        assert!(
            !wait_for_path(&rx, &ignored_file, Duration::from_millis(500)),
            "a gitignored directory must not be watched"
        );
    }

    // `.git` is never matched by a project's own `.gitignore`, but this
    // pins down that the watcher doesn't need special-case code to keep
    // watching it — the Changes dock's external-change detection depends
    // on seeing writes under `.git` (see `editor_tabs_vcs.cpp`).
    #[test]
    fn a_dot_git_directory_is_watched_like_any_other() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join(".git")).unwrap();

        let (tx, rx) = mpsc::channel::<PathBuf>();
        let _watcher = ProjectWatcher::start(dir.path(), false, move |_kind, path| {
            let _ = tx.send(path);
        })
        .unwrap();

        let head_file = dir.path().join(".git").join("HEAD");
        fs::write(&head_file, "ref: refs/heads/main").unwrap();

        assert!(
            wait_for_path(&rx, &head_file, Duration::from_secs(5)),
            "expected a watcher event for a write under .git"
        );
    }

    // The initial watch set is built once at `start`; a directory created
    // afterwards (a fresh `mkdir`, the first `cargo build` populating a
    // `target/` that didn't exist yet) has to pick up its own watch
    // dynamically rather than going unnoticed for the rest of the session.
    #[test]
    fn a_newly_created_non_ignored_directory_is_watched_dynamically() {
        let dir = tempfile::tempdir().unwrap();

        let (tx, rx) = mpsc::channel::<PathBuf>();
        let _watcher = ProjectWatcher::start(dir.path(), false, move |_kind, path| {
            let _ = tx.send(path);
        })
        .unwrap();

        let new_dir = dir.path().join("created-after-start");
        fs::create_dir(&new_dir).unwrap();
        // Give the `Create` handler a moment to register the new watch
        // before the write below, same generous tolerance the other tests
        // give `notify`'s OS-level delivery — bind-mounted filesystems
        // (this crate's own tests run inside a container over one) can be
        // markedly slower to deliver a freshly-added watch's first event
        // than a plain local disk.
        std::thread::sleep(Duration::from_secs(1));
        let nested_file = new_dir.join("new.txt");
        fs::write(&nested_file, "hello").unwrap();

        assert!(
            wait_for_path(&rx, &nested_file, Duration::from_secs(10)),
            "expected a watcher event for a file created under a directory added after start"
        );
    }
}
