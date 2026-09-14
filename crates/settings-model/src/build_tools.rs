//! Settings > Build Tools (the jvm-build-tools plan's A7, ADR-0057): the
//! draft the page edits, and validation for the fields a free-form string
//! or path cannot self-check.
//!
//! Shaped after [`crate::analysis`]: persistence stays in `app-config`
//! (`BuildToolsSettings`), and this module owns the typed vocabulary
//! (`AutoReload`, `GradleDistribution`) both `jvm-build-core` and the
//! settings page read the same id strings through. `trusted_roots` is
//! deliberately **not** a `ScopedField` — ADR-0057 §3 requires it to live
//! only in the global settings file, so this module never resolves it
//! against a project layer the way `scope::resolve` does for every other
//! section.

use std::path::PathBuf;

use app_config::{BuildToolsSettings, GradleToolSettings, MavenToolSettings, Settings};

/// The three reload policies (ADR-0057 §5, IntelliJ's own vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AutoReload {
    External,
    Any,
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

    pub fn label(self) -> &'static str {
        match self {
            AutoReload::External => "External changes",
            AutoReload::Any => "Any changes",
            AutoReload::None => "Never (show a banner)",
        }
    }
}

/// Where Gradle's own binary comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradleDistribution {
    Wrapper,
    GradleHome,
}

impl GradleDistribution {
    pub const DEFAULT: GradleDistribution = GradleDistribution::Wrapper;

    pub fn id(self) -> &'static str {
        match self {
            GradleDistribution::Wrapper => "wrapper",
            GradleDistribution::GradleHome => "gradle-home",
        }
    }

    pub fn from_id(id: &str) -> Option<Self> {
        match id {
            "wrapper" => Some(GradleDistribution::Wrapper),
            "gradle-home" => Some(GradleDistribution::GradleHome),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GradleDistribution::Wrapper => "Project wrapper",
            GradleDistribution::GradleHome => "Local installation",
        }
    }
}

/// One field the page can flag a problem against.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuildToolsField {
    GradleHome,
    GradleAutoReload,
    MavenHome,
    MavenAutoReload,
    MavenThreads,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildToolsProblem {
    pub field: BuildToolsField,
    pub sentence: String,
}

/// The page's draft, committed on OK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildToolsDraft {
    pub trusted_roots: Vec<PathBuf>,
    pub gradle_distribution: GradleDistribution,
    pub gradle_home: Option<PathBuf>,
    pub gradle_java_home: Option<PathBuf>,
    pub gradle_offline: bool,
    pub gradle_auto_reload: AutoReload,
    pub gradle_download_sources: bool,
    pub gradle_jvm_args: Vec<String>,
    pub maven_home: Option<String>,
    pub maven_user_settings_file: Option<PathBuf>,
    pub maven_local_repository: Option<PathBuf>,
    pub maven_offline: bool,
    pub maven_skip_tests: bool,
    pub maven_threads: Option<String>,
    pub maven_always_update_snapshots: bool,
    pub maven_auto_reload: AutoReload,
}

impl BuildToolsDraft {
    pub fn new(settings: &Settings) -> Self {
        let b = &settings.build_tools;
        Self {
            trusted_roots: b.trusted_roots.clone(),
            gradle_distribution: b
                .gradle
                .distribution
                .as_deref()
                .and_then(GradleDistribution::from_id)
                .unwrap_or(GradleDistribution::DEFAULT),
            gradle_home: b.gradle.gradle_home.clone(),
            gradle_java_home: b.gradle.java_home.clone(),
            gradle_offline: b.gradle.offline.unwrap_or(false),
            gradle_auto_reload: b
                .gradle
                .auto_reload
                .as_deref()
                .and_then(AutoReload::from_id)
                .unwrap_or(AutoReload::DEFAULT),
            gradle_download_sources: b.gradle.download_sources.unwrap_or(false),
            gradle_jvm_args: b.gradle.jvm_args.clone(),
            maven_home: b.maven.maven_home.clone(),
            maven_user_settings_file: b.maven.user_settings_file.clone(),
            maven_local_repository: b.maven.local_repository.clone(),
            maven_offline: b.maven.offline.unwrap_or(false),
            maven_skip_tests: b.maven.skip_tests.unwrap_or(false),
            maven_threads: b.maven.threads.clone(),
            maven_always_update_snapshots: b.maven.always_update_snapshots.unwrap_or(false),
            maven_auto_reload: b
                .maven
                .auto_reload
                .as_deref()
                .and_then(AutoReload::from_id)
                .unwrap_or(AutoReload::DEFAULT),
        }
    }

