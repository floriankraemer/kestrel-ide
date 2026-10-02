//! Settings > Language Servers (task L6): the draft the page edits, and the
//! rule for which of its rows are worth persisting.
//!
//! Every language gets a row, including the ones with no server — that is
//! what removes the need for an `Add Server` button: configuring a server
//! for a language with no default is selecting its row and typing a
//! command.

use std::collections::HashMap;

use app_config::{LanguageServerSetting, Settings};
use lsp_core::{resolve_servers, PluginServer, ServerOverride};

/// What the Status column says before the live state is known — the part
/// that is a property of the configuration rather than of a running
/// process. The live states (`Starting`, `Running`, `Crashed, retrying`)
/// come from `LspManager`'s events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRowStatus {
    /// No command: the row is a placeholder, and has no checkbox.
    NotConfigured,
    /// Configured but switched off.
    Disabled,
    /// Configured and enabled; the live status replaces this once known.
    Enabled,
}

/// One row: a language, and what should be run for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerRow {
    /// Server id: the language id for a language with one server, the
    /// catalog's own id (`phpactor`) for one of several.
    pub id: String,
    /// LSP language id, the key both the catalog and the settings use.
    pub language_id: String,
    /// What the Language column shows.
    pub language_name: String,
    pub command: String,
    /// The Arguments field, one space-separated line.
    ///
    /// **Judgement call** (spec open question 2): users of this page know
    /// how to type a command line, and a list editor is four widgets and
    /// two behaviours to learn in exchange for arguments containing
    /// spaces. If those turn out to matter the upgrade is shell-style
    /// quoting in this same field, not a list editor.
    pub args: String,
    pub enabled: bool,
}

impl ServerRow {
    pub fn status(&self) -> ServerRowStatus {
        if self.command.trim().is_empty() {
            ServerRowStatus::NotConfigured
        } else if self.enabled {
            ServerRowStatus::Enabled
        } else {
            ServerRowStatus::Disabled
        }
    }
}

/// The LSP language id for one of the editor's own language ids.
///
/// Re-exported from `lsp-core` rather than reimplemented. What the protocol
/// calls a language is `lsp-core`'s business (ADR-0018), and this page must
/// emit the *same* string the runtime later looks a server up by: the page
/// writes the config key, `config_for_path` reads it, and a divergence
/// between two copies would mean a server the user configured and saw
/// enabled never starts, silently — which is exactly the failure ADR-0018
/// removed one layer down.
pub use lsp_core::lsp_language_id;

/// Whether a language could ever have a language server attached to it.
///
/// An injection-only grammar declares no extensions and no filenames:
/// `markdown_inline` is reached solely from the Markdown block grammar's
/// injections, never by opening a file. Since suffix-aware resolution
/// landed, `language_for_path` cannot return a language with no declared
/// patterns, so "declares no patterns" and "no file can ever open in it"
/// are the same statement — and a server that can never attach has no
/// business being offered as `Not configured` on the page. Plain text is
/// excluded for the older reason: "no highlighting" is not a language a
/// server is configured for.
pub fn can_have_server(language: syntax_core::Language) -> bool {
    language
        .def()
        .is_some_and(|def| def.extensions().next().is_some() || def.filenames().next().is_some())
}

/// The page's draft: one row per language, committed on OK.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerDraft {
    rows: Vec<ServerRow>,
    /// Per-language `(command, args)` each row is diffed against by
    /// `override_for` — the shipped catalog's default, or the contributing
    /// plugin's own default when a plugin supplies this language's server
    /// (see `resolve_servers`'s precedence). Absent for a language with
    /// neither: only a user-typed command created that row, so everything
    /// about it is theirs.
    baselines: HashMap<String, (String, Vec<String>)>,
}

