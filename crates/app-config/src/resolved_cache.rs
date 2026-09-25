//! Process-wide cache of `settings.toml` / `<project>/.ide/settings.toml`
//! reads, shared by every consumer that resolves a project's settings.
//!
//! Plan doc `fast-project-open-plan.md` step 5: `ProjectTreeModel::projectOpened`
//! fans out into roughly seven to nine slots (search, LSP, build tools,
//! analysis, ...), each of which used to call `settings_model::scope::resolve`
//! itself — a fresh parse of both files, every time, even though the answer
//! is identical for all of them until something changes. This module holds
//! the one shared answer instead.
//!
//! `app-config` may not depend on `settings-model` (ADR-0017: layering.md
//! puts `settings-model` above this crate, since it joins a persisted value
//! to the vocabularies `syntax-core`/`lsp-core` own), so this cache does not
//! compute the resolved settings itself — the caller supplies a loader
//! closure, the same shape `std::sync::OnceLock`/`HashMap::entry` already
//! use in the standard library for "compute once, reuse after". What this
//! module owns is the one rule every consumer must share: a cached answer
//! is only ever as good as the last write. Every hit re-stats the files it
//! was built from (one `stat` each, microseconds, against a TOML parse) and
//! misses if either changed, so an edit from outside this process — a hand
//! edit, a `git checkout`, a second IDE instance saving its recents — is
//! seen on the next read with no watcher involved. [`crate::save`]/
//! [`crate::update`] and [`crate::project_settings::save`]/
//! [`crate::project_settings::update`] also call [`invalidate`], for a
//! filesystem whose mtime is too coarse to tell two quick writes apart.
//!
//! One entry for the global layer, one for the currently open project's
//! layer plus its resolved combination — never a map keyed by every root
//! ever opened, since ADR-0037/ADR-0062 already establish that only one
//! project is ever open at a time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::SystemTime;

use crate::project_settings::{ProjectSettings, PROJECT_DIR, PROJECT_SETTINGS_FILE};
use crate::{Settings, SETTINGS_FILE};

/// What a settings file looked like on disk when a cached answer was built
/// from it: modification time and length, or `None` for a missing or
/// unreadable file — "absent" is a state too, and creating the file must
/// miss the cache just like editing it.
type FileStamp = Option<(SystemTime, u64)>;

fn stamp(path: &Path) -> FileStamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

fn global_stamp(config_dir: &Path) -> FileStamp {
    stamp(&config_dir.join(SETTINGS_FILE))
}

fn project_stamps(config_dir: &Path, root: &Path) -> [FileStamp; 2] {
    [
        global_stamp(config_dir),
        stamp(&root.join(PROJECT_DIR).join(PROJECT_SETTINGS_FILE)),
    ]
}

/// How many times a loader actually ran (a cache miss) since process start —
/// a measurement hook for `fast_project_open_timing.rs`'s E2E probe (plan
/// step 5), not read by any production code path. The E2E harness runs the
/// app as a separate process, so it cannot read this counter directly; it
/// greps `app.stderr` for the line [`record_miss`] prints instead, gated the
/// same way every other test-only signal in this codebase is (free when
/// `IDE_E2E_EVENTS` is unset, per `e2e_mark.h`'s convention).
static MISS_COUNT: AtomicUsize = AtomicUsize::new(0);

/// Cumulative miss count, for unit tests that want to assert a load actually
/// happened without depending on stderr.
pub fn miss_count() -> usize {
    MISS_COUNT.load(Ordering::Relaxed)
}

fn record_miss(file: &str) {
    MISS_COUNT.fetch_add(1, Ordering::Relaxed);
    if std::env::var_os("IDE_E2E_EVENTS").is_some() {
        eprintln!("ide-settings-parse {file}");
    }
}

struct GlobalEntry {
    config_dir: PathBuf,
    stamp: FileStamp,
    settings: Settings,
}

struct ProjectEntry {
    config_dir: PathBuf,
    root: PathBuf,
    stamps: [FileStamp; 2],
    project_settings: ProjectSettings,
    resolved: Settings,
}

/// The cache itself. Production code only ever uses the one process-wide
/// instance behind the free functions below; tests build their own, so a
/// parallel test's `invalidate()` cannot turn one test's hit into a miss.
#[derive(Default)]
struct Cache {
    global: Mutex<Option<GlobalEntry>>,
    project: Mutex<Option<ProjectEntry>>,
}

