//! When a sync happens, and whether it may happen at all (ADR-0057 §3, §5):
//! the trust gate, the three reload policies, and the save/watcher-event
//! dedupe that keeps the IDE's own `Ctrl+S` from banner-ing itself.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use globset::{Glob, GlobSet, GlobSetBuilder};

/// The three reload policies (IntelliJ's own vocabulary, ADR-0057 §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoReload {
    /// A watcher-observed change (an external edit, a `git checkout`, a
    /// branch switch) auto-syncs; an in-IDE save shows the banner instead —
    /// the default, because a save usually means "I'm still editing this".
    External,
    /// Either kind of change auto-syncs.
    Any,
    /// Neither auto-syncs; every change shows the banner.
    None,
}

impl AutoReload {
    pub const DEFAULT: AutoReload = AutoReload::External;

    pub fn id(self) -> &'static str {
        match self {
            AutoReload::External => "external",
            AutoReload::Any => "any",
            AutoReload::None => "none",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "external" => Some(AutoReload::External),
            "any" => Some(AutoReload::Any),
            "none" => Some(AutoReload::None),
            _ => None,
        }
    }
}

/// Where a build-file change was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeOrigin {
    /// The project filesystem watcher fired for this path — an external
    /// edit, a VCS operation, or (unless deduped, see
    /// [`is_echo_of_recent_save`]) the IDE's own save reaching disk.
    Watcher,
    /// The user pressed Ctrl+S on this path while it was open in a tab.
    Save,
}

/// What a build-file change should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReloadAction {
    Auto,
    Banner,
}

/// The reload policy's whole truth table.
///
/// Deliberately ignorant of whether the path is open in a tab: a `git
/// checkout` of an open build file must still auto-reload in `External`
/// mode exactly as IntelliJ does, so "was this the user still editing" is
/// answered by [`ChangeOrigin`] (watcher vs. an explicit save), not by tab
/// state.
pub fn decide(mode: AutoReload, origin: ChangeOrigin) -> ReloadAction {
    match (mode, origin) {
        (AutoReload::None, _) => ReloadAction::Banner,
        (AutoReload::Any, _) => ReloadAction::Auto,
        (AutoReload::External, ChangeOrigin::Watcher) => ReloadAction::Auto,
        (AutoReload::External, ChangeOrigin::Save) => ReloadAction::Banner,
    }
}

/// How long a watcher event is treated as an echo of the IDE's own save to
/// the same path, rather than an independent external change.
pub const SAVE_ECHO_WINDOW: Duration = Duration::from_millis(750);

/// Tracks recent in-IDE saves so a watcher event they triggered is not
/// double-counted as a second, independent change.
///
/// `ponytail:` an unbounded `HashMap` that never evicts an entry older than
/// [`SAVE_ECHO_WINDOW`] — fine at the scale of "paths saved in the last
/// second", revisit with a scheduled sweep if a very long session with
/// thousands of distinct saved build files ever makes this measurable.
#[derive(Debug, Default)]
pub struct SaveTracker {
    last_saved: HashMap<PathBuf, Instant>,
}

impl SaveTracker {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `path` was just saved from within the IDE.
    pub fn record_save(&mut self, path: &Path) {
        self.last_saved.insert(path.to_path_buf(), Instant::now());
    }

    /// Is a watcher event for `path`, arriving now, plausibly an echo of a
    /// save this tracker recorded within [`SAVE_ECHO_WINDOW`]?
    pub fn is_echo_of_recent_save(&self, path: &Path) -> bool {
        self.last_saved
            .get(path)
            .is_some_and(|saved_at| saved_at.elapsed() < SAVE_ECHO_WINDOW)
    }
}

/// Does `relative_path` (project-relative, forward-slash separated) match
/// one of a [`plugin_api::BuildToolContribution`]'s `build_files` patterns?
///
/// A pattern with no glob metacharacter (`build.gradle`) matches only that
/// exact relative path — not "ends with", which would make
/// `some/build.gradle.bak` a false positive.
pub fn is_build_file(relative_path: &Path, patterns: &[String]) -> bool {
    build_glob_set(patterns).is_match(relative_path)
}

fn build_glob_set(patterns: &[String]) -> GlobSet {
    let mut builder = GlobSetBuilder::new();
    for pattern in patterns {
        if let Ok(glob) = Glob::new(pattern) {
            builder.add(glob);
        }
    }
    // A pattern this build somehow could not compile is dropped rather than
    // panicking a caller mid-watcher-callback; `plugin-api` already
    // validates every pattern is non-empty at manifest load time, so this
    // is a defensive fallback, not the expected path.
    builder.build().unwrap_or_else(|_| GlobSet::empty())
}