impl ServerDraft {
    /// Build the rows from the saved settings, the languages the editor
    /// knows about, and the live plugin registry's server contributions.
    ///
    /// `languages` is `(lsp language id, display name)`; every entry gets a
    /// row even with no server configured, and every catalog, plugin, or
    /// user entry gets one even for a language the editor cannot
    /// highlight. Sorted by display name, stably, so a live status change
    /// never moves a row.
    pub fn new(
        settings: &Settings,
        languages: &[(String, String)],
        plugin_servers: &[PluginServer],
    ) -> Self {
        let overrides: Vec<ServerOverride> = settings
            .language_servers
            .iter()
            .map(|entry| ServerOverride {
                language_id: entry.language_id.clone(),
                name: entry.name.clone(),
                command: entry.command.clone(),
                args: entry.args.clone(),
                enabled: entry.enabled,
                ..Default::default()
            })
            .collect();

        let name_of = |language_id: &str| {
            languages
                .iter()
                .find(|(id, _)| id == language_id)
                .map(|(_, name)| name.clone())
                .unwrap_or_else(|| language_id.to_string())
        };

        // The baseline every row is diffed against: the catalog/plugin
        // default *before* any user override, which is exactly what
        // `resolve_servers` builds when given no overrides at all.
        let baselines: HashMap<String, (String, Vec<String>)> =
            resolve_servers(&[], plugin_servers)
                .into_iter()
                .map(|config| (config.id, (config.command, config.args)))
                .collect();

        let mut rows: Vec<ServerRow> = resolve_servers(&overrides, plugin_servers)
            .into_iter()
            .map(|config| ServerRow {
                id: config.id,
                language_name: name_of(&config.language_id),
                language_id: config.language_id,
                args: config.args.join(" "),
                command: config.command,
                enabled: config.enabled,
            })
            .collect();

        for (language_id, name) in languages {
            if rows.iter().any(|row| &row.language_id == language_id) {
                continue;
            }
            rows.push(ServerRow {
                id: language_id.clone(),
                language_id: language_id.clone(),
                language_name: name.clone(),
                command: String::new(),
                args: String::new(),
                enabled: false,
            });
        }

        rows.sort_by_key(|row| row.language_name.to_lowercase());
        Self { rows, baselines }
    }

    pub fn rows(&self) -> &[ServerRow] {
        &self.rows
    }

    pub fn row(&self, id: &str) -> Option<&ServerRow> {
        self.rows.iter().find(|row| row.id == id)
    }

    pub fn set_command(&mut self, id: &str, command: &str) {
        let has_default = self.baselines.contains_key(id);
        if let Some(row) = self.row_mut(id) {
            row.command = command.trim().to_string();
            // A language with no shipped or plugin-contributed default only
            // has a row at all because the user typed a command into it —
            // they typed it to use it, not to leave it switched off.
            if !row.command.is_empty() && !has_default {
                row.enabled = true;
            }
        }
    }

    pub fn set_args(&mut self, id: &str, args: &str) {
        if let Some(row) = self.row_mut(id) {
            row.args = args.split_whitespace().collect::<Vec<_>>().join(" ");
        }
    }

    pub fn set_enabled(&mut self, id: &str, enabled: bool) {
        if let Some(row) = self.row_mut(id) {
            row.enabled = enabled;
        }
    }

    /// The `[[language_server]]` entries worth writing: only what differs
    /// from each row's baseline (the shipped catalog's or the contributing
    /// plugin's own default), and only the fields that differ, so changing
    /// a shipped or plugin default still reaches a user who never touched
    /// it — the same rule the keymap follows.
    pub fn overrides(&self) -> Vec<LanguageServerSetting> {
        self.rows
            .iter()
            .filter_map(|row| override_for(row, self.baselines.get(&row.id)))
            .collect()
    }

    /// Commit the draft into settings.
    pub fn apply_to(&self, settings: &mut Settings) {
        settings.language_servers = self.overrides();
    }

    fn row_mut(&mut self, id: &str) -> Option<&mut ServerRow> {
        self.rows.iter_mut().find(|row| row.id == id)
    }
}