impl Cache {
    fn global_settings(&self, config_dir: &Path, load: impl FnOnce() -> Settings) -> Settings {
        let stamp = global_stamp(config_dir);
        let mut guard = self.global.lock().unwrap();
        if let Some(entry) = guard
            .as_ref()
            .filter(|entry| entry.config_dir == config_dir && entry.stamp == stamp)
        {
            return entry.settings.clone();
        }
        record_miss("settings.toml");
        let settings = load();
        *guard = Some(GlobalEntry {
            config_dir: config_dir.to_path_buf(),
            stamp,
            settings: settings.clone(),
        });
        settings
    }

    fn project_settings_and_resolved(
        &self,
        config_dir: &Path,
        root: &Path,
        load: impl FnOnce() -> (ProjectSettings, Settings),
    ) -> (ProjectSettings, Settings) {
        let stamps = project_stamps(config_dir, root);
        let mut guard = self.project.lock().unwrap();
        if let Some(entry) = guard.as_ref().filter(|entry| {
            entry.config_dir == config_dir && entry.root == root && entry.stamps == stamps
        }) {
            return (entry.project_settings.clone(), entry.resolved.clone());
        }
        record_miss(".ide/settings.toml");
        let (project_settings, resolved) = load();
        *guard = Some(ProjectEntry {
            config_dir: config_dir.to_path_buf(),
            root: root.to_path_buf(),
            stamps,
            project_settings: project_settings.clone(),
            resolved: resolved.clone(),
        });
        (project_settings, resolved)
    }

    fn invalidate(&self) {
        *self.global.lock().unwrap() = None;
        *self.project.lock().unwrap() = None;
    }
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(Cache::default)
}

/// The cached global `<config_dir>/settings.toml`, computing it with `load`
/// (and caching the result) the first time, after [`invalidate`], or once
/// the file changed on disk.
pub fn global_settings(config_dir: &Path, load: impl FnOnce() -> Settings) -> Settings {
    cache().global_settings(config_dir, load)
}

/// The cached `(project_settings, resolved_settings)` pair for `root`,
/// computing both with `load` (and caching the result) if nothing is
/// cached yet, if what's cached belongs to a different root, or if either
/// `<config_dir>/settings.toml` or `<root>/.ide/settings.toml` changed on
/// disk since — the resolved answer depends on both.
pub fn project_settings_and_resolved(
    config_dir: &Path,
    root: &Path,
    load: impl FnOnce() -> (ProjectSettings, Settings),
) -> (ProjectSettings, Settings) {
    cache().project_settings_and_resolved(config_dir, root, load)
}

/// Drop everything cached, regardless of root. Call after any write to
/// `settings.toml` or a project's `.ide/settings.toml` so the next reader
/// gets a fresh parse instead of a value that predates the write — the
/// staleness rule this cache must never violate (ADR-0037/ADR-0062).
pub fn invalidate() {
    cache().invalidate();
}