    /// Trust a root the user just clicked "Load" for (ADR-0057 §3). A
    /// no-op if it is already trusted — trust is a set, not a log.
    pub fn trust(&mut self, root: PathBuf) {
        if !self.trusted_roots.contains(&root) {
            self.trusted_roots.push(root);
        }
    }

    pub fn revoke_trust(&mut self, root: &std::path::Path) {
        self.trusted_roots.retain(|r| r != root);
    }

    /// Everything wrong with the draft. `threads` accepts `mvn -T`'s own
    /// spelling (a bare integer, or an integer followed by `C` for
    /// "per core"), so this is the one place that shape is checked rather
    /// than trusted verbatim into an argv.
    pub fn validate(&self) -> Vec<BuildToolsProblem> {
        let mut problems = Vec::new();
        if let Some(path) = &self.gradle_home {
            if self.gradle_distribution == GradleDistribution::GradleHome && as_str(path).is_empty()
            {
                problems.push(BuildToolsProblem {
                    field: BuildToolsField::GradleHome,
                    sentence: "a local Gradle installation is selected, but no directory is set"
                        .to_string(),
                });
            }
        } else if self.gradle_distribution == GradleDistribution::GradleHome {
            problems.push(BuildToolsProblem {
                field: BuildToolsField::GradleHome,
                sentence: "a local Gradle installation is selected, but no directory is set"
                    .to_string(),
            });
        }
        if let Some(threads) = &self.maven_threads {
            if !is_valid_thread_count(threads) {
                problems.push(BuildToolsProblem {
                    field: BuildToolsField::MavenThreads,
                    sentence: format!(
                        "`{threads}` is not a thread count `mvn -T` accepts — use a plain \
                         number (`4`), a number of threads per core (`1C`, `1.5C`), or `C` \
                         alone for one thread per core"
                    ),
                });
            }
        }
        problems
    }

    /// The Gradle sub-table this draft would write — its own function so a
    /// project-scope commit (`BuildToolsProjectSettings`, no
    /// `trusted_roots` field to speak of) can reuse it without going
    /// through [`Self::apply_to`]'s whole-`Settings` shape.
    pub fn to_gradle_settings(&self) -> GradleToolSettings {
        GradleToolSettings {
            distribution: (self.gradle_distribution != GradleDistribution::DEFAULT)
                .then(|| self.gradle_distribution.id().to_string()),
            gradle_home: self.gradle_home.clone(),
            java_home: self.gradle_java_home.clone(),
            offline: self.gradle_offline.then_some(true),
            auto_reload: (self.gradle_auto_reload != AutoReload::DEFAULT)
                .then(|| self.gradle_auto_reload.id().to_string()),
            download_sources: self.gradle_download_sources.then_some(true),
            jvm_args: self.gradle_jvm_args.clone(),
        }
    }

    /// The Maven sub-table this draft would write — see
    /// [`Self::to_gradle_settings`]'s own doc comment.
    pub fn to_maven_settings(&self) -> MavenToolSettings {
        MavenToolSettings {
            maven_home: self.maven_home.clone(),
            user_settings_file: self.maven_user_settings_file.clone(),
            local_repository: self.maven_local_repository.clone(),
            offline: self.maven_offline.then_some(true),
            skip_tests: self.maven_skip_tests.then_some(true),
            threads: self.maven_threads.clone(),
            always_update_snapshots: self.maven_always_update_snapshots.then_some(true),
            auto_reload: (self.maven_auto_reload != AutoReload::DEFAULT)
                .then(|| self.maven_auto_reload.id().to_string()),
        }
    }

    pub fn apply_to(&self, settings: &mut Settings) {
        settings.build_tools = BuildToolsSettings {
            trusted_roots: self.trusted_roots.clone(),
            gradle: self.to_gradle_settings(),
            maven: self.to_maven_settings(),
        };
    }
}

fn as_str(path: &std::path::Path) -> &str {
    path.to_str().unwrap_or("")
}

/// `mvn -T`'s own grammar: a plain integer thread count (`4`), a
/// (possibly decimal) multiplier of the machine's core count followed by
/// `C` (`1C`, `1.5C`), or `C` alone for one thread per core.
fn is_valid_thread_count(value: &str) -> bool {
    match value.strip_suffix(['C', 'c']) {
        Some("") => true,
        Some(multiplier) => is_decimal_number(multiplier),
        None => !value.is_empty() && value.chars().all(|c| c.is_ascii_digit()),
    }
}

