//! The Tests dock's start-up rules (PR #434 review, H1/M2/L10): a run is
//! prepared on a worker (its package, configuration and framework program,
//! which may mean a probe inside WSL or a container) and starts once that
//! answers. What a start means for the tree, which runs keep their
//! configuration, and what becomes of a preparation that Stop or a project
//! switch overtook are decided here, so the adapter only acts on the answer.

use std::path::Path;

use crate::scope::TestScope;

/// What asked for a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunKind {
    /// Run All: the framework's own arguments.
    AdHoc,
    /// A run configuration from the toolbar, the list or a before-launch task.
    Configuration,
    /// A gutter, caret or project tree run: a temporary configuration.
    Editor,
    /// A Tests dock rerun of the configuration that produced its tests.
    Rerun,
}

impl RunKind {
    /// Whether the run's configuration, when temporary, joins the
    /// configuration list: an editor run's and a chosen configuration's do,
    /// as in PhpStorm; a rerun's already is there, Run All has none.
    pub fn remembers_configuration(self) -> bool {
        matches!(self, Self::Configuration | Self::Editor)
    }
}

/// Whether a run replaces the tree an earlier run left: a whole run (Run
/// All, a configuration-file or directory scope) does; a narrowed rerun and
/// a class, method or file run update only the nodes they report, so the
/// rest of the earlier results stay visible.
pub fn replaces_tree(is_rerun: bool, scope: Option<&TestScope>) -> bool {
    !is_rerun
        && scope.is_none_or(|scope| {
            matches!(
                scope,
                TestScope::Config { .. } | TestScope::Directory { .. }
            )
        })
}

/// What becomes of a run once its preparation answers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prepared {
    /// Start it.
    Start,
    /// Stop ended it while it was being prepared, or it is not the run the
    /// service waits for: drop it, Stop has already said so.
    Dropped,
    /// Another project was opened meanwhile: refuse it, so it never resets
    /// the new project's tree nor writes into the old project's settings.
    ProjectChanged,
}

/// `run`, prepared for the project at `prepared_root`, answered; the
/// service waits for `preparing` (`None` once Stop ended it) and the
/// project at `current_root` is open.
pub fn prepared(
    run: u64,
    preparing: Option<u64>,
    prepared_root: &Path,
    current_root: Option<&Path>,
) -> Prepared {
    if preparing != Some(run) {
        Prepared::Dropped
    } else if current_root != Some(prepared_root) {
        Prepared::ProjectChanged
    } else {
        Prepared::Start
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_editor_and_configuration_runs_remember_their_configuration() {
        assert!(RunKind::Editor.remembers_configuration());
        assert!(RunKind::Configuration.remembers_configuration());
        assert!(!RunKind::Rerun.remembers_configuration());
        assert!(!RunKind::AdHoc.remembers_configuration());
    }

    #[test]
    fn whole_runs_replace_the_tree_and_narrowed_ones_update_it() {
        let directory = TestScope::Directory { path: ".".into() };
        let config = TestScope::Config { testsuite: None };
        let class = TestScope::Class {
            class: "A".into(),
            path: None,
        };
        assert!(replaces_tree(false, None));
        assert!(replaces_tree(false, Some(&directory)));
        assert!(replaces_tree(false, Some(&config)));
        assert!(!replaces_tree(false, Some(&class)));
        assert!(!replaces_tree(true, None));
        assert!(!replaces_tree(true, Some(&config)));
    }

    #[test]
    fn a_preparation_starts_only_while_waited_for_in_the_same_project() {
        let a = Path::new("/a");
        let b = Path::new("/b");
        assert_eq!(prepared(2, Some(2), a, Some(a)), Prepared::Start);
        // Stop ended it, or a newer start replaced it.
        assert_eq!(prepared(2, None, a, Some(a)), Prepared::Dropped);
        assert_eq!(prepared(2, Some(3), a, Some(a)), Prepared::Dropped);
        assert_eq!(prepared(2, Some(2), a, Some(b)), Prepared::ProjectChanged);
        assert_eq!(prepared(2, Some(2), a, None), Prepared::ProjectChanged);
    }
}
