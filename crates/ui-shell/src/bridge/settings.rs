use core::pin::Pin;
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;

use ai_chat_core::models::{self, ModelInfo};
use ai_chat_core::providers::{ProviderConfig, ProviderKind};
use ai_chat_core::tools;
use ai_chat_core::ChatError;
use cxx_qt::Threading;
use cxx_qt_lib::{QString, QStringList};
use syntax_core::theme;

use crate::bridge::convert::{load_settings, user_styles};
use crate::bridge::errors;
use crate::bridge::ffi::{
    self, FfiEditingProblem, FfiEditingRow, FfiEditorColors, FfiEditorFont, FfiResult,
    FfiUiFontScales, FfiWhitespaceOptions, FfiWindowGeometry,
};

/// Rust side of the `AppSettings` QObject: every call re-reads or re-writes
/// `settings.toml` directly (mirrors `push_recent_project`).
///
/// The one piece of state is which scope the settings dialog is currently
/// showing (F0-10). It is dialog session state, not domain state — nothing
/// persists it, and closing the dialog leaves it where it was — and it lives
/// on the one object every page already has a pointer to, so the scope
/// selector and the origin badges cannot disagree about which layer is being
/// looked at.
pub struct AppSettingsRust {
    scope: RefCell<settings_model::Scope>,
}

impl Default for AppSettingsRust {
    /// Global until the user says otherwise: a project cannot configure the
    /// person, so the person's own layer is the one that opens.
    fn default() -> Self {
        Self {
            scope: RefCell::new(settings_model::Scope::Global),
        }
    }
}

/// The scope names crossing the seam. Strings rather than a shared enum
/// because the C++ side only ever passes back what it was handed, and a
/// second FFI enum for two values is more seam than the feature needs.
const SCOPE_GLOBAL: &str = "global";
const SCOPE_PROJECT: &str = "project";

fn scope_from_name(name: &str) -> settings_model::Scope {
    match name {
        SCOPE_PROJECT => settings_model::Scope::Project,
        // Anything unrecognised is the global layer, which is the answer
        // that can never write into a file the project shares.
        _ => settings_model::Scope::Global,
    }
}

fn scope_name(scope: settings_model::Scope) -> &'static str {
    match scope {
        settings_model::Scope::Project => SCOPE_PROJECT,
        _ => SCOPE_GLOBAL,
    }
}

impl ffi::AppSettings {
    /// Which layer the settings dialog is editing: `"global"` or
    /// `"project"`.
    pub fn settings_scope(&self) -> QString {
        QString::from(scope_name(*self.scope.borrow()))
    }

    /// Switch the layer the dialog edits. Emits `settingsScopeChanged` so
    /// every open page reloads its draft from the layer now selected.
    pub fn set_settings_scope(self: Pin<&mut Self>, scope: &QString) {
        let next = scope_from_name(&scope.to_string());
        if *self.scope.borrow() == next {
            return;
        }
        *self.scope.borrow_mut() = next;
        self.settings_scope_changed();
    }

    /// Whether the open project has a settings file of its own — what the
    /// scope selector needs to say "this project overrides nothing yet"
    /// rather than pretending the file is there.
    pub fn has_project_settings(&self) -> bool {
        !crate::bridge::convert::load_project_settings().is_empty()
    }

    /// Whether a project is open at all — the difference between "this
    /// project overrides nothing yet" and "there is no project to override
    /// anything".
    pub fn is_project_open(&self) -> bool {
        crate::bridge::convert::current_project_root().is_some()
    }

    /// Where the effective value of one scoped field comes from, as the word
    /// the badge shows: "from project", "from global" or "default".
    ///
    /// The answer is `settings_model::scope`'s and the view never re-derives
    /// it (ADR-0022) — a badge computed separately from the value is a badge
    /// that eventually lies.
    pub fn field_origin(&self, field_id: &QString) -> QString {
        let Some(field) = settings_model::ScopedField::from_id(&field_id.to_string()) else {
            // Not an overridable setting at all, so its value can only have
            // come from the global layer or a default.
            return QString::from(settings_model::Scope::Global.label());
        };
        let origin = settings_model::origin_for_view(
            field,
            &crate::bridge::convert::load_settings(),
            &crate::bridge::convert::load_project_settings(),
            *self.scope.borrow(),
        );
        QString::from(origin.label())
    }

    pub fn recent_projects(&self) -> QStringList {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        settings
            .recent_projects
            .iter()
            .map(|p| QString::from(p.to_string_lossy().as_ref()))
            .collect()
    }

    pub fn reload_languages(&self) -> QStringList {
        let config_dir = app_core::resolve_config_dir();
        let disabled = app_config::load(&config_dir)
            .unwrap_or_default()
            .disabled_languages;
        syntax_core::reload(&config_dir, &disabled)
            .iter()
            .map(|err| QString::from(err.to_string().as_str()))
            .collect()
    }

    pub fn window_geometry(&self) -> FfiWindowGeometry {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        let g = settings.window_geometry;
        FfiWindowGeometry {
            x: g.x,
            y: g.y,
            width: g.width,
            height: g.height,
        }
    }

    pub fn save_window_geometry(&self, x: i32, y: i32, width: u32, height: u32) {
        let geometry = app_config::WindowGeometry {
            x,
            y,
            width,
            height,
        };
        // A window on its way out can report a 0x0 rect; persisting it would
        // replace a usable saved size with one the next launch has to throw
        // away. Keeping the previous geometry is the better answer.
        if !geometry.is_usable() {
            return;
        }
        let _ = app_config::update(&app_core::resolve_config_dir(), |settings| {
            settings.window_geometry = geometry;
        });
    }

