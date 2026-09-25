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
//! list — editing behaviour, language servers, run configurations and its
//! own excluded folders — and it is an enum rather than a convention so that
//! "is this overridable?" is a question the compiler answers. Theme, fonts,
//! keymap and AI providers are deliberately absent: a project that forces
//! your colour scheme on you is hostile.
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

use std::collections::BTreeMap;

use app_config::project_settings::ProjectSettings;
use app_config::{Layout, Settings};

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
    /// The project's `excluded` folders (ADR-0064) — project-scoped, no
    /// global counterpart, same reasoning as [`ScopedField::RunConfigs`].
    Excluded,
    /// The global `ignored_names` list (ADR-0064).
    IgnoredNames,
    /// The `[terminal]` section: shell, start directory and environment.
    Terminal,
    /// The `[[analyzer]]` overrides (the PHP tooling plan's B7).
    Analysis,
    /// The `[tab_padding]` section: air around an editor tab's label, per
    /// side.
    TabPadding,
    /// The `[containers]` section: Docker/Podman connections, registries
    /// and the dock filters (ADR-0055).
    Containers,
    /// The `[build_tools]` section's Gradle/Maven sub-tables only
    /// (jvm-build-tools plan, ADR-0057 §3) — `trusted_roots` is not part
    /// of this field at all, since `BuildToolsProjectSettings` has no such
    /// field to override with.
    BuildTools,
    /// The `[database]` section's tuning knobs (`page_size`,
    /// `memory_cap_mib`, …). The `sources` list itself is *not* resolved
    /// through this field — see [`resolve_database_sources`], the same
    /// merge-by-id shape [`resolve_layouts`] uses for named layouts.
    Database,
}

impl ScopedField {
    /// Every field a project may override, in settings-dialog order.
    pub const ALL: [ScopedField; 11] = [
        ScopedField::Editing,
        ScopedField::LanguageServers,
        ScopedField::RunConfigs,
        ScopedField::Excluded,
        ScopedField::IgnoredNames,
        ScopedField::Terminal,
        ScopedField::Analysis,
        ScopedField::TabPadding,
        ScopedField::Containers,
        ScopedField::BuildTools,
        ScopedField::Database,
    ];