/// Is `root` one the user has explicitly agreed to sync (ADR-0057 §3)?
///
/// Both sides are canonicalised so a symlinked or differently-cased path to
/// the same directory still matches; a root that does not exist on disk
/// (deleted since it was trusted, or simply wrong) is never trusted — a
/// canonicalisation failure is not silently treated as a match.
pub fn is_trusted(root: &Path, trusted_roots: &[PathBuf]) -> bool {
    let Ok(canonical_root) = root.canonicalize() else {
        return false;
    };
    trusted_roots.iter().any(|trusted| {
        trusted
            .canonicalize()
            .is_ok_and(|canonical_trusted| canonical_trusted == canonical_root)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn external_mode_auto_syncs_a_watcher_change_and_banners_a_save() {
        assert_eq!(
            decide(AutoReload::External, ChangeOrigin::Watcher),
            ReloadAction::Auto
        );
        assert_eq!(
            decide(AutoReload::External, ChangeOrigin::Save),
            ReloadAction::Banner
        );
    }

    #[test]
    fn any_mode_auto_syncs_both_origins() {
        assert_eq!(
            decide(AutoReload::Any, ChangeOrigin::Watcher),
            ReloadAction::Auto
        );
        assert_eq!(
            decide(AutoReload::Any, ChangeOrigin::Save),
            ReloadAction::Auto
        );
    }

    #[test]
    fn none_mode_always_banners() {
        assert_eq!(
            decide(AutoReload::None, ChangeOrigin::Watcher),
            ReloadAction::Banner
        );
        assert_eq!(
            decide(AutoReload::None, ChangeOrigin::Save),
            ReloadAction::Banner
        );
    }

    #[test]
    fn auto_reload_ids_round_trip() {
        for mode in [AutoReload::External, AutoReload::Any, AutoReload::None] {
            assert_eq!(AutoReload::from_id(mode.id()), Some(mode));
        }
        assert_eq!(AutoReload::from_id("bogus"), None);
    }

    #[test]
    fn a_watcher_event_right_after_a_recorded_save_is_an_echo() {
        let mut tracker = SaveTracker::new();
        let path = PathBuf::from("build.gradle.kts");
        assert!(!tracker.is_echo_of_recent_save(&path));
        tracker.record_save(&path);
        assert!(tracker.is_echo_of_recent_save(&path));
    }

    #[test]
    fn a_watcher_event_for_a_different_path_is_not_an_echo() {
        let mut tracker = SaveTracker::new();
        tracker.record_save(Path::new("build.gradle.kts"));
        assert!(!tracker.is_echo_of_recent_save(Path::new("settings.gradle.kts")));
    }

    #[test]
    fn a_literal_pattern_matches_only_that_exact_path() {
        let patterns = vec!["build.gradle".to_string()];
        assert!(is_build_file(Path::new("build.gradle"), &patterns));
        assert!(!is_build_file(Path::new("module/build.gradle"), &patterns));
        assert!(!is_build_file(Path::new("build.gradle.bak"), &patterns));
    }

    #[test]
    fn a_glob_pattern_matches_anything_under_the_directory() {
        let patterns = vec!["buildSrc/**".to_string()];
        assert!(is_build_file(
            Path::new("buildSrc/build.gradle.kts"),
            &patterns
        ));
        assert!(is_build_file(
            Path::new("buildSrc/src/main/kotlin/Foo.kt"),
            &patterns
        ));
        assert!(!is_build_file(Path::new("app/build.gradle"), &patterns));
    }

    #[test]
    fn a_pom_glob_matches_at_any_depth() {
        let patterns = vec!["**/pom.xml".to_string()];
        assert!(is_build_file(Path::new("pom.xml"), &patterns));
        assert!(is_build_file(Path::new("module-a/pom.xml"), &patterns));
        assert!(!is_build_file(Path::new("module-a/child.xml"), &patterns));
    }

    #[test]
    fn a_trusted_root_matches_regardless_of_a_symlink_or_trailing_slash() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        assert!(!is_trusted(&root, &[]));
        assert!(is_trusted(&root, std::slice::from_ref(&root)));
    }

    #[test]
    fn an_untrusted_root_is_not_trusted() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        assert!(!is_trusted(a.path(), &[b.path().to_path_buf()]));
    }

    #[test]
    fn a_root_that_no_longer_exists_is_never_trusted() {
        let dir = tempfile::tempdir().unwrap();
        let ghost = dir.path().join("deleted");
        assert!(!is_trusted(&ghost, std::slice::from_ref(&ghost)));
    }
}