    pub fn window_state(&self) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(settings.window_state.as_str())
    }

    pub fn save_window_state(&self, state: &QString) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.window_state = state.to_string();
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn editor_layout(&self) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(settings.editor_layout.as_str())
    }

    pub fn save_editor_layout(&self, layout: &QString) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.editor_layout = layout.to_string();
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn theme_name(&self) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(settings.theme_name())
    }

    pub fn save_theme(&self, theme: &QString) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.theme = theme.to_string();
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn icon_theme_id(&self) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(settings.icon_theme.as_str())
    }

    pub fn save_icon_theme(&self, id: &QString) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.icon_theme = id.to_string();
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn editor_font(&self) -> FfiEditorFont {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        FfiEditorFont {
            family: QString::from(settings.editor_font_family_or_default()),
            size: settings.editor_font_size_or_default(),
        }
    }

    pub fn save_editor_font(&self, family: &QString, size: u32) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.editor_font_family = family.to_string();
        settings.editor_font_size = size;
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn whitespace_options(&self) -> FfiWhitespaceOptions {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        FfiWhitespaceOptions {
            enabled: settings.show_whitespace,
            leading: settings.show_whitespace_leading,
            inner: settings.show_whitespace_inner,
            trailing: settings.show_whitespace_trailing,
            eol_markers: settings.show_eol_markers,
        }
    }

    pub fn save_whitespace_options(&self, options: &FfiWhitespaceOptions) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.show_whitespace = options.enabled;
        settings.show_whitespace_leading = options.leading;
        settings.show_whitespace_inner = options.inner;
        settings.show_whitespace_trailing = options.trailing;
        settings.show_eol_markers = options.eol_markers;
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn mcp_discovery_file_path(&self) -> QString {
        let path = mcp_server::discovery_file_path(&app_core::resolve_config_dir());
        QString::from(path.to_string_lossy().as_ref())
    }

    pub fn mcp_enabled(&self) -> bool {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        settings.mcp_enabled_or_default()
    }

    pub fn mcp_port(&self) -> u16 {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        settings.mcp_port
    }

    pub fn save_mcp_settings(&self, enabled: bool, port: u16) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.mcp_enabled = Some(enabled);
        settings.mcp_port = port;
        let _ = app_config::save(&config_dir, &settings);
    }

    /// The `[terminal]` section of the layer the settings dialog is
    /// currently editing.
    ///
    /// Project scope shows the project's own override, defaulted when there
    /// is none — the same rule `EditingEditor::begin_edit` follows, and for
    /// the same reason: a project page pre-filled with global values would
    /// turn "look at this layer" into "copy the other layer into it" on the
    /// first OK.
    pub fn terminal_settings(&self) -> ffi::FfiTerminalSettings {
        let terminal = match *self.scope.borrow() {
            settings_model::Scope::Project => crate::bridge::convert::load_project_settings()
                .terminal
                .unwrap_or_default(),
            _ => load_settings().terminal,
        };
        to_ffi_terminal_settings(&terminal)
    }

    /// Write the `[terminal]` section back to the layer being edited.
    ///
    /// A project override that says nothing is removed rather than written
    /// as an empty section: `.ide/settings.toml` is reviewed by people, and
    /// a section that overrides nothing reads as one that does.
    pub fn save_terminal_settings(&self, terminal: &ffi::FfiTerminalSettings) -> FfiResult {
        let terminal = from_ffi_terminal_settings(terminal);
        if *self.scope.borrow() == settings_model::Scope::Project {
            let section = (terminal != app_config::TerminalSettings::default()).then_some(terminal);
            return commit_to_project(|project| project.terminal = section);
        }
        let config_dir = app_core::resolve_config_dir();
        match app_config::update(&config_dir, |settings| settings.terminal = terminal) {
            Ok(()) => FfiResult::default(),
            Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
        }
    }

    /// Every shell this machine offers, for the Terminal page's combo. The
    /// same list the dock's "+" dropdown shows, from the same place — see
    /// `TerminalSupervisor::available_shells`.
    pub fn available_shells(&self) -> Vec<ffi::FfiShellCandidate> {
        pty_core::shells::detect()
            .into_iter()
            .map(|candidate| ffi::FfiShellCandidate {
                id: QString::from(candidate.id.as_str()),
                label: QString::from(candidate.label.as_str()),
            })
            .collect()
    }

    pub fn shortcut_for(&self, action_id: &QString) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(settings.keymap().shortcut_for(&action_id.to_string()))
    }

    pub fn ui_font_scales(&self) -> FfiUiFontScales {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        FfiUiFontScales {
            ui: settings.ui_font_scale_or_default(),
            project_tree: settings.project_tree_font_scale_or_default(),
            menu: settings.menu_font_scale_or_default(),
        }
    }

    pub fn save_ui_font_scales(&self, ui: u32, project_tree: u32, menu: u32) {
        let _ = app_config::update(&app_core::resolve_config_dir(), |settings| {
            settings.ui_font_scale = ui;
            settings.project_tree_font_scale = project_tree;
            settings.menu_font_scale = menu;
        });
    }

    pub fn editor_colors(&self) -> FfiEditorColors {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        FfiEditorColors {
            background: QString::from(
                settings
                    .editor_colors
                    .get("background")
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
            foreground: QString::from(
                settings
                    .editor_colors
                    .get("foreground")
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
            current_line: QString::from(
                settings
                    .editor_colors
                    .get("current_line")
                    .map(String::as_str)
                    .unwrap_or(""),
            ),
        }
    }

    pub fn save_editor_colors(
        &self,
        background: &QString,
        foreground: &QString,
        current_line: &QString,
    ) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        let background = background.to_string();
        let foreground = foreground.to_string();
        let current_line = current_line.to_string();
        if background.is_empty() {
            settings.editor_colors.remove("background");
        } else {
            settings
                .editor_colors
                .insert("background".to_string(), background);
        }
        if foreground.is_empty() {
            settings.editor_colors.remove("foreground");
        } else {
            settings
                .editor_colors
                .insert("foreground".to_string(), foreground);
        }
        if current_line.is_empty() {
            settings.editor_colors.remove("current_line");
        } else {
            settings
                .editor_colors
                .insert("current_line".to_string(), current_line);
        }
        let _ = app_config::save(&config_dir, &settings);
    }
}