    /// The stable id the view names this field by — the same string the
    /// `AppSettings::fieldOrigin` slot takes, so the C++ side passes through
    /// what a page was built with rather than inventing a vocabulary.
    pub fn id(self) -> &'static str {
        match self {
            ScopedField::Editing => "editing",
            ScopedField::LanguageServers => "languageServers",
            ScopedField::RunConfigs => "runConfigs",
            ScopedField::Excluded => "excluded",
            ScopedField::IgnoredNames => "ignoredNames",
            ScopedField::Terminal => "terminal",
            ScopedField::Analysis => "analysis",
            ScopedField::TabPadding => "tabPadding",
            ScopedField::Containers => "containers",
            ScopedField::BuildTools => "buildTools",
            ScopedField::Database => "database",
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
    if let Some(file_associations) = &project.file_associations {
        resolved.file_associations = file_associations.clone();
    }
    if let Some(servers) = &project.language_servers {
        resolved.language_servers = servers.clone();
    }
    // Excluded has no global counterpart at all (ADR-0064, same reasoning
    // as run configurations below): the project's own list, or none if the
    // project has never touched it, is the only answer there is. Unlike run
    // configurations, `Settings::excluded` exists as a resolve-only,
    // never-persisted field, because — unlike run configs — consumers
    // (`project_model::ProjectScope`) need it through the same resolved
    // `Settings` every other scoped field comes back through.
    resolved.excluded = project.excluded.clone().unwrap_or_default();
    if let Some(terminal) = &project.terminal {
        resolved.terminal = terminal.clone();
    }
    if let Some(analyzers) = &project.analysis {
        resolved.analysis.analyzers = analyzers.clone();
    }
    if let Some(tab_padding) = &project.tab_padding {
        resolved.tab_padding = *tab_padding;
    }
    if let Some(containers) = &project.containers {
        resolved.containers = containers.clone();
    }
    if let Some(build_tools) = &project.build_tools {
        // `trusted_roots` is deliberately never touched here: it has no
        // counterpart on `BuildToolsProjectSettings` to read from, so the
        // global layer's own value always survives this overlay (ADR-0057
        // §3) — only the Gradle/Maven sub-tables are the project's to
        // override.
        resolved.build_tools.gradle = build_tools.gradle.clone();
        resolved.build_tools.maven = build_tools.maven.clone();
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
        ScopedField::Excluded => project.excluded.is_some(),
        // The project layer has no `ignored_names` field to override with
        // (ADR-0064: it is a global-only list) — never "from project".
        ScopedField::IgnoredNames => false,
        ScopedField::Terminal => project.terminal.is_some(),
        ScopedField::Analysis => project.analysis.is_some(),
        ScopedField::TabPadding => project.tab_padding.is_some(),
        ScopedField::Containers => project.containers.is_some(),
        ScopedField::BuildTools => project.build_tools.is_some(),
        ScopedField::Database => project.database.is_some(),
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

/// Every layout visible to the user, project entries shadowing global ones
/// of the same name, ordered by name.
///
/// This deliberately sits beside [`ScopedField`] rather than inside it. That
/// enum's contract is that a project overrides whole *areas*, because a
/// half-overridden section is a merge rule nobody can predict from reading
/// the file. Layouts are not a section — they are a named collection whose
/// entries are each atomic — so a union keyed by name is the rule a reader
/// of the two files would expect, and folding them into `ScopedField` would
/// instead force a project that ships one layout to replace every layout the
/// user has. See ADR-0045.
///
/// The other line in this module's docs — that theme and fonts stay out
/// because a project forcing your colour scheme on you is hostile — does not
/// apply here: a project *offers* layouts by name, and nothing applies one
/// until the user picks it.
pub fn resolve_layouts(
    global: &Settings,
    project: &ProjectSettings,
) -> Vec<(String, Layout, Scope)> {
    let mut resolved: BTreeMap<&str, (&Layout, Scope)> = global
        .layouts
        .iter()
        .map(|(name, layout)| (name.as_str(), (layout, Scope::Global)))
        .collect();
    // `None` is "the project ships no layouts", and so is `Some(empty)` —
    // unlike the sparse sections above, an empty map here cannot mean "the
    // project clears the user's layouts", because a merge by name has no way
    // to express a removal in the first place.
    for (name, layout) in project.layouts.iter().flatten() {
        resolved.insert(name.as_str(), (layout, Scope::Project));
    }
    resolved
        .into_iter()
        .map(|(name, (layout, scope))| (name.to_string(), layout.clone(), scope))
        .collect()
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
        // Excluded has no global field at all — same "never from global"
        // answer as run configurations, for the same reason.
        ScopedField::Excluded => false,
        ScopedField::IgnoredNames => global.ignored_names != defaults.ignored_names,
        ScopedField::Terminal => global.terminal != defaults.terminal,
        ScopedField::Analysis => global.analysis != defaults.analysis,
        ScopedField::TabPadding => global.tab_padding != defaults.tab_padding,
        ScopedField::Containers => global.containers != defaults.containers,
        // Compares only the Gradle/Maven sub-tables, never `trusted_roots`:
        // a global file that has trusted a root but touched no other
        // `[build_tools]` field must not report this field as "set
        // globally" — trusted_roots is not what this field means at all.
        ScopedField::BuildTools => {
            (
                global.build_tools.gradle.clone(),
                global.build_tools.maven.clone(),
            ) != (defaults.build_tools.gradle, defaults.build_tools.maven)
        }
        // The tuning knobs only — `sources` is not this field's concern,
        // same reasoning as `RunConfigs`/layouts: a collection merged by id
        // has no single "set globally" answer.
        ScopedField::Database => {
            (
                global.database.page_size,
                global.database.memory_cap_mib,
                global.database.idle_close_minutes,
                global.database.history_cap,
                global.database.allow_third_party_drivers,
            ) != (
                defaults.database.page_size,
                defaults.database.memory_cap_mib,
                defaults.database.idle_close_minutes,
                defaults.database.history_cap,
                defaults.database.allow_third_party_drivers,
            )
        }
    }
}

/// Every data source visible to the user, project entries shadowing global
/// ones of the same id, ordered by id — the same union-by-key rule
/// [`resolve_layouts`] uses for named layouts (ADR-0045 §2), applied to
/// `[[database.sources]]` instead: a project adding one source must not
/// hide the ones the user already configured for themselves.
pub fn resolve_database_sources(
    global: &Settings,
    project: &ProjectSettings,
) -> Vec<(app_config::database::DataSourceSetting, Scope)> {
    let mut resolved: BTreeMap<&str, (&app_config::database::DataSourceSetting, Scope)> = global
        .database
        .sources
        .iter()
        .map(|source| (source.id.as_str(), (source, Scope::Global)))
        .collect();
    for source in project
        .database
        .iter()
        .flat_map(|database| database.sources.iter())
    {
        resolved.insert(source.id.as_str(), (source, Scope::Project));
    }
    resolved
        .into_values()
        .map(|(source, scope)| (source.clone(), scope))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use app_config::{EditingSettings, TabPaddingSettings, TerminalSettings};

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
            ignored_names: vec!["target".to_string()],
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
            ignored_names: vec!["scratch".to_string()],
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
            resolved.ignored_names,
            vec!["scratch".to_string()],
            "an area the project has no field to override at all still comes from global"
        );
    }