/// Whether a project's scope (ADR-0064) — what the index, the watcher and
/// the project tree consider part of the project — would answer
/// differently between two resolved [`Settings`] snapshots this cache
/// handed out.
///
/// The only two fields that feed `project_model::ProjectScope`: `excluded`
/// (project) and `ignored_names` (global). Every other field a save might
/// touch (theme, fonts, run configs, ...) leaves the scope alone, so the
/// caller — the bridge adapter, after a project- or global-settings save —
/// uses this to decide whether the open project needs a rescope (index
/// delta reopen + watcher restart) at all, rather than paying for one on
/// every unrelated settings save.
pub fn scope_changed(before: &Settings, after: &Settings) -> bool {
    before.excluded != after.excluded || before.ignored_names != after.ignored_names
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn themed(theme: &str) -> Settings {
        Settings {
            theme: theme.to_string(),
            ..Settings::default()
        }
    }

    /// Rewrite `path` so its stamp is guaranteed to differ: a different
    /// length, since two writes inside one mtime tick look identical.
    fn write(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn scope_changed_only_on_excluded_or_ignored_names() {
        let before = Settings {
            excluded: vec!["lib".to_string()],
            ignored_names: vec![".git".to_string()],
            theme: "light".to_string(),
            ..Settings::default()
        };

        assert!(!scope_changed(&before, &before.clone()));

        let mut excluded_changed = before.clone();
        excluded_changed.excluded.push("vendor".to_string());
        assert!(scope_changed(&before, &excluded_changed));

        let mut ignored_names_changed = before.clone();
        ignored_names_changed
            .ignored_names
            .push("*.pyc".to_string());
        assert!(scope_changed(&before, &ignored_names_changed));

        let mut unrelated_changed = before.clone();
        unrelated_changed.theme = "dark".to_string();
        assert!(!scope_changed(&before, &unrelated_changed));
    }

    #[test]
    fn global_settings_computes_once_then_serves_the_cached_value() {
        let cache = Cache::default();
        let config = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            themed("dark")
        };
        let first = cache.global_settings(config.path(), load);
        let second = cache.global_settings(config.path(), load);
        assert_eq!(first, second);
        assert_eq!(calls.get(), 1, "second call must not re-load");
    }

    #[test]
    fn global_settings_reloads_after_an_external_edit() {
        let cache = Cache::default();
        let config = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            Settings::default()
        };
        cache.global_settings(config.path(), load);
        write(&config.path().join(SETTINGS_FILE), "theme = \"light\"\n");
        cache.global_settings(config.path(), load);
        assert_eq!(
            calls.get(),
            2,
            "a file created or edited behind the cache's back must miss"
        );
    }

    #[test]
    fn project_settings_recomputes_for_a_different_root() {
        let cache = Cache::default();
        let config = tempfile::tempdir().unwrap();
        let root_a = tempfile::tempdir().unwrap();
        let root_b = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let load = |theme: &'static str| {
            let calls = &calls;
            move || {
                calls.set(calls.get() + 1);
                (ProjectSettings::default(), themed(theme))
            }
        };

        let (_, resolved_a) =
            cache.project_settings_and_resolved(config.path(), root_a.path(), load("a"));
        let (_, resolved_a_again) =
            cache.project_settings_and_resolved(config.path(), root_a.path(), load("x"));
        assert_eq!(resolved_a, resolved_a_again);
        assert_eq!(calls.get(), 1, "same root must hit the cache");

        let (_, resolved_b) =
            cache.project_settings_and_resolved(config.path(), root_b.path(), load("b"));
        assert_eq!(calls.get(), 2, "a different root must miss the cache");
        assert_ne!(resolved_a, resolved_b);
    }

    #[test]
    fn project_settings_reload_after_either_file_changes() {
        let cache = Cache::default();
        let config = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            (ProjectSettings::default(), Settings::default())
        };
        cache.project_settings_and_resolved(config.path(), root.path(), load);

        write(
            &root.path().join(PROJECT_DIR).join(PROJECT_SETTINGS_FILE),
            "x = 1\n",
        );
        cache.project_settings_and_resolved(config.path(), root.path(), load);
        assert_eq!(calls.get(), 2, "project layer edit must miss");

        write(&config.path().join(SETTINGS_FILE), "theme = \"light\"\n");
        cache.project_settings_and_resolved(config.path(), root.path(), load);
        assert_eq!(calls.get(), 3, "global layer edit must miss");

        cache.project_settings_and_resolved(config.path(), root.path(), load);
        assert_eq!(calls.get(), 3, "unchanged files must hit");
    }

    #[test]
    fn invalidate_forces_the_next_call_to_recompute() {
        let cache = Cache::default();
        let config = tempfile::tempdir().unwrap();
        let root = tempfile::tempdir().unwrap();
        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            (ProjectSettings::default(), Settings::default())
        };
        let global_load = || {
            calls.set(calls.get() + 1);
            Settings::default()
        };
        cache.project_settings_and_resolved(config.path(), root.path(), load);
        cache.global_settings(config.path(), global_load);
        assert_eq!(calls.get(), 2);

        cache.invalidate();
        cache.project_settings_and_resolved(config.path(), root.path(), load);
        cache.global_settings(config.path(), global_load);
        assert_eq!(calls.get(), 4, "invalidate must force a fresh load of both");
    }

    #[test]
    fn global_settings_are_cached_per_config_dir() {
        let cache = Cache::default();
        let config_a = tempfile::tempdir().unwrap();
        let config_b = tempfile::tempdir().unwrap();
        cache.global_settings(config_a.path(), || themed("a"));
        let from_b = cache.global_settings(config_b.path(), || themed("b"));
        assert_eq!(
            from_b.theme, "b",
            "two missing files must not share an entry"
        );
    }
}