/// Rust side of the `KeymapEditor` QObject: unlike `AppSettings` (stateless,
/// re-reads `settings.toml` per call) this one holds the settings dialog's
/// draft keymap, so an edit only reaches disk when `commit` is called.
/// `RefCell` rather than `Pin<&mut Self>` mutation, matching how
/// `TerminalSupervisorRust` keeps its interior state.
#[derive(Default)]
pub struct KeymapEditorRust {
    draft: RefCell<app_config::Keymap>,
}

impl ffi::KeymapEditor {
    pub fn begin_edit(&self) {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        *self.draft.borrow_mut() = settings.keymap();
    }

    pub fn bindings(&self) -> Vec<ffi::FfiKeyBinding> {
        self.draft
            .borrow()
            .bindings()
            .into_iter()
            .map(|binding| ffi::FfiKeyBinding {
                action_id: QString::from(binding.action.id),
                label: QString::from(binding.action.label),
                category: QString::from(binding.action.category),
                shortcut: QString::from(binding.shortcut.as_str()),
                is_default: binding.is_default,
            })
            .collect()
    }

    pub fn conflicts(&self, action_id: &QString, shortcut: &QString) -> QStringList {
        self.draft
            .borrow()
            .conflicts(&action_id.to_string(), &shortcut.to_string())
            .iter()
            .map(|action| QString::from(action.label))
            .collect()
    }

    pub fn assign(&self, action_id: &QString, shortcut: &QString) {
        self.draft
            .borrow_mut()
            .assign(&action_id.to_string(), &shortcut.to_string());
    }

    pub fn reset_defaults(&self) {
        self.draft.borrow_mut().reset_to_defaults();
    }

    pub fn commit(&self) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.set_keymap(self.draft.borrow().clone());
        let _ = app_config::save(&config_dir, &settings);
    }
}

/// Rust side of `SyntaxColorEditor` (T4). Holds the draft and the snapshot
/// `beginEdit` took; every rule is `settings_model::SyntaxColorDraft` and
/// `syntax_core::theme`.
#[derive(Default)]
pub struct SyntaxColorEditorRust {
    draft: RefCell<settings_model::SyntaxColorDraft>,
    /// The saved tables as they were when the dialog opened, so Cancel can
    /// put them back — the page applies live, so there is something to undo.
    snapshot: RefCell<Option<settings_model::SyntaxColorDraft>>,
}

/// Level as the page names it: an empty language id is the base table.
fn color_level(language_id: &QString) -> Option<String> {
    let id = language_id.to_string();
    (!id.is_empty()).then_some(id)
}

impl SyntaxColorEditorRust {
    /// Write the draft through to settings, which is what makes the page
    /// apply live: the highlighters re-read them on the next repaint.
    fn save(&self) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        self.draft.borrow().apply_to(&mut settings);
        let _ = app_config::save(&config_dir, &settings);
    }
}

impl ffi::SyntaxColorEditor {
    pub fn begin_edit(&self) {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        let draft = settings_model::SyntaxColorDraft::from_settings(&settings);
        *self.snapshot.borrow_mut() = Some(draft.clone());
        *self.draft.borrow_mut() = draft;
    }

    pub fn languages(&self) -> Vec<ffi::FfiLanguageOption> {
        syntax_core::registry()
            .languages()
            .into_iter()
            // Every language with queries can be themed, including the
            // injection-only ones: `markdown_inline` never owns a file but
            // its spans are what colour a Markdown paragraph, so its
            // per-language overrides are reachable and worth offering.
            .filter(|language| *language != syntax_core::Language::PLAIN_TEXT)
            .map(|language| ffi::FfiLanguageOption {
                id: QString::from(&language.id()),
                name: QString::from(&language.name()),
            })
            .collect()
    }

    pub fn scopes(&self, language_id: &QString) -> Vec<ffi::FfiSyntaxScopeRow> {
        let level = color_level(language_id);
        let draft = self.draft.borrow();

        // The Sample cell shows what the editor will paint, which is the
        // draft resolved against the active theme — not the entry stored on
        // the row, which may be nothing at all.
        let mut settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        draft.apply_to(&mut settings);
        let theme_name = settings.theme_name().to_string();
        let palette = theme::palette(
            &theme_name,
            level.as_deref().unwrap_or_default(),
            &user_styles(&settings),
        );

        settings_model::ordered_scopes()
            .into_iter()
            .filter_map(|name| Some((name, syntax_core::Scope::resolve(name)?)))
            .map(|(name, scope)| {
                let resolved = palette.style(scope);
                let fg = resolved.fg.unwrap_or(theme::Rgb::new(0, 0, 0));
                let entry = draft.effective(level.as_deref(), name);
                ffi::FfiSyntaxScopeRow {
                    scope: QString::from(name),
                    family: QString::from(settings_model::scope_family(name)),
                    sample: QString::from(settings_model::scope_sample(name)),
                    origin: match draft.origin(level.as_deref(), name) {
                        settings_model::Origin::Theme => ffi::FfiColorOrigin::Theme,
                        settings_model::Origin::Base => ffi::FfiColorOrigin::Base,
                        settings_model::Origin::Language => ffi::FfiColorOrigin::Language,
                    },
                    has_fg: resolved.fg.is_some(),
                    red: fg.r,
                    green: fg.g,
                    blue: fg.b,
                    sample_bold: resolved.bold,
                    sample_italic: resolved.italic,
                    sample_underline: resolved.underline,
                    hex: QString::from(entry.and_then(|style| style.fg()).unwrap_or_default()),
                    bold: entry.is_some_and(|style| style.bold()),
                    italic: entry.is_some_and(|style| style.italic()),
                    underline: entry.is_some_and(|style| style.underline()),
                    can_reset: draft.can_clear(level.as_deref(), name),
                }
            })
            .collect()
    }

    pub fn set_style(
        &self,
        language_id: &QString,
        scope: &QString,
        hex: &QString,
        bold: bool,
        italic: bool,
        underline: bool,
    ) {
        let level = color_level(language_id);
        let hex = hex.to_string();
        self.draft.borrow_mut().set_style(
            level.as_deref(),
            &scope.to_string(),
            Some(hex.as_str()),
            bold,
            italic,
            underline,
        );
        self.save();
    }

