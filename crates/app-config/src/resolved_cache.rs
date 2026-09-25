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
//! is only ever as good as the last write, so [`invalidate`] must be called
//! by every path that writes either file — [`crate::save`]/[`crate::update`]
//! and [`crate::project_settings::save`]/[`crate::project_settings::update`]
//! all do, and so does an external edit the caller detects (a filesystem
//! watcher event on `.ide/settings.toml`).
//!
//! One entry for the global layer, one for the currently open project's
//! layer plus its resolved combination — never a map keyed by every root
//! ever opened, since ADR-0037/ADR-0062 already establish that only one
//! project is ever open at a time.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::project_settings::ProjectSettings;
use crate::Settings;

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

struct ProjectEntry {
    root: PathBuf,
    project_settings: ProjectSettings,
    resolved: Settings,
}

struct Cache {
    global: Mutex<Option<Settings>>,
    project: Mutex<Option<ProjectEntry>>,
}

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Cache {
        global: Mutex::new(None),
        project: Mutex::new(None),
    })
}

/// The cached global `settings.toml`, computing it with `load` (and caching
/// the result) the first time, or after [`invalidate`].
pub fn global_settings(load: impl FnOnce() -> Settings) -> Settings {
    let mut guard = cache().global.lock().unwrap();
    if let Some(settings) = guard.as_ref() {
        return settings.clone();
    }
    record_miss("settings.toml");
    let settings = load();
    *guard = Some(settings.clone());
    settings
}

/// The cached `(project_settings, resolved_settings)` pair for `root`,
/// computing both with `load` (and caching the result) if nothing is
/// cached yet, or if what's cached belongs to a different root — opening a
/// different project is not "stale", it's simply a cache miss for its own
/// root, the same way [`invalidate`] leaves the *next* call a miss too.
pub fn project_settings_and_resolved(
    root: &Path,
    load: impl FnOnce() -> (ProjectSettings, Settings),
) -> (ProjectSettings, Settings) {
    let mut guard = cache().project.lock().unwrap();
    if let Some(entry) = guard.as_ref() {
        if entry.root == root {
            return (entry.project_settings.clone(), entry.resolved.clone());
        }
    }
    record_miss(".ide/settings.toml");
    let (project_settings, resolved) = load();
    *guard = Some(ProjectEntry {
        root: root.to_path_buf(),
        project_settings: project_settings.clone(),
        resolved: resolved.clone(),
    });
    (project_settings, resolved)
}

/// Drop everything cached, regardless of root. Call after any write to
/// `settings.toml` or a project's `.ide/settings.toml` so the next reader
/// gets a fresh parse instead of a value that predates the write — the
/// staleness rule this cache must never violate (ADR-0037/ADR-0062).
pub fn invalidate() {
    *cache().global.lock().unwrap() = None;
    *cache().project.lock().unwrap() = None;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::path::Path;

    #[test]
    fn global_settings_computes_once_then_serves_the_cached_value() {
        invalidate();
        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            Settings {
                theme: "dark".to_string(),
                ..Settings::default()
            }
        };
        let first = global_settings(load);
        let second = global_settings(|| {
            calls.set(calls.get() + 1);
            Settings::default()
        });
        assert_eq!(first, second);
        assert_eq!(calls.get(), 1, "second call must not re-load");
    }

    #[test]
    fn project_settings_recomputes_for_a_different_root() {
        invalidate();
        let root_a = Path::new("/projects/a");
        let root_b = Path::new("/projects/b");
        let calls = Cell::new(0);

        let (_, resolved_a) = project_settings_and_resolved(root_a, || {
            calls.set(calls.get() + 1);
            (
                ProjectSettings::default(),
                Settings {
                    theme: "a".to_string(),
                    ..Settings::default()
                },
            )
        });
        let (_, resolved_a_again) = project_settings_and_resolved(root_a, || {
            calls.set(calls.get() + 1);
            (ProjectSettings::default(), Settings::default())
        });
        assert_eq!(resolved_a, resolved_a_again);
        assert_eq!(calls.get(), 1, "same root must hit the cache");

        let (_, resolved_b) = project_settings_and_resolved(root_b, || {
            calls.set(calls.get() + 1);
            (
                ProjectSettings::default(),
                Settings {
                    theme: "b".to_string(),
                    ..Settings::default()
                },
            )
        });
        assert_eq!(calls.get(), 2, "a different root must miss the cache");
        assert_ne!(resolved_a, resolved_b);
    }

    #[test]
    fn invalidate_forces_the_next_call_to_recompute() {
        invalidate();
        let root = Path::new("/projects/c");
        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            (ProjectSettings::default(), Settings::default())
        };
        project_settings_and_resolved(root, load);
        project_settings_and_resolved(root, load);
        assert_eq!(calls.get(), 1);

        invalidate();
        project_settings_and_resolved(root, load);
        assert_eq!(calls.get(), 2, "invalidate must force a fresh load");
    }

    #[test]
    fn invalidate_clears_the_global_cache_too() {
        invalidate();
        let calls = Cell::new(0);
        let load = || {
            calls.set(calls.get() + 1);
            Settings::default()
        };
        global_settings(load);
        invalidate();
        global_settings(load);
        assert_eq!(calls.get(), 2);
    }
}
