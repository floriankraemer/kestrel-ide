//! Which settings layer a value comes from, and which layer wins.
//!
//! Two files hold settings: the global `settings.toml` in the config
//! directory, and `<project_root>/.ide/settings.toml`. `app-config` reads and
//! writes both and knows nothing about precedence (ADR-0017). Precedence is a
//! rule, so it lives here (ADR-0022 §3).
//!
//! # What a project may configure
//!
//! A project may configure the project, not you. [`ScopedField`] is the whole
//! list — editing behaviour, language servers, run configurations and index
//! excludes — and it is an enum rather than a convention so that "is this
//! overridable?" is a question the compiler answers. Theme, fonts, keymap and
//! AI providers are deliberately absent: a project that forces your colour
//! scheme on you is hostile.
//!
//! Widening the list later is additive. Narrowing it is a breaking change to
//! a file people have already committed, which is why the line is drawn
//! deliberately rather than "everything that happens to be in both structs".
//!
//! # Absent is not empty
//!
//! Every field of `ProjectSettings` is an `Option`, and the distinction
//! carries all the weight here: `None` means *the project says nothing, ask
//! the global layer*, while `Some(vec![])` means *the project says: none*.
//! A project that explicitly clears the run configurations is overriding the
//! global list, not failing to mention it.

use app_config::project_settings::ProjectSettings;
use app_config::Settings;

/// Where an effective value came from.
///
/// The settings dialog shows this beside a field and never re-derives it
/// (ADR-0022): the origin badge and the value have to agree, and the only
/// way to guarantee that is for one function to answer both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Scope {
    /// The global `settings.toml` sets it, and the project is silent.
    ///
    /// The default, and deliberately so: a surface that has not been told
    /// which layer it is looking at is looking at the person's own, which is
    /// the one that cannot write into a file the whole team shares.
    #[default]
    Global,
    /// The project's `.ide/settings.toml` overrides this field.
    Project,
    /// Neither file says anything; the value is this build's default.
    Default,
}

impl Scope {
    /// The word the dialog puts on the badge.
    pub fn label(self) -> &'static str {
        match self {
            Scope::Project => "from project",
            Scope::Global => "from global",
            Scope::Default => "default",
        }
    }
}

/// A setting a project is allowed to override.
///
/// Coarser than one variant per key on purpose: the project layer overrides
/// whole *areas* — the `[editing]` section, the `[[language_server]]` list —
/// because a half-overridden section is a merge rule nobody can predict from
/// looking at the file, and the file is meant to be read by humans in a code
/// review.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScopedField {
    /// The `[editing]` section: indentation, wrapping and the save rules,
    /// with its per-language tables.
    Editing,
    /// The `[[language_server]]` blocks.
    LanguageServers,
    /// The `[[run_config]]` blocks.
    RunConfigs,
    /// The index's exclude patterns.
    IndexExcludes,
    /// The `[terminal]` section: shell, start directory and environment.
    Terminal,
}

impl ScopedField {
    /// Every field a project may override, in settings-dialog order.
    pub const ALL: [ScopedField; 5] = [
        ScopedField::Editing,
        ScopedField::LanguageServers,
        ScopedField::RunConfigs,
        ScopedField::IndexExcludes,
        ScopedField::Terminal,
    ];

    /// The stable id the view names this field by — the same string the
    /// `AppSettings::fieldOrigin` slot takes, so the C++ side passes through
    /// what a page was built with rather than inventing a vocabulary.
    pub fn id(self) -> &'static str {
        match self {
            ScopedField::Editing => "editing",
            ScopedField::LanguageServers => "languageServers",
            ScopedField::RunConfigs => "runConfigs",
            ScopedField::IndexExcludes => "indexExcludes",
            ScopedField::Terminal => "terminal",
        }
    }

    /// The field with this id, or `None` for anything else — including the
    /// settings that are global by decision, which is the answer that makes
    /// "is this overridable?" a total function.
    pub fn from_id(id: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|field| field.id() == id)
    }
}

