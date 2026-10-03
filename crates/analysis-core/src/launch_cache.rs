//! The resolved launch of each analyzer, remembered between runs.
//!
//! Per-file runs fire on every debounced edit and finding a program can
//! mean a `wsl.exe` or container round trip, so the answer is cached — a
//! miss too. A cached answer goes stale in three ways, each handled here:
//! the project's Composer files change (`composer require --dev phpstan`
//! makes a cached miss wrong), a run reports the program gone, or a run
//! cannot reach the host at all (a stopped container).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::scheduler::RunFailure;

/// The project files whose change can add or remove an analyzer.
const WATCHED: &[&str] = &["composer.json", "composer.lock"];

type Stamp = Option<(SystemTime, u64)>;

#[derive(Debug)]
pub struct LaunchCache<L> {
    entries: HashMap<String, Option<L>>,
    /// The project the entries belong to, and its Composer files' stamps.
    root: PathBuf,
    stamps: Vec<Stamp>,
}

impl<L> Default for LaunchCache<L> {
    fn default() -> Self {
        Self {
            entries: HashMap::new(),
            root: PathBuf::new(),
            stamps: Vec::new(),
        }
    }
}

impl<L: Clone> LaunchCache<L> {
    /// The cached launch for `analyzer_id` under `variant` (interpreter and
    /// host), resolving with `resolve` on a miss. Drops everything first
    /// when another project is asked about, or this one's Composer files
    /// changed since the last call.
    pub fn get_or_resolve(
        &mut self,
        root: &Path,
        analyzer_id: &str,
        variant: &str,
        resolve: impl FnOnce() -> Option<L>,
    ) -> Option<L> {
        let stamps = WATCHED.iter().map(|name| stamp(&root.join(name))).collect();
        if stamps != self.stamps || root != self.root {
            self.entries.clear();
            self.stamps = stamps;
            self.root = root.to_path_buf();
        }
        self.entries
            .entry(key(analyzer_id, variant))
            .or_insert_with(resolve)
            .clone()
    }

    /// A run of `analyzer_id` failed: when it says the program is gone or
    /// the host unreachable, forget what was cached for that analyzer.
    pub fn note_failure(&mut self, analyzer_id: &str, failure: &RunFailure) {
        if matches!(failure, RunFailure::NotFound | RunFailure::Io(_)) {
            let prefix = key(analyzer_id, "");
            self.entries.retain(|k, _| !k.starts_with(&prefix));
        }
    }

    /// Forget every launch (detection is being redone).
    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

fn key(analyzer_id: &str, variant: &str) -> String {
    format!("{analyzer_id}\0{variant}")
}

fn stamp(path: &Path) -> Stamp {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    fn lookup(
        cache: &mut LaunchCache<&'static str>,
        root: &Path,
        id: &str,
        found: Option<&'static str>,
        calls: &Cell<u32>,
    ) -> Option<&'static str> {
        cache.get_or_resolve(root, id, "php", || {
            calls.set(calls.get() + 1);
            found
        })
    }

    #[test]
    fn a_hit_and_a_miss_are_both_remembered() {
        let dir = tempfile::tempdir().unwrap();
        let (mut cache, calls) = (LaunchCache::default(), Cell::new(0));
        assert_eq!(
            lookup(&mut cache, dir.path(), "phpstan", None, &calls),
            None
        );
        assert_eq!(
            lookup(&mut cache, dir.path(), "phpstan", Some("x"), &calls),
            None
        );
        assert_eq!(calls.get(), 1, "the miss was cached");
        assert_eq!(
            lookup(&mut cache, dir.path(), "phpcs", Some("y"), &calls),
            Some("y")
        );
        assert_eq!(calls.get(), 2);
    }

    #[test]
    fn composer_require_dev_invalidates_a_cached_miss() {
        let dir = tempfile::tempdir().unwrap();
        let (mut cache, calls) = (LaunchCache::default(), Cell::new(0));
        assert_eq!(
            lookup(&mut cache, dir.path(), "phpstan", None, &calls),
            None
        );
        // `composer require --dev phpstan/phpstan` rewrites composer.json.
        std::fs::write(dir.path().join("composer.json"), "{\"require-dev\":{}}").unwrap();
        assert_eq!(
            lookup(
                &mut cache,
                dir.path(),
                "phpstan",
                Some("vendor/bin/phpstan"),
                &calls
            ),
            Some("vendor/bin/phpstan")
        );
        // A lock-only change (composer install) counts as well.
        std::fs::write(dir.path().join("composer.lock"), "{}").unwrap();
        assert_eq!(
            lookup(&mut cache, dir.path(), "phpstan", None, &calls),
            None
        );
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn a_vanished_program_or_unreachable_host_drops_that_analyzers_entries_only() {
        let dir = tempfile::tempdir().unwrap();
        for failure in [
            RunFailure::NotFound,
            RunFailure::Io("container stopped".into()),
        ] {
            let (mut cache, calls) = (LaunchCache::default(), Cell::new(0));
            lookup(&mut cache, dir.path(), "phpstan", Some("a"), &calls);
            lookup(&mut cache, dir.path(), "phpcs", Some("b"), &calls);
            cache.note_failure("phpstan", &failure);
            lookup(&mut cache, dir.path(), "phpstan", Some("a"), &calls);
            lookup(&mut cache, dir.path(), "phpcs", Some("b"), &calls);
            assert_eq!(calls.get(), 3, "{failure:?}: only phpstan resolved again");
        }
    }

    #[test]
    fn another_project_starts_from_nothing() {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let (mut cache, calls) = (LaunchCache::default(), Cell::new(0));
        lookup(&mut cache, a.path(), "phpstan", Some("a"), &calls);
        assert_eq!(lookup(&mut cache, b.path(), "phpstan", None, &calls), None);
        assert_eq!(
            calls.get(),
            2,
            "no Composer files in either, still asked again"
        );
    }

    #[test]
    fn a_timeout_is_not_a_reason_to_forget() {
        let dir = tempfile::tempdir().unwrap();
        let (mut cache, calls) = (LaunchCache::default(), Cell::new(0));
        lookup(&mut cache, dir.path(), "phpstan", Some("a"), &calls);
        cache.note_failure("phpstan", &RunFailure::TimedOut);
        lookup(&mut cache, dir.path(), "phpstan", Some("a"), &calls);
        assert_eq!(calls.get(), 1);
    }
}