    #[test]
    fn a_project_that_excludes_a_folder_is_reflected_on_the_resolved_settings() {
        let project = ProjectSettings {
            excluded: Some(vec!["build".to_string()]),
            ..ProjectSettings::default()
        };

        let resolved = resolve(&Settings::default(), &project);

        assert_eq!(resolved.excluded, vec!["build".to_string()]);
        assert_eq!(
            origin(ScopedField::Excluded, &Settings::default(), &project),
            Scope::Project
        );
    }

    #[test]
    fn a_silent_project_resolves_to_no_excluded_folders() {
        let resolved = resolve(&Settings::default(), &ProjectSettings::default());
        assert!(resolved.excluded.is_empty());
        assert_eq!(
            origin(
                ScopedField::Excluded,
                &Settings::default(),
                &ProjectSettings::default()
            ),
            Scope::Default,
            "excluded has no global field to fall back to, same as run configurations"
        );
    }

    #[test]
    fn ignored_names_is_never_from_project_since_the_project_layer_has_no_field_for_it() {
        let global = Settings {
            ignored_names: vec!["scratch".to_string()],
            ..Settings::default()
        };
        assert_eq!(
            origin(
                ScopedField::IgnoredNames,
                &global,
                &ProjectSettings::default()
            ),
            Scope::Global
        );
        assert_eq!(
            origin(
                ScopedField::IgnoredNames,
                &Settings::default(),
                &ProjectSettings::default()
            ),
            Scope::Default
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
    fn a_project_tab_padding_override_wins_over_the_persons_own() {
        let global = Settings {
            tab_padding: TabPaddingSettings {
                right: Some(4),
                ..TabPaddingSettings::default()
            },
            ..Settings::default()
        };
        let project = ProjectSettings {
            tab_padding: Some(TabPaddingSettings {
                right: Some(30),
                ..TabPaddingSettings::default()
            }),
            ..ProjectSettings::default()
        };

        assert_eq!(
            resolve(&global, &project).tab_padding.right_or_default(),
            30
        );
        assert_eq!(
            origin(ScopedField::TabPadding, &global, &project),
            Scope::Project
        );
    }

    #[test]
    fn a_tab_padding_section_nobody_touched_reports_default_not_global() {
        let global = Settings::default();
        let project = ProjectSettings::default();

        assert_eq!(
            origin(ScopedField::TabPadding, &global, &project),
            Scope::Default
        );
        assert_eq!(
            origin(
                ScopedField::TabPadding,
                &Settings {
                    tab_padding: TabPaddingSettings {
                        left: Some(20),
                        ..TabPaddingSettings::default()
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
        assert_eq!(
            ScopedField::ALL.len(),
            11,
            "ADR-0022 names five areas (run configurations, editing, language \
             servers, terminal, and what ADR-0064 split index excludes into: \
             Excluded and IgnoredNames), plus Analysis (the PHP tooling plan's \
             B7), TabPadding (tab padding, per-side, project-overridable), \
             Containers (ADR-0055), BuildTools (jvm-build-tools plan, \
             ADR-0057 §3) and Database (Database Tools plan, F1.4)"
        );
    }

    #[test]
    fn an_explicit_empty_excluded_override_is_still_the_projects_own_answer() {
        // Excluded has no global field to inherit from (ADR-0064), so
        // silence and an explicit empty list resolve to the same visible
        // (empty) list — but `origin` still tells them apart, the same way
        // it does for run configurations.
        let global = Settings::default();
        let silent = ProjectSettings::default();
        let explicit = ProjectSettings {
            excluded: Some(Vec::new()),
            ..ProjectSettings::default()
        };

        assert!(resolve(&global, &silent).excluded.is_empty());
        assert!(resolve(&global, &explicit).excluded.is_empty());
        assert_eq!(
            origin(ScopedField::Excluded, &global, &silent),
            Scope::Default
        );
        assert_eq!(
            origin(ScopedField::Excluded, &global, &explicit),
            Scope::Project,
            "an explicit empty list is still the project's own answer"
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

    fn layout(tag: &str) -> Layout {
        Layout {
            window_state: format!("{tag}-docks"),
            editor_grid: format!("{tag}-grid"),
        }
    }

    fn with_layouts(pairs: &[(&str, &str)]) -> Settings {
        Settings {
            layouts: pairs
                .iter()
                .map(|(name, tag)| (name.to_string(), layout(tag)))
                .collect(),
            ..Settings::default()
        }
    }

    fn project_layouts(pairs: &[(&str, &str)]) -> ProjectSettings {
        ProjectSettings {
            layouts: Some(
                pairs
                    .iter()
                    .map(|(name, tag)| (name.to_string(), layout(tag)))
                    .collect(),
            ),
            ..ProjectSettings::default()
        }
    }

    #[test]
    fn a_project_layout_shadows_the_global_one_of_the_same_name() {
        let resolved = resolve_layouts(
            &with_layouts(&[("Debugging", "global")]),
            &project_layouts(&[("Debugging", "project")]),
        );

        assert_eq!(
            resolved,
            vec![("Debugging".to_string(), layout("project"), Scope::Project)],
            "the same name in both layers is one layout, and the project's wins"
        );
    }

    #[test]
    fn layouts_from_the_two_layers_are_merged_rather_than_replaced() {
        let resolved = resolve_layouts(
            &with_layouts(&[("Writing", "global")]),
            &project_layouts(&[("Debugging", "project")]),
        );

        assert_eq!(
            resolved
                .iter()
                .map(|(name, _, scope)| (name.as_str(), *scope))
                .collect::<Vec<_>>(),
            vec![("Debugging", Scope::Project), ("Writing", Scope::Global)],
            "a project shipping one layout must not hide the user's own, and the order is by name"
        );
    }

    #[test]
    fn a_silent_project_leaves_the_global_layouts_alone() {
        let global = with_layouts(&[("Writing", "global")]);

        assert_eq!(
            resolve_layouts(&global, &ProjectSettings::default()),
            vec![("Writing".to_string(), layout("global"), Scope::Global)]
        );
    }

    #[test]
    fn an_empty_project_table_still_cannot_clear_the_users_layouts() {
        // Unlike the sparse sections, `Some(empty)` is not "the project
        // overrides with none": a merge by name has no way to spell a
        // removal, so this is the same answer as saying nothing.
        let global = with_layouts(&[("Writing", "global")]);

        assert_eq!(
            resolve_layouts(&global, &project_layouts(&[])),
            resolve_layouts(&global, &ProjectSettings::default())
        );
    }

    #[test]
    fn containers_is_id_round_trips_and_resolves_like_terminal() {
        use app_config::ContainerSettings;

        assert_eq!(ScopedField::Containers.id(), "containers");
        assert_eq!(
            ScopedField::from_id("containers"),
            Some(ScopedField::Containers)
        );

        let global = Settings {
            containers: ContainerSettings {
                selinux_relabel: true,
                ..ContainerSettings::default()
            },
            ..Settings::default()
        };
        assert_eq!(
            origin(
                ScopedField::Containers,
                &global,
                &ProjectSettings::default()
            ),
            Scope::Global
        );

        let project = ProjectSettings {
            containers: Some(ContainerSettings::default()),
            ..ProjectSettings::default()
        };
        assert_eq!(
            origin(ScopedField::Containers, &global, &project),
            Scope::Project
        );
        assert_eq!(
            resolve(&global, &project).containers,
            ContainerSettings::default(),
            "the project's own (empty) override wins over the global layer"
        );
    }

    #[test]
    fn build_tools_id_round_trips() {
        assert_eq!(ScopedField::BuildTools.id(), "buildTools");
        assert_eq!(
            ScopedField::from_id("buildTools"),
            Some(ScopedField::BuildTools)
        );
    }

    /// ADR-0057 §3: a project's Gradle/Maven override resolves like any
    /// other scoped field, but `trusted_roots` — set only in the global
    /// layer, since `BuildToolsProjectSettings` has no such field —
    /// survives the overlay untouched regardless of what the project
    /// overrides.
    #[test]
    fn build_tools_resolves_gradle_and_maven_but_never_touches_trusted_roots() {
        use app_config::{BuildToolsProjectSettings, BuildToolsSettings, GradleToolSettings};
        use std::path::PathBuf;

        let global = Settings {
            build_tools: BuildToolsSettings {
                trusted_roots: vec![PathBuf::from("/home/u/project")],
                ..BuildToolsSettings::default()
            },
            ..Settings::default()
        };
        assert_eq!(
            origin(
                ScopedField::BuildTools,
                &global,
                &ProjectSettings::default()
            ),
            // trusted_roots alone must not read as "set globally" for this
            // field — see `set_globally`'s own comment.
            Scope::Default
        );

        let project = ProjectSettings {
            build_tools: Some(BuildToolsProjectSettings {
                gradle: GradleToolSettings {
                    offline: Some(true),
                    ..GradleToolSettings::default()
                },
                ..BuildToolsProjectSettings::default()
            }),
            ..ProjectSettings::default()
        };
        assert_eq!(
            origin(ScopedField::BuildTools, &global, &project),
            Scope::Project
        );
        let resolved = resolve(&global, &project);
        assert_eq!(resolved.build_tools.gradle.offline, Some(true));
        assert_eq!(
            resolved.build_tools.trusted_roots,
            vec![PathBuf::from("/home/u/project")],
            "a project override must never change which roots are trusted"
        );
    }

    /// Bridge-free coverage of the project-over-global merge
    /// `ui-shell::bridge::build_tools::build_local_repo_index` relies on
    /// for `[build_tools.maven].local_repository`: a project override wins,
    /// a global-only value survives when the project says nothing about
    /// Maven at all, and neither layer setting it resolves to `None` (D3's
    /// default `~/.m2/repository` is then `jvm_build_core`'s to supply).
    #[test]
    fn maven_local_repository_resolves_project_over_global_over_none() {
        use app_config::{BuildToolsProjectSettings, BuildToolsSettings, MavenToolSettings};
        use std::path::PathBuf;

        let global_only = Settings {
            build_tools: BuildToolsSettings {
                maven: MavenToolSettings {
                    local_repository: Some(PathBuf::from("/global/repo")),
                    ..MavenToolSettings::default()
                },
                ..BuildToolsSettings::default()
            },
            ..Settings::default()
        };

        // Neither layer sets it: resolves to `None`.
        assert_eq!(
            resolve(&Settings::default(), &ProjectSettings::default())
                .build_tools
                .maven
                .local_repository,
            None
        );

        // Global-only: the project is silent, so the global value survives.
        assert_eq!(
            resolve(&global_only, &ProjectSettings::default())
                .build_tools
                .maven
                .local_repository,
            Some(PathBuf::from("/global/repo"))
        );

        // Project overrides the global value.
        let project = ProjectSettings {
            build_tools: Some(BuildToolsProjectSettings {
                maven: MavenToolSettings {
                    local_repository: Some(PathBuf::from("/project/repo")),
                    ..MavenToolSettings::default()
                },
                ..BuildToolsProjectSettings::default()
            }),
            ..ProjectSettings::default()
        };
        assert_eq!(
            resolve(&global_only, &project)
                .build_tools
                .maven
                .local_repository,
            Some(PathBuf::from("/project/repo"))
        );
    }

    fn data_source(id: &str, name: &str) -> app_config::database::DataSourceSetting {
        app_config::database::DataSourceSetting {
            id: id.to_string(),
            name: name.to_string(),
            driver: "postgresql".to_string(),
            ..app_config::database::DataSourceSetting::default()
        }
    }

    fn with_sources(sources: Vec<app_config::database::DataSourceSetting>) -> Settings {
        Settings {
            database: app_config::database::DatabaseSettings {
                sources,
                ..app_config::database::DatabaseSettings::default()
            },
            ..Settings::default()
        }
    }

    fn project_sources(sources: Vec<app_config::database::DataSourceSetting>) -> ProjectSettings {
        ProjectSettings {
            database: Some(app_config::database::DatabaseProjectSettings {
                sources,
                ..app_config::database::DatabaseProjectSettings::default()
            }),
            ..ProjectSettings::default()
        }
    }

    #[test]
    fn a_project_source_shadows_the_global_one_of_the_same_id() {
        let resolved = resolve_database_sources(
            &with_sources(vec![data_source("abc", "global-name")]),
            &project_sources(vec![data_source("abc", "project-name")]),
        );
        assert_eq!(
            resolved,
            vec![(data_source("abc", "project-name"), Scope::Project)],
            "the same id in both layers is one source, and the project's wins"
        );
    }

    #[test]
    fn sources_from_the_two_layers_are_merged_rather_than_replaced() {
        let resolved = resolve_database_sources(
            &with_sources(vec![data_source("global-only", "g")]),
            &project_sources(vec![data_source("project-only", "p")]),
        );
        assert_eq!(
            resolved
                .iter()
                .map(|(source, scope)| (source.id.as_str(), *scope))
                .collect::<Vec<_>>(),
            vec![
                ("global-only", Scope::Global),
                ("project-only", Scope::Project),
            ],
            "a project adding one source must not hide the user's own"
        );
    }

    #[test]
    fn a_silent_project_leaves_the_global_sources_alone() {
        let resolved = resolve_database_sources(
            &with_sources(vec![data_source("abc", "global-name")]),
            &ProjectSettings::default(),
        );
        assert_eq!(
            resolved,
            vec![(data_source("abc", "global-name"), Scope::Global)]
        );
    }
}