/// The settings actually in force: the global layer with the project's
/// overrides applied.
///
/// Per field, not per file. A project that overrides only `[editing]` leaves
/// every other setting showing through from the global layer, which is the
/// same rule `editing::resolve_for_language` applies one level down between
/// a language table and its section.
pub fn resolve(global: &Settings, project: &ProjectSettings) -> Settings {
    let mut resolved = global.clone();
    if let Some(editing) = &project.editing {
        resolved.editing = editing.clone();
    }
    if let Some(servers) = &project.language_servers {
        resolved.language_servers = servers.clone();
    }
    if let Some(excludes) = &project.index_excludes {
        resolved.index_excludes = excludes.clone();
    }
    if let Some(terminal) = &project.terminal {
        resolved.terminal = terminal.clone();
    }
    // Run configurations are deliberately *not* folded in: they have no
    // counterpart in the global layer at all (ADR-0029 — a run configuration
    // is the definition of a project, never a preference), so there is
    // nothing for them to override. `origin` still reports on them, because
    // the dialog labels them like everything else.
    resolved
}

/// Where the effective value of `field` comes from.
///
/// `Project` when the project overrides it, `Global` when the global layer
/// has something to say, `Default` when neither does. "Has something to say"
/// means differing from a fresh [`Settings`] — a global file that spells out
/// the default is indistinguishable from one that omits it, which is exactly
/// the ambiguity the project layer avoids by being sparse.
///
/// This is the *effective* origin — the layer whose value the app actually
/// runs with — not necessarily the layer a particular settings-dialog page
/// is showing. The dialog has two pages per scoped field (one per layer),
/// and the Global page always displays the global layer's own value even
/// when a project override shadows it at runtime; a badge built from this
/// function alone would say "from project" on a page that is visibly
/// showing the global value, which is the exact badge-disagrees-with-the-
/// value bug (#143) this note exists to head off. [`origin_for_view`] is
/// the one to call from the dialog; this one answers a different, real
/// question ("what does the app use") that has no view attached to it.
pub fn origin(field: ScopedField, global: &Settings, project: &ProjectSettings) -> Scope {
    let overridden = match field {
        ScopedField::Editing => project.editing.is_some(),
        ScopedField::LanguageServers => project.language_servers.is_some(),
        ScopedField::RunConfigs => project.run_configs.is_some(),
        ScopedField::IndexExcludes => project.index_excludes.is_some(),
        ScopedField::Terminal => project.terminal.is_some(),
    };
    if overridden {
        return Scope::Project;
    }
    if set_globally(field, global) {
        Scope::Global
    } else {
        Scope::Default
    }
}

/// The origin of the value a settings-dialog page actually has on screen
/// for `field`, given which layer (`viewing`) that page is editing.
///
/// Viewing the Project layer answers exactly what [`origin`] does — the
/// page shows the project's override when there is one, and falls through
/// to the global/default value exactly as the app would at runtime, so the
/// two questions coincide there. Viewing the Global layer is where they
/// diverge: that page shows the global file's own value regardless of any
/// project override elsewhere, so the project layer is not consulted at
/// all — the badge can only ever read "from global" or "default", never
/// "from project", because a project override is never what is on screen
/// there.
pub fn origin_for_view(
    field: ScopedField,
    global: &Settings,
    project: &ProjectSettings,
    viewing: Scope,
) -> Scope {
    if viewing == Scope::Project {
        return origin(field, global, project);
    }
    if set_globally(field, global) {
        Scope::Global
    } else {
        Scope::Default
    }
}