fn is_decimal_number(value: &str) -> bool {
    if value.is_empty() || value.starts_with('.') || value.ends_with('.') {
        return false;
    }
    let mut seen_dot = false;
    value.chars().all(|c| {
        if c == '.' {
            let first_dot = !seen_dot;
            seen_dot = true;
            first_dot
        } else {
            c.is_ascii_digit()
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_untouched_draft_has_every_default_and_no_problems() {
        let draft = BuildToolsDraft::new(&Settings::default());
        assert!(draft.trusted_roots.is_empty());
        assert_eq!(draft.gradle_distribution, GradleDistribution::DEFAULT);
        assert_eq!(draft.gradle_auto_reload, AutoReload::DEFAULT);
        assert_eq!(draft.maven_auto_reload, AutoReload::DEFAULT);
        assert!(draft.validate().is_empty());
    }

    #[test]
    fn trusting_a_root_is_idempotent() {
        let mut draft = BuildToolsDraft::new(&Settings::default());
        let root = PathBuf::from("/home/u/project");
        draft.trust(root.clone());
        draft.trust(root.clone());
        assert_eq!(draft.trusted_roots, vec![root]);
    }

    #[test]
    fn revoking_trust_removes_only_that_root() {
        let mut draft = BuildToolsDraft::new(&Settings::default());
        draft.trust(PathBuf::from("/a"));
        draft.trust(PathBuf::from("/b"));
        draft.revoke_trust(std::path::Path::new("/a"));
        assert_eq!(draft.trusted_roots, vec![PathBuf::from("/b")]);
    }

    #[test]
    fn apply_to_writes_a_sparse_section_when_nothing_changed() {
        let draft = BuildToolsDraft::new(&Settings::default());
        let mut settings = Settings::default();
        draft.apply_to(&mut settings);
        assert_eq!(settings.build_tools, BuildToolsSettings::default());
    }

    #[test]
    fn a_saved_section_round_trips_into_the_draft_and_back() {
        let mut settings = Settings::default();
        settings.build_tools.trusted_roots.push(PathBuf::from("/p"));
        settings.build_tools.gradle.offline = Some(true);
        settings.build_tools.gradle.auto_reload = Some("any".to_string());
        settings.build_tools.maven.skip_tests = Some(true);
        settings.build_tools.maven.threads = Some("1C".to_string());

        let draft = BuildToolsDraft::new(&settings);
        assert_eq!(draft.trusted_roots, vec![PathBuf::from("/p")]);
        assert!(draft.gradle_offline);
        assert_eq!(draft.gradle_auto_reload, AutoReload::Any);
        assert!(draft.maven_skip_tests);
        assert_eq!(draft.maven_threads.as_deref(), Some("1C"));

        let mut round_tripped = Settings::default();
        draft.apply_to(&mut round_tripped);
        assert_eq!(round_tripped.build_tools, settings.build_tools);
    }

    #[test]
    fn a_local_gradle_distribution_with_no_home_directory_is_a_problem() {
        let mut draft = BuildToolsDraft::new(&Settings::default());
        draft.gradle_distribution = GradleDistribution::GradleHome;
        let problems = draft.validate();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, BuildToolsField::GradleHome);
    }

    #[test]
    fn an_invalid_thread_count_is_a_problem() {
        for bad in ["", "x", "4x", "1.5", "1.C", ".5C", "1.5.5C"] {
            let mut draft = BuildToolsDraft::new(&Settings::default());
            draft.maven_threads = Some(bad.to_string());
            let problems = draft.validate();
            assert_eq!(problems.len(), 1, "`{bad}` should have been rejected");
            assert_eq!(problems[0].field, BuildToolsField::MavenThreads);
        }
    }

    #[test]
    fn valid_thread_counts_are_accepted() {
        // mvn -T's own grammar: a plain integer, C alone (one thread per
        // core), or a (possibly decimal) per-core multiplier followed by C.
        for good in ["4", "8", "C", "1C", "1.5C", "0.5C"] {
            let mut draft = BuildToolsDraft::new(&Settings::default());
            draft.maven_threads = Some(good.to_string());
            assert!(
                draft.validate().is_empty(),
                "`{good}` should have been accepted"
            );
        }
    }

    #[test]
    fn auto_reload_ids_round_trip() {
        for mode in [AutoReload::External, AutoReload::Any, AutoReload::None] {
            assert_eq!(AutoReload::from_id(mode.id()), Some(mode));
        }
        assert_eq!(AutoReload::from_id("bogus"), None);
    }

    #[test]
    fn gradle_distribution_ids_round_trip() {
        for dist in [GradleDistribution::Wrapper, GradleDistribution::GradleHome] {
            assert_eq!(GradleDistribution::from_id(dist.id()), Some(dist));
        }
        assert_eq!(GradleDistribution::from_id("bogus"), None);
    }
}