    pub fn reset_scope(&self, language_id: &QString, scope: &QString) {
        let level = color_level(language_id);
        self.draft
            .borrow_mut()
            .clear(level.as_deref(), &scope.to_string());
        self.save();
    }

    pub fn reset_level(&self, language_id: &QString) {
        let level = color_level(language_id);
        self.draft.borrow_mut().clear_level(level.as_deref());
        self.save();
    }

    pub fn can_reset_level(&self, language_id: &QString) -> bool {
        let level = color_level(language_id);
        self.draft.borrow().can_clear_level(level.as_deref())
    }

    pub fn revert(&self) {
        let Some(snapshot) = self.snapshot.borrow_mut().take() else {
            return;
        };
        *self.draft.borrow_mut() = snapshot;
        self.save();
    }

    pub fn unknown_scope_warning(&self) -> QString {
        let settings = app_config::load(&app_core::resolve_config_dir()).unwrap_or_default();
        QString::from(&settings_model::unknown_scope_warning(&settings))
    }
}

/// Rust side of `LanguageCatalog` (G3).
///
/// The overlay is scanned here rather than read out of the global registry
/// because the registry keeps only what loaded — and the whole point of this
/// page is the entries that did not.
#[derive(Default)]
pub struct LanguageCatalogRust {
    rows: RefCell<Vec<settings_model::LanguageRow>>,
}

fn to_ffi_io_result(result: std::io::Result<String>) -> FfiResult {
    match result {
        Ok(_) => FfiResult::default(),
        Err(err) => FfiResult {
            code: errors::CODE_SETTINGS_IO,
            message: QString::from(err.to_string().as_str()),
        },
    }
}

impl ffi::LanguageCatalog {
    pub fn refresh(&self) {
        let config_dir = app_core::resolve_config_dir();
        // The scan's definitions are read into rows and dropped with
        // `overlay` when this method returns — refreshing the page costs
        // nothing permanently.
        let overlay = syntax_core::runtime::load_builtin_overlay(&config_dir);
        let builtins: Vec<settings_model::languages::CatalogEntry> = syntax_core::BUILTIN_LANGUAGES
            .iter()
            .map(|def| settings_model::languages::catalog_entry(&syntax_core::Def::Builtin(def)))
            .collect();
        let loaded: Vec<settings_model::languages::CatalogEntry> = overlay
            .entries
            .iter()
            .map(|def| {
                settings_model::languages::catalog_entry(&syntax_core::Def::Runtime(def.clone()))
            })
            .collect();
        let disabled = app_config::load(&config_dir)
            .unwrap_or_default()
            .disabled_languages;
        *self.rows.borrow_mut() = settings_model::languages::rows(
            &builtins,
            &loaded,
            &overlay.errors,
            &settings_model::scan_manifests(&config_dir),
            &disabled,
        );
    }

    pub fn languages(&self) -> Vec<ffi::FfiLanguageRow> {
        self.rows
            .borrow()
            .iter()
            .map(|row| ffi::FfiLanguageRow {
                id: QString::from(row.id.as_str()),
                name: QString::from(row.name.as_str()),
                matches: QString::from(row.matches.as_str()),
                status: QString::from(row.status.text()),
                source: match row.source {
                    settings_model::LanguageSource::BuiltIn => ffi::FfiLanguageSource::BuiltIn,
                    settings_model::LanguageSource::Overlay => ffi::FfiLanguageSource::Overlay,
                    settings_model::LanguageSource::Library => ffi::FfiLanguageSource::Library,
                },
                severity: match row.status {
                    settings_model::LanguageStatus::Ok => ffi::FfiRowSeverity::Healthy,
                    settings_model::LanguageStatus::Disabled => ffi::FfiRowSeverity::Muted,
                    settings_model::LanguageStatus::DisabledAfterCrash => {
                        ffi::FfiRowSeverity::Warning
                    }
                    _ => ffi::FfiRowSeverity::Error,
                },
            })
            .collect()
    }

    pub fn problem(&self, id: &QString) -> ffi::FfiLanguageProblem {
        let id = id.to_string();
        let rows = self.rows.borrow();
        let problem = rows
            .iter()
            .find(|row| row.id == id)
            .and_then(|row| row.problem.as_ref());
        let Some(problem) = problem else {
            return ffi::FfiLanguageProblem::default();
        };
        let offers = |action| problem.actions.contains(&action);
        ffi::FfiLanguageProblem {
            artifact: QString::from(problem.artifact.as_str()),
            sentence: QString::from(problem.sentence.as_str()),
            detail: QString::from(problem.detail.as_str()),
            path: QString::from(problem.path.as_str()),
            confirm: QString::from(problem.confirm.as_str()),
            marker: QString::from(problem.marker.as_str()),
            open_file: offers(settings_model::LanguageAction::OpenFile),
            reload: offers(settings_model::LanguageAction::Reload),
            open_folder: offers(settings_model::LanguageAction::OpenFolder),
        }
    }

    pub fn toggle(&self, id: &QString) -> ffi::FfiLanguageToggle {
        let id = id.to_string();
        let rows = self.rows.borrow();
        let toggle = settings_model::languages::toggle(rows.iter().find(|row| row.id == id));
        ffi::FfiLanguageToggle {
            label: QString::from(toggle.label),
            enabled: toggle.enabled,
            disable: toggle.disable,
        }
    }