fn set_globally(field: ScopedField, global: &Settings) -> bool {
    let defaults = Settings::default();
    match field {
        ScopedField::Editing => global.editing != defaults.editing,
        ScopedField::LanguageServers => global.language_servers != defaults.language_servers,
        // Run configurations only ever exist in the project layer, so a
        // project that has none is at its default rather than inheriting
        // one.
        ScopedField::RunConfigs => false,
        ScopedField::IndexExcludes => global.index_excludes != defaults.index_excludes,
        ScopedField::Terminal => global.terminal != defaults.terminal,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_config::{EditingSettings, TerminalSettings};

    fn project_editing(tab_width: u32) -> ProjectSettings {
        ProjectSettings {
            editing: Some(EditingSettings {
                tab_width,
                ..EditingSettings::default()
            }),
            ..ProjectSettings::default()
        }
    }

    #[test]
    fn a_project_that_says_nothing_resolves_to_the_global_layer_exactly() {
        let global = Settings {
            index_excludes: vec!["target/".to_string()],
            editing: EditingSettings {
                tab_width: 8,
                ..EditingSettings::default()
            },
            ..Settings::default()
        };

        let resolved = resolve(&global, &ProjectSettings::default());

        assert_eq!(resolved, global);
    }

    #[test]
    fn a_project_overriding_one_area_leaves_the_rest_showing_through() {
        let global = Settings {
            theme: "Dark".to_string(),
            index_excludes: vec!["scratch/".to_string()],
            editing: EditingSettings {
                tab_width: 8,
                ..EditingSettings::default()
            },
            ..Settings::default()
        };

        let resolved = resolve(&global, &project_editing(2));

        assert_eq!(resolved.editing.tab_width, 2, "the project's own answer");
        assert_eq!(resolved.theme, "Dark", "untouched by the project layer");
        assert_eq!(
            resolved.index_excludes,
            vec!["scratch/".to_string()],
            "an area the project did not mention still comes from global"
        );
    }

    /// The case the terminal's shell picker rests on: a checkout whose
    /// tooling only runs under one shell says so in its committed file, and
    /// that beats whatever the person set globally.
    #[test]
    fn a_project_shell_overrides_the_persons_own() {
        let global = Settings {
            terminal: TerminalSettings {
                shell_id: "fish".to_string(),
                ..TerminalSettings::default()
            },
            ..Settings::default()
        };
        let project = ProjectSettings {
            terminal: Some(TerminalSettings {
                shell_id: "wsl:Ubuntu".to_string(),
                ..TerminalSettings::default()
            }),
            ..ProjectSettings::default()
        };

        assert_eq!(resolve(&global, &project).terminal.shell_id, "wsl:Ubuntu");
        assert_eq!(
            origin(ScopedField::Terminal, &global, &project),
            Scope::Project
        );
    }

    #[test]
    fn a_terminal_section_nobody_touched_reports_default_not_global() {
        let global = Settings::default();
        let project = ProjectSettings::default();

        assert_eq!(
            origin(ScopedField::Terminal, &global, &project),
            Scope::Default
        );
        assert_eq!(
            origin(
                ScopedField::Terminal,
                &Settings {
                    terminal: TerminalSettings {
                        shell_id: "zsh".to_string(),
                        ..TerminalSettings::default()
                    },
                    ..Settings::default()
                },
                &project
            ),
            Scope::Global
        );
    }

    #[test]
    fn a_project_may_not_override_a_person_shaped_setting() {
        // The list is the type: there is no `ScopedField::Theme` to pass,
        // and `ProjectSettings` has no field to put one in. This test exists
        // so that adding either without an ADR change fails a review with a
        // failing test rather than a discussion.
        assert!(ScopedField::from_id("theme").is_none());
        assert!(ScopedField::from_id("keymap").is_none());
        assert!(ScopedField::from_id("aiProviders").is_none());
        assert!(ScopedField::from_id("editorFontSize").is_none());
        assert_eq!(ScopedField::ALL.len(), 5, "ADR-0022 names five areas");
    }

    #[test]
    fn an_empty_override_is_not_the_same_as_no_override() {
        let global = Settings {
            index_excludes: vec!["target/".to_string()],
            ..Settings::default()
        };

        let silent = ProjectSettings::default();
        let explicit = ProjectSettings {
            index_excludes: Some(Vec::new()),
            ..ProjectSettings::default()
        };

        assert_eq!(
            resolve(&global, &silent).index_excludes,
            vec!["target/".to_string()],
            "silence inherits"
        );
        assert!(
            resolve(&global, &explicit).index_excludes.is_empty(),
            "an explicit empty list overrides the global one"
        );
    }

    #[test]
    fn the_origin_of_a_value_is_the_layer_that_last_set_it() {
        let global = Settings {
            editing: EditingSettings {
                tab_width: 8,
                ..EditingSettings::default()
            },
            ..Settings::default()
        };

        let project = project_editing(2);

        assert_eq!(
            origin(ScopedField::Editing, &global, &project),
            Scope::Project
        );
        assert_eq!(
            origin(ScopedField::Editing, &global, &ProjectSettings::default()),
            Scope::Global,
            "the global layer set a tab width, so it is not a default"
        );
        assert_eq!(
            origin(
                ScopedField::Editing,
                &Settings::default(),
                &ProjectSettings::default()
            ),
            Scope::Default,
            "neither layer said anything"
        );
    }

    #[test]
    fn run_configurations_are_a_project_answer_or_no_answer_at_all() {
        // They have no global counterpart (ADR-0029), so "the global file
        // set them" is not a state that exists — a badge saying otherwise
        // would send the user looking for a setting that is not there.
        let global = Settings::default();
        assert_eq!(
            origin(
                ScopedField::RunConfigs,
                &global,
                &ProjectSettings::default()
            ),
            Scope::Default
        );
        let project = ProjectSettings {
            run_configs: Some(Vec::new()),
            ..ProjectSettings::default()
        };
        assert_eq!(
            origin(ScopedField::RunConfigs, &global, &project),
            Scope::Project,
            "an explicitly empty list is still the project's answer"
        );
    }

    #[test]
    fn every_field_id_round_trips_and_is_unique() {
        // The ids cross the FFI seam as strings, so a duplicate would make
        // two areas share a badge.
        let mut ids: Vec<&str> = ScopedField::ALL.iter().map(|f| f.id()).collect();
        ids.sort_unstable();
        let mut unique = ids.clone();
        unique.dedup();
        assert_eq!(unique, ids, "two scoped fields share an id");
        for field in ScopedField::ALL {
            assert_eq!(ScopedField::from_id(field.id()), Some(field));
        }
    }

    #[test]
    fn every_scope_has_a_word_the_dialog_can_show() {
        for scope in [Scope::Project, Scope::Global, Scope::Default] {
            assert!(!scope.label().is_empty());
        }
        assert_eq!(Scope::Project.label(), "from project");
    }

    // #143: the Global-scope page shows the global file's own value — never
    // a project override, which lives on a different page entirely — so its
    // badge must never read "from project" even when one exists. Regression
    // test for the bug this ticket found: `field_origin` used to call
    // `origin` (the *effective*, project-first answer) regardless of which
    // page was open, so opening Preferences on a project with an editing
    // override showed "Showing: from project" over the global page's own
    // (unrelated) value.
    #[test]
    fn viewing_global_never_reports_a_project_override_that_is_not_on_screen() {
        let global = Settings::default();
        let project = project_editing(2);

        assert_eq!(
            origin_for_view(ScopedField::Editing, &global, &project, Scope::Project),
            Scope::Project,
            "the project page shows its own override"
        );
        assert_eq!(
            origin_for_view(ScopedField::Editing, &global, &project, Scope::Global),
            Scope::Default,
            "the global page shows the global (default) value, not the project's"
        );
    }

    #[test]
    fn viewing_global_still_reports_global_over_default() {
        let global = Settings {
            editing: EditingSettings {
                tab_width: 8,
                ..EditingSettings::default()
            },
            ..Settings::default()
        };

        assert_eq!(
            origin_for_view(
                ScopedField::Editing,
                &global,
                &ProjectSettings::default(),
                Scope::Global
            ),
            Scope::Global
        );
    }

    #[test]
    fn viewing_project_with_no_override_falls_through_exactly_like_the_effective_origin() {
        let global = Settings {
            editing: EditingSettings {
                tab_width: 8,
                ..EditingSettings::default()
            },
            ..Settings::default()
        };
        let project = ProjectSettings::default();

        assert_eq!(
            origin_for_view(ScopedField::Editing, &global, &project, Scope::Project),
            origin(ScopedField::Editing, &global, &project),
            "with no override the project page shows exactly what origin() already says"
        );
    }
}