fn override_for(
    row: &ServerRow,
    baseline: Option<&(String, Vec<String>)>,
) -> Option<LanguageServerSetting> {
    let args: Vec<String> = row.args.split_whitespace().map(str::to_string).collect();
    let command = row.command.trim();

    match baseline {
        Some((default_command, default_args)) => {
            let command_differs = command != default_command;
            let args_differ = &args != default_args;
            if !command_differs && !args_differ && row.enabled {
                return None;
            }
            Some(LanguageServerSetting {
                language_id: row.language_id.clone(),
                name: None,
                command: command_differs.then(|| command.to_string()),
                args: args_differ.then_some(args),
                enabled: (!row.enabled).then_some(false),
            })
        }
        // No catalog or plugin entry: the row only exists once the user
        // typed a command, and then everything about it is theirs.
        None if command.is_empty() => None,
        None => Some(LanguageServerSetting {
            language_id: row.language_id.clone(),
            name: None,
            command: Some(command.to_string()),
            args: (!args.is_empty()).then_some(args),
            enabled: (!row.enabled).then_some(false),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn languages() -> Vec<(String, String)> {
        [("rust", "Rust"), ("nim", "Nim"), ("cpp", "C++")]
            .into_iter()
            .map(|(id, name)| (id.to_string(), name.to_string()))
            .collect()
    }

    fn draft() -> ServerDraft {
        ServerDraft::new(&Settings::default(), &languages(), &[])
    }

    #[test]
    fn every_language_gets_a_row_even_without_a_server() {
        let draft = draft();
        let nim = draft.row("nim").expect("row");
        assert_eq!(nim.command, "");
        assert_eq!(nim.status(), ServerRowStatus::NotConfigured);
        // And every catalog server is there too, named after its language.
        assert_eq!(draft.row("rust").expect("row").command, "rust-analyzer");
        assert_eq!(draft.row("rust").expect("row").language_name, "Rust");
        assert!(draft.row("python").is_some());
    }

    #[test]
    fn rows_are_sorted_by_language_name() {
        let names: Vec<String> = draft()
            .rows()
            .iter()
            .map(|row| row.language_name.to_lowercase())
            .collect();
        let mut sorted = names.clone();
        sorted.sort();
        assert_eq!(names, sorted);
    }

    #[test]
    fn an_untouched_draft_persists_nothing() {
        assert!(draft().overrides().is_empty());
    }

    #[test]
    fn only_the_changed_field_is_persisted() {
        let mut draft = draft();
        draft.set_command("rust", "/opt/ra");
        let overrides = draft.overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].language_id, "rust");
        assert_eq!(overrides[0].command.as_deref(), Some("/opt/ra"));
        assert_eq!(overrides[0].args, None);
        assert_eq!(overrides[0].enabled, None);
    }

    #[test]
    fn disabling_a_shipped_server_keeps_its_command() {
        let mut draft = draft();
        draft.set_enabled("go", false);
        let overrides = draft.overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].enabled, Some(false));
        assert_eq!(overrides[0].command, None);
        assert_eq!(
            draft.row("go").expect("row").status(),
            ServerRowStatus::Disabled
        );
    }

    #[test]
    fn configuring_a_language_with_no_default_persists_all_of_it() {
        let mut draft = draft();
        draft.set_command("nim", "zls");
        draft.set_args("nim", "  --enable-debug   --stdio ");
        let overrides = draft.overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].language_id, "nim");
        assert_eq!(overrides[0].command.as_deref(), Some("zls"));
        assert_eq!(
            overrides[0].args,
            Some(vec!["--enable-debug".to_string(), "--stdio".to_string()])
        );
        assert_eq!(
            draft.row("nim").expect("row").status(),
            ServerRowStatus::Enabled
        );
    }

    fn csharp_plugin() -> PluginServer {
        PluginServer {
            plugin_id: "csharp".to_string(),
            language_id: "csharp".to_string(),
            name: "csharp-ls".to_string(),
            command: "csharp-ls".to_string(),
            args: vec!["--stdio".to_string()],
            settings_section: None,
            settings: serde_json::Value::Null,
        }
    }

    #[test]
    fn an_untouched_plugin_backed_row_persists_nothing() {
        let draft = ServerDraft::new(
            &Settings::default(),
            &languages(),
            std::slice::from_ref(&csharp_plugin()),
        );
        let row = draft.row("csharp").expect("row");
        assert_eq!(row.command, "csharp-ls");
        assert!(draft.overrides().is_empty());
    }

    #[test]
    fn a_plugin_backed_row_only_persists_what_the_user_changed() {
        let mut draft = ServerDraft::new(
            &Settings::default(),
            &languages(),
            std::slice::from_ref(&csharp_plugin()),
        );
        draft.set_enabled("csharp", false);
        let overrides = draft.overrides();
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].language_id, "csharp");
        assert_eq!(overrides[0].enabled, Some(false));
        // The command/args the plugin shipped were never touched, so they
        // are not written even though `default_server("csharp")` (the const
        // catalog) knows nothing about this row at all.
        assert_eq!(overrides[0].command, None);
        assert_eq!(overrides[0].args, None);
    }

    #[test]
    fn a_draft_round_trips_through_settings() {
        let mut settings = Settings::default();
        let mut draft = draft();
        draft.set_command("rust", "/opt/ra");
        draft.set_enabled("go", false);
        draft.set_command("nim", "zls");
        draft.apply_to(&mut settings);

        let reloaded = ServerDraft::new(&settings, &languages(), &[]);
        assert_eq!(reloaded.row("rust").expect("row").command, "/opt/ra");
        assert!(!reloaded.row("go").expect("row").enabled);
        assert_eq!(reloaded.row("nim").expect("row").command, "zls");
        assert_eq!(reloaded, draft);
    }

    #[test]
    fn clearing_a_command_drops_the_row_from_settings() {
        let mut draft = draft();
        draft.set_command("nim", "zls");
        assert_eq!(draft.overrides().len(), 1);
        draft.set_command("nim", "");
        assert!(draft.overrides().is_empty());
    }

    #[test]
    fn arguments_are_normalised_to_one_line() {
        let mut draft = draft();
        draft.set_args("rust", "  --a    --b ");
        assert_eq!(draft.row("rust").expect("row").args, "--a --b");
    }

    #[test]
    fn editor_language_ids_map_onto_protocol_ones() {
        assert_eq!(lsp_language_id("tsx"), "typescriptreact");
        assert_eq!(lsp_language_id("rust"), "rust");
        assert_eq!(lsp_language_id("csharp"), "csharp");
    }

    /// Injection-only grammars are not configurable servers.
    ///
    /// `markdown_inline` declares no extensions and no filenames, so no
    /// file can ever open in it and no server could ever attach. The
    /// predicate is on the property, not the id, so the next
    /// injection-only grammar added to the catalog is excluded too.
    #[test]
    fn only_languages_a_file_can_open_in_can_have_a_server() {
        let by_id = |wanted: &str| {
            syntax_core::registry()
                .languages()
                .into_iter()
                .find(|language| language.id() == wanted)
                .unwrap_or_else(|| panic!("`{wanted}` is not in the registry"))
        };
        assert!(!can_have_server(by_id("markdown_inline")));
        assert!(can_have_server(by_id("markdown")));
        assert!(can_have_server(by_id("rust")));
        assert!(!can_have_server(syntax_core::Language::PLAIN_TEXT));
    }

    /// Guards the *class* of bug, not one instance of it.
    ///
    /// This page writes a server's config key using `lsp_language_id`, and
    /// the runtime later reads that key back through the same function on
    /// its way to `config_for_path`. While there is one implementation the
    /// two agree by construction. There used to be two — this crate had its
    /// own `match` beside `lsp-core`'s table — and they agreed only because
    /// both happened to map `tsx` and pass everything else through. The next
    /// divergence added to one and not the other would have meant a server
    /// the user configured, saved, and saw enabled simply never starting.
    ///
    /// So this asserts the property that matters: every id the page can emit
    /// round-trips to exactly what the runtime will look up, for every
    /// language in the catalog. A second mapping reintroduced anywhere makes
    /// it fail as soon as the two disagree about any catalog language.
    #[test]
    fn every_page_row_id_round_trips_to_what_the_runtime_looks_up() {
        for language in syntax_core::registry().languages() {
            let Some(def) = language.def() else {
                continue; // plain text has no id worth mapping
            };
            let from_page = lsp_language_id(def.id());
            let from_runtime = lsp_core::lsp_language_id(def.id());
            assert_eq!(
                from_page,
                from_runtime,
                "`{}` maps to `{from_page}` on the settings page but \
                 `{from_runtime}` in the runtime; a server configured here \
                 would never start",
                def.id()
            );
        }
    }
}