    pub fn set_disabled(&self, id: &QString, disabled: bool) -> FfiResult {
        let id = id.to_string();
        let config_dir = app_core::resolve_config_dir();
        // Never edit a defaulted Settings here: saving that back would drop
        // everything else the file holds.
        let mut settings = match app_config::load(&config_dir) {
            Ok(settings) => settings,
            Err(err) => {
                return FfiResult {
                    code: errors::CODE_SETTINGS_IO,
                    message: QString::from(err.to_string().as_str()),
                }
            }
        };
        if disabled {
            settings.set_language_disabled(&id, true);
        } else {
            let row = self.rows.borrow().iter().find(|row| row.id == id).cloned();
            let enabled = match &row {
                Some(row) => settings_model::languages::enable(&mut settings, row),
                None => {
                    settings.set_language_disabled(&id, false);
                    Ok(())
                }
            };
            if let Err(err) = enabled {
                return FfiResult {
                    code: errors::CODE_REFUSED,
                    message: QString::from(err.to_string().as_str()),
                };
            }
        }
        if let Err(err) = app_config::save(&config_dir, &settings) {
            return FfiResult {
                code: errors::CODE_SETTINGS_IO,
                message: QString::from(err.to_string().as_str()),
            };
        }
        // Same swap the reload path does, so the change reaches files that
        // are already open instead of waiting for a restart.
        syntax_core::reload(&config_dir, &settings.disabled_languages);
        self.refresh();
        FfiResult::default()
    }

    pub fn add_language_folder(&self, path: &QString) -> FfiResult {
        let config_dir = app_core::resolve_config_dir();
        to_ffi_io_result(settings_model::languages::install_language_folder(
            &config_dir,
            Path::new(&path.to_string()),
        ))
    }

    pub fn add_grammar_library(&self, path: &QString) -> FfiResult {
        let config_dir = app_core::resolve_config_dir();
        to_ffi_io_result(settings_model::languages::install_grammar_library(
            &config_dir,
            Path::new(&path.to_string()),
        ))
    }

    pub fn languages_dir(&self) -> QString {
        QString::from(
            app_core::resolve_config_dir()
                .join(settings_model::languages::LANGUAGES_DIR)
                .display()
                .to_string()
                .as_str(),
        )
    }
}

/// Rust side of `LanguageServerEditor` (L6).
#[derive(Default)]
pub struct LanguageServerEditorRust {
    draft: RefCell<Option<settings_model::ServerDraft>>,
    /// What was saved when the page opened, so the page can tell a row it
    /// has edited from one it has not without diffing widgets.
    saved: RefCell<Option<settings_model::ServerDraft>>,
    /// The layer this draft came from and will be written back to (F0-10).
    scope: RefCell<settings_model::Scope>,
}

/// Every language a row could be about: the editor's own languages that a
/// file can actually open in, under the ids the *protocol* uses, plus
/// whatever the server catalog adds.
fn server_page_languages() -> Vec<(String, String)> {
    syntax_core::registry()
        .languages()
        .into_iter()
        .filter(|language| settings_model::can_have_server(*language))
        .map(|language| {
            (
                settings_model::lsp_language_id(&language.id()).to_string(),
                language.name(),
            )
        })
        .collect()
}

impl ffi::LanguageServerEditor {
    /// Load the draft from `scope` — `"global"` or `"project"`.
    ///
    /// A project's server block is the same shape as the global one on
    /// purpose (ADR-0022), so the same draft type reads both: the project's
    /// list is lifted into an otherwise-default `Settings` and lowered back
    /// out on commit.
    pub fn begin_edit(&self, scope: &QString) {
        let scope = scope_from_name(&scope.to_string());
        *self.scope.borrow_mut() = scope;
        let settings = match scope {
            settings_model::Scope::Project => app_config::Settings {
                language_servers: crate::bridge::convert::load_project_settings()
                    .language_servers
                    .unwrap_or_default(),
                ..app_config::Settings::default()
            },
            _ => app_config::load(&app_core::resolve_config_dir()).unwrap_or_default(),
        };
        let draft = settings_model::ServerDraft::new(
            &settings,
            &server_page_languages(),
            &crate::bridge::language::plugin_servers(),
        );
        *self.saved.borrow_mut() = Some(draft.clone());
        *self.draft.borrow_mut() = Some(draft);
    }

    pub fn rows(&self) -> Vec<ffi::FfiLanguageServerRow> {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return Vec::new();
        };
        draft
            .rows()
            .iter()
            .map(|row| ffi::FfiLanguageServerRow {
                language_id: QString::from(row.language_id.as_str()),
                language_name: QString::from(row.language_name.as_str()),
                command: QString::from(row.command.as_str()),
                args: QString::from(row.args.as_str()),
                enabled: row.enabled,
                status: match row.status() {
                    settings_model::ServerRowStatus::NotConfigured => {
                        ffi::FfiServerRowStatus::NotConfigured
                    }
                    settings_model::ServerRowStatus::Disabled => ffi::FfiServerRowStatus::Disabled,
                    settings_model::ServerRowStatus::Enabled => ffi::FfiServerRowStatus::Enabled,
                },
            })
            .collect()
    }

    pub fn set_command(&self, language_id: &QString, command: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_command(&language_id.to_string(), &command.to_string());
        }
    }

    pub fn set_args(&self, language_id: &QString, args: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_args(&language_id.to_string(), &args.to_string());
        }
    }

    pub fn set_enabled(&self, language_id: &QString, enabled: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_enabled(&language_id.to_string(), enabled);
        }
    }

    pub fn is_dirty(&self, language_id: &QString) -> bool {
        let language_id = language_id.to_string();
        let draft = self.draft.borrow();
        let saved = self.saved.borrow();
        match (draft.as_ref(), saved.as_ref()) {
            (Some(draft), Some(saved)) => draft.row(&language_id) != saved.row(&language_id),
            _ => false,
        }
    }

    pub fn commit(&self) {
        let Some(draft) = self.draft.borrow().clone() else {
            return;
        };
        if *self.scope.borrow() == settings_model::Scope::Project {
            let mut lowered = app_config::Settings::default();
            draft.apply_to(&mut lowered);
            let servers = lowered.language_servers.clone();
            let _ = commit_to_project(move |project| {
                project.language_servers = Some(servers);
            });
            *self.saved.borrow_mut() = Some(draft);
            return;
        }
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        draft.apply_to(&mut settings);
        let _ = app_config::save(&config_dir, &settings);
        *self.saved.borrow_mut() = Some(draft);
    }
}

/// Rust side of the `AiProviderEditor` QObject — the same draft-and-commit
/// shape as `LanguageServerEditor`, plus the tool-policy table, which
/// `settings_model::ai` keeps on `Settings` rather than on the draft.
#[derive(Default)]
pub struct AiProviderEditorRust {
    draft: RefCell<Option<settings_model::ai::AiProviderDraft>>,
    /// The policies as the page has them, applied to settings on commit.
    policies: RefCell<HashMap<String, settings_model::ai::ToolPolicy>>,
    /// Each row's last catalogue fetch, keyed by provider id. A row that
    /// has never been asked is absent.
    ///
    /// ponytail: per-dialog cache with no TTL; the dialog is short-lived,
    /// and `beginEdit` clears it.
    models: RefCell<HashMap<String, Result<Vec<ModelInfo>, ChatError>>>,
    /// Rows with a fetch in flight, so opening the same cell twice does not
    /// start two requests.
    fetching: RefCell<HashMap<String, ()>>,
}

/// The draft row as `ai-chat-core` wants it, so a fetch uses what the user
/// has typed rather than what is saved.
fn row_config(row: &settings_model::ai::AiProviderRow) -> Result<ProviderConfig, ChatError> {
    Ok(ProviderConfig {
        id: row.label.clone(),
        kind: ProviderKind::from_str(&row.kind)?,
        base_url: row.base_url.clone(),
        model: row.model.clone(),
        api_key_env: row.api_key_env.clone(),
        enabled: true,
    })
}

impl ffi::AiProviderEditor {
    pub fn begin_edit(&self) {
        let settings = load_settings();
        *self.policies.borrow_mut() = settings_model::ai::known_tools()
            .map(|tool| {
                (
                    tool.to_string(),
                    settings_model::ai::tool_policy(&settings, tool),
                )
            })
            .collect();
        *self.draft.borrow_mut() = Some(settings_model::ai::AiProviderDraft::begin(&settings));
        self.models.borrow_mut().clear();
    }

    pub fn rows(&self) -> Vec<ffi::FfiAiProviderRow> {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return Vec::new();
        };
        draft
            .rows()
            .iter()
            .map(|row| {
                let status = row.key_status();
                ffi::FfiAiProviderRow {
                    id: QString::from(row.id.as_str()),
                    label: QString::from(row.label.as_str()),
                    kind: QString::from(row.kind.as_str()),
                    base_url: QString::from(row.base_url.as_str()),
                    model: QString::from(row.model.as_str()),
                    key_env_var: QString::from(row.api_key_env.as_str()),
                    enabled: row.enabled,
                    key_present: status == settings_model::ai::KeyStatus::Present,
                    // The sentence is `settings_model`'s; the page shows it
                    // verbatim and never composes one (ADR-0002).
                    status: QString::from(status.sentence().as_str()),
                }
            })
            .collect()
    }

    pub fn tool_policies(&self) -> Vec<ffi::FfiAiToolPolicyRow> {
        let policies = self.policies.borrow();
        settings_model::ai::known_tools()
            .map(|tool| ffi::FfiAiToolPolicyRow {
                tool: QString::from(tool),
                policy: QString::from(
                    policies
                        .get(tool)
                        .copied()
                        .unwrap_or_else(|| settings_model::ai::default_tool_policy(tool))
                        .as_str(),
                ),
                // The read/write split is `ai-chat-core`'s catalog, so the
                // page groups rows without an `if` in C++ deciding which
                // tool changes the project.
                writes: tools::spec(tool).is_some_and(|spec| spec.kind == tools::ToolKind::Write),
            })
            .collect()
    }

    pub fn set_base_url(&self, id: &QString, base_url: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_base_url(&id.to_string(), &base_url.to_string());
        }
    }

    pub fn set_model(&self, id: &QString, model: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_model(&id.to_string(), &model.to_string());
        }
    }

    pub fn models(&self, id: &QString) -> Vec<ffi::FfiAiModel> {
        match self.models.borrow().get(&id.to_string()) {
            Some(Ok(models)) => models
                .iter()
                .map(|model| ffi::FfiAiModel {
                    id: QString::from(model.id.as_str()),
                    label: QString::from(model.label.as_str()),
                })
                .collect(),
            _ => Vec::new(),
        }
    }

    pub fn models_status(&self, id: &QString) -> QString {
        let id = id.to_string();
        match self.models.borrow().get(&id) {
            Some(result) => QString::from(models::models_status(result).as_str()),
            None if self.fetching.borrow().contains_key(&id) => {
                QString::from("Asking the provider…")
            }
            None => QString::from("No models listed yet."),
        }
    }

    pub fn fetch_models(mut self: Pin<&mut Self>, id: &QString) {
        let id = id.to_string();
        if self.fetching.borrow().contains_key(&id) {
            return;
        }
        let config = {
            let draft = self.draft.borrow();
            let row = draft
                .as_ref()
                .and_then(|draft| draft.rows().iter().find(|row| row.id == id).cloned());
            match row {
                Some(row) => row_config(&row),
                None => return,
            }
        };
        let config = match config {
            Ok(config) => config,
            Err(error) => {
                self.models.borrow_mut().insert(id.clone(), Err(error));
                self.as_mut().models_changed(QString::from(id.as_str()));
                return;
            }
        };
        self.fetching.borrow_mut().insert(id.clone(), ());
        self.as_mut().models_changed(QString::from(id.as_str()));

        let qt_thread = self.as_mut().qt_thread();
        // Blocking HTTP on its own thread (ADR-0021 §4): a settings dialog
        // that freezes while a provider thinks is a settings dialog nobody
        // opens twice.
        std::thread::spawn(move || {
            let fetched = models::list_models(&config);
            let _ = qt_thread.queue(move |editor: Pin<&mut Self>| {
                editor.finish_model_fetch(id, fetched);
            });
        });
    }

    /// Lands a catalogue fetch back on the Qt thread.
    fn finish_model_fetch(
        mut self: Pin<&mut Self>,
        id: String,
        fetched: Result<Vec<ModelInfo>, ChatError>,
    ) {
        self.fetching.borrow_mut().remove(&id);
        self.models.borrow_mut().insert(id.clone(), fetched);
        self.as_mut().models_changed(QString::from(id.as_str()));
    }

    pub fn set_key_env_var(&self, id: &QString, key_env_var: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_key_env_var(&id.to_string(), &key_env_var.to_string());
        }
    }

    pub fn set_enabled(&self, id: &QString, enabled: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.set_enabled(&id.to_string(), enabled);
        }
    }

    pub fn set_tool_policy(&self, tool: &QString, policy: &QString) {
        // An unrecognised spelling is dropped rather than defaulted: silently
        // reading an unreadable policy as `Auto` would widen the agent's
        // authority on a typo.
        if let Some(policy) = settings_model::ai::ToolPolicy::parse(&policy.to_string()) {
            self.policies.borrow_mut().insert(tool.to_string(), policy);
        }
    }

    pub fn is_dirty(&self, id: &QString) -> bool {
        match self.draft.borrow().as_ref() {
            Some(draft) => draft.is_dirty(&id.to_string()),
            None => false,
        }
    }

    pub fn validate(&self) -> FfiResult {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return FfiResult::default();
        };
        match draft.validate_all() {
            Ok(()) => FfiResult::default(),
            Err(problem) => FfiResult {
                code: errors::CODE_REFUSED,
                message: QString::from(problem.sentence.as_str()),
            },
        }
    }

    pub fn commit(&self) -> FfiResult {
        let refusal = self.validate();
        if refusal.code != 0 {
            return refusal;
        }
        let draft = self.draft.borrow().clone();
        let Some(draft) = draft else {
            return FfiResult::default();
        };
        let config_dir = app_core::resolve_config_dir();
        let policies = self.policies.borrow().clone();
        match app_config::update(&config_dir, |settings| {
            draft.commit(settings);
            for (tool, policy) in policies.iter() {
                settings_model::ai::set_tool_policy(settings, tool, *policy);
            }
        }) {
            Ok(()) => FfiResult::default(),
            Err(error) => FfiResult {
                code: errors::CODE_SETTINGS_IO,
                message: QString::from(error.to_string().as_str()),
            },
        }
    }

    pub fn revert(&self) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.revert();
        }
        let settings = load_settings();
        *self.policies.borrow_mut() = settings_model::ai::known_tools()
            .map(|tool| {
                (
                    tool.to_string(),
                    settings_model::ai::tool_policy(&settings, tool),
                )
            })
            .collect();
    }
}

/// Every language a per-language editing override could be about: the
/// editor's own registry, under the ids `settings-model` speaks (its own,
/// not the LSP ones `LanguageServerEditor` uses — this page is not talking
/// to a server).
fn editing_page_languages() -> Vec<(String, String)> {
    syntax_core::registry()
        .languages()
        .into_iter()
        .filter(|language| *language != syntax_core::Language::PLAIN_TEXT)
        .map(|language| (language.id(), language.name()))
        .collect()
}

fn to_ffi_editing_row(
    language_id: &str,
    language_name: &str,
    settings: &app_config::editing::EditingSettings,
) -> FfiEditingRow {
    FfiEditingRow {
        language_id: QString::from(language_id),
        language_name: QString::from(language_name),
        tab_width: settings.tab_width,
        has_use_spaces: settings.use_spaces.is_some(),
        use_spaces: settings.use_spaces.unwrap_or(false),
        has_trim_trailing_whitespace: settings.trim_trailing_whitespace.is_some(),
        trim_trailing_whitespace: settings.trim_trailing_whitespace.unwrap_or(false),
        has_insert_final_newline: settings.insert_final_newline.is_some(),
        insert_final_newline: settings.insert_final_newline.unwrap_or(false),
        has_wrap_column: settings.wrap_column.is_some(),
        wrap_column: settings.wrap_column.unwrap_or(0),
        default_encoding: QString::from(settings.default_encoding.as_str()),
        line_endings: QString::from(settings.line_endings.as_str()),
    }
}

fn from_ffi_editing_row(row: &FfiEditingRow) -> app_config::editing::EditingSettings {
    app_config::editing::EditingSettings {
        tab_width: row.tab_width,
        use_spaces: row.has_use_spaces.then_some(row.use_spaces),
        trim_trailing_whitespace: row
            .has_trim_trailing_whitespace
            .then_some(row.trim_trailing_whitespace),
        insert_final_newline: row
            .has_insert_final_newline
            .then_some(row.insert_final_newline),
        wrap_column: row.has_wrap_column.then_some(row.wrap_column),
        default_encoding: row.default_encoding.to_string(),
        line_endings: row.line_endings.to_string(),
        languages: HashMap::new(),
    }
}

fn to_ffi_editing_problem(problem: &settings_model::editing::EditingProblem) -> FfiEditingProblem {
    FfiEditingProblem {
        language_id: QString::from(problem.language_id.as_deref().unwrap_or_default()),
        sentence: QString::from(problem.sentence.as_str()),
    }
}

/// Rust side of the `EditingEditor` QObject (F1-14, F1-17): the Settings >
/// Editing page's draft. Isomorphic to `LanguageServerEditor` — begin, edit,
/// validate, commit — because they answer the same shape of question:
/// "what does each language do differently, and did the user change it."
#[derive(Default)]
pub struct EditingEditorRust {
    draft: RefCell<Option<settings_model::editing::EditingDraft>>,
    /// Which layer this draft was loaded from, and therefore the one
    /// `commit` writes back to. Held rather than re-read on commit so a
    /// scope switch mid-dialog cannot save one layer's draft into the other.
    scope: RefCell<settings_model::Scope>,
}

impl ffi::EditingEditor {
    /// Load the draft from `scope` — `"global"` or `"project"`. Called again
    /// whenever the scope selector changes, which discards the draft that
    /// belonged to the other layer, exactly as Cancel would.
    pub fn begin_edit(&self, scope: &QString) {
        let scope = scope_from_name(&scope.to_string());
        *self.scope.borrow_mut() = scope;
        // The project layer holds a bare `[editing]` section, and
        // `EditingDraft` speaks `Settings`, so the section is lifted into an
        // otherwise-default `Settings` to be edited and lowered back out on
        // commit. Cheaper than a second draft type that would have to be
        // kept in step with the first one forever.
        let settings = match scope {
            settings_model::Scope::Project => app_config::Settings {
                editing: crate::bridge::convert::load_project_settings()
                    .editing
                    .unwrap_or_default(),
                ..app_config::Settings::default()
            },
            _ => load_settings(),
        };
        *self.draft.borrow_mut() = Some(settings_model::editing::EditingDraft::from_settings(
            &settings,
        ));
    }

    pub fn global_row(&self) -> FfiEditingRow {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return to_ffi_editing_row("", "", &app_config::editing::EditingSettings::default());
        };
        to_ffi_editing_row("", "", draft.global())
    }

    pub fn set_global_row(&self, row: &FfiEditingRow) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            *draft.global_mut() = from_ffi_editing_row(row);
        }
    }

    /// Every language with an override, plus the ones without one, so the
    /// page can offer every language and show which already differ. The
    /// registry order (not `languages()`'s sort) is what the picker shows.
    pub fn language_rows(&self) -> Vec<FfiEditingRow> {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return Vec::new();
        };
        editing_page_languages()
            .into_iter()
            .map(|(id, name)| {
                let settings = draft.language(&id).cloned().unwrap_or_default();
                to_ffi_editing_row(&id, &name, &settings)
            })
            .collect()
    }

    pub fn set_language_row(&self, row: &FfiEditingRow) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            let id = row.language_id.to_string();
            draft.set_language(&id, from_ffi_editing_row(row));
        }
    }

    /// What the resolved rules would be for `language_id` if the draft were
    /// saved right now — the preview row's tab width and spaces-vs-tabs.
    pub fn resolved_tab_width(&self, language_id: &QString) -> u32 {
        let draft = self.draft.borrow();
        draft
            .as_ref()
            .map(|draft| draft.resolved(&language_id.to_string()).tab_width as u32)
            .unwrap_or(4)
    }

    /// Everything the page has to say out loud before it may commit, in the
    /// order the user should be walked through it.
    pub fn problems(&self) -> Vec<FfiEditingProblem> {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return Vec::new();
        };
        draft
            .validate()
            .iter()
            .map(to_ffi_editing_problem)
            .collect()
    }

    /// Refuses when [`problems`](Self::problems) is non-empty — a setting
    /// that parses and then does nothing is worse than one the dialog
    /// refused to save, per the page's own rule.
    pub fn commit(&self) -> FfiResult {
        let Some(draft) = self.draft.borrow().clone() else {
            return FfiResult::default();
        };
        if !draft.validate().is_empty() {
            return FfiResult {
                code: errors::CODE_REFUSED,
                message: QString::from("Fix the highlighted editing settings first."),
            };
        }
        if *self.scope.borrow() == settings_model::Scope::Project {
            return commit_to_project(|project| {
                let mut section = app_config::Settings::default();
                draft.apply_to(&mut section);
                // An override that says nothing is removed rather than
                // written as an empty section: `.ide/settings.toml` is
                // reviewed by people, and a section that overrides nothing
                // reads as one that does.
                project.editing = Some(section.editing);
            });
        }
        let config_dir = app_core::resolve_config_dir();
        match app_config::update(&config_dir, |settings| draft.apply_to(settings)) {
            Ok(()) => FfiResult::default(),
            Err(error) => FfiResult {
                code: errors::CODE_SETTINGS_IO,
                message: QString::from(error.to_string().as_str()),
            },
        }
    }
}

/// Write one project-scoped change into `<project>/.ide/settings.toml`.
///
/// Goes through `app_config::project_settings::update`, which brings the
/// atomic write, the refusal to save over a file it could not read, and the
/// unknown-key round trip with it (ADR-0022 §5, §6) — none of which the
/// adapter should be re-implementing.
/// `KEY=VALUE` lines, the convention `FfiRunConfig::env` already crosses
/// the seam with — a `Vec` field on a shared struct is not a shape cxx
/// supports, and a second one-field wrapper type for what is already a
/// text area in the view would buy nothing.
fn to_ffi_terminal_settings(terminal: &app_config::TerminalSettings) -> ffi::FfiTerminalSettings {
    let env: Vec<String> = terminal
        .env
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    ffi::FfiTerminalSettings {
        shell_id: QString::from(terminal.shell_id.as_str()),
        shell_path: QString::from(terminal.shell_path.as_str()),
        shell_args: QString::from(terminal.shell_args.as_str()),
        start_directory: QString::from(terminal.start_directory.as_str()),
        env: QString::from(env.join("\n").as_str()),
    }
}

fn from_ffi_terminal_settings(row: &ffi::FfiTerminalSettings) -> app_config::TerminalSettings {
    app_config::TerminalSettings {
        shell_id: row.shell_id.to_string().trim().to_string(),
        shell_path: row.shell_path.to_string().trim().to_string(),
        shell_args: row.shell_args.to_string().trim().to_string(),
        start_directory: row.start_directory.to_string().trim().to_string(),
        env: parse_env_lines(&row.env.to_string()),
    }
}

/// One `KEY=VALUE` per line. A line with no `=`, or an empty key, is
/// dropped rather than stored as a variable with no name — the view has a
/// free-text area and a half-typed line is a normal thing to find in it.
fn parse_env_lines(text: &str) -> std::collections::BTreeMap<String, String> {
    text.lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty())
        .collect()
}

fn commit_to_project(
    edit: impl FnOnce(&mut app_config::project_settings::ProjectSettings),
) -> FfiResult {
    let Some(root) = crate::bridge::convert::current_project_root() else {
        return errors::failure(
            errors::CODE_NO_PROJECT,
            "Open a project before editing its settings — project settings live in the project.",
        );
    };
    match app_config::project_settings::update(&root, edit) {
        Ok(()) => FfiResult::default(),
        Err(error) => errors::failure(errors::CODE_SETTINGS_IO, error.to_string()),
    }
}
