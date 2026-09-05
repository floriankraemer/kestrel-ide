//! Structured application settings: theme, editor font/colors, recent
//! projects, and window geometry/state, persisted as TOML.
//!
//! No Qt dependency — pure Rust, unit-testable. This crate is independent of
//! `project-model`, which keeps its own separate single-line
//! `last-project.txt` persistence for the last-opened-project path (decision
//! A7); that mechanism is untouched by this crate. `ui-shell` reads/writes
//! [`Settings`] via [`load`]/[`save`] and drives a settings dialog around it.

use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::io;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// The `[editing]` section: indentation, wrapping, and save behaviour.
pub mod editing;
pub mod keymap;
pub mod syntax_colors;
/// The `[terminal]` section: which shell the embedded terminal spawns.
pub mod terminal;

/// Per-project settings layered over the global file (ADR-0022).
pub mod project_settings;

/// Machine-local VCS preferences, layered under `.ide/local/` rather than
/// the committed project settings file.
pub mod vcs_local_settings;

/// Machine-local breakpoints, under `.ide/local/` for the same reason
/// (D2-4).
pub mod breakpoint_settings;

/// What a launch is made of: run configurations, their before-launch tasks,
/// and debug adapter overrides. Split out of this file when it reached its
/// size ceiling; the three belong together anyway.
pub mod launch_settings;

pub use editing::EditingSettings;
pub use keymap::{action, ActionDef, Binding, Keymap, ACTIONS};
pub use launch_settings::{BeforeLaunchSetting, DebugAdapterSetting, RunConfigSetting};
pub use syntax_colors::{LanguageScopeStyles, ScopeStyle, ScopeStyles};
pub use terminal::TerminalSettings;

/// File name used to persist settings inside the config directory.
const SETTINGS_FILE: &str = "settings.toml";
/// Where [`save`] stages the new content before renaming it over
/// [`SETTINGS_FILE`]. Same directory, so the rename stays within one
/// filesystem and is therefore atomic.
const TEMP_SETTINGS_FILE: &str = "settings.toml.tmp";

/// Window position and size, as last saved by the view (`QMainWindow`
/// geometry). Every field is individually defaulted so a TOML file that only
/// sets some of them still parses.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct WindowGeometry {
    #[serde(default)]
    pub x: i32,
    #[serde(default)]
    pub y: i32,
    #[serde(default)]
    pub width: u32,
    #[serde(default)]
    pub height: u32,
}

impl WindowGeometry {
    /// Whether this geometry is worth persisting or restoring. A zero-sized
    /// rect is what the window reports while it is minimised or already torn
    /// down, and restoring it next launch would open a window nobody can see.
    pub fn is_usable(&self) -> bool {
        self.width > 0 && self.height > 0
    }
}

/// One `[[language_server]]` entry: what the user says about the language
/// server for one language id.
///
/// Every field but `language_id` is optional, so `enabled = false` alone
/// switches a shipped server off without wiping its command. This mirrors
/// `lsp_core::ServerOverride` field for field but is declared here so the
/// config crate keeps no dependency on the LSP client (ADR-0016) — `ui-shell`
/// maps one to the other at the seam.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct LanguageServerSetting {
    /// LSP language id, e.g. `"rust"`. The key both the shipped catalog and
    /// this table are keyed by.
    #[serde(default)]
    pub language_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

/// One `[[ai_provider]]` entry: what the user says about one AI chat
/// provider.
///
/// Every field is defaulted so a `settings.toml` written before AI chat
/// existed still loads, and so `enabled = false` alone switches a shipped
/// provider off without wiping the rest of its configuration.
///
/// `kind` is a plain `String` here on purpose. This crate stores a provider
/// kind exactly the way it stores a language id: as an opaque string it
/// never interprets, so a kind a newer build understands survives a
/// load/save cycle in an older one. What the four kinds *mean* is
/// `settings_model::ai`'s business (ADR-0017), and the dialect behind each
/// is `ai-chat-core`'s.
///
/// **There is deliberately no API-key field, and there must never be one.**
/// The IDE never writes a key to disk. `api_key_env` holds the *name* of an
/// environment variable, and the key itself is read with `std::env::var` at
/// request time. A future reader who "fixes" the missing field by adding
/// `api_key: String` would move every user's secret into a plain-text file
/// under the config directory.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AiProviderSetting {
    /// Stable id of the provider entry, e.g. `"anthropic"`. The key both the
    /// default catalog and this table are keyed by.
    #[serde(default)]
    pub id: String,
    /// Provider dialect, e.g. `"anthropic"`, `"openai"`,
    /// `"openai-compatible"`, `"gemini"`. Opaque to this crate.
    #[serde(default)]
    pub kind: String,
    /// API base URL. Empty means "use the kind's default", which is what an
    /// OpenAI-compatible endpoint (Ollama, Groq, vLLM) overrides.
    #[serde(default)]
    pub base_url: String,
    /// Model id sent with each request, e.g. `"claude-sonnet-4-5"`.
    #[serde(default)]
    pub model: String,
    /// Name of the environment variable the API key is read from — never the
    /// key. Empty is legitimate: a local endpoint needs no key at all.
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default)]
    pub enabled: bool,
}

/// One `[[ai_tool_policy]]` entry: how far the agent may go with one tool.
///
/// `policy` is one of `auto`, `ask`, `never`, as a plain string for the same
/// reason `AiProviderSetting::kind` is: this crate stores the vocabulary, it
/// does not own it. The read/write classification that decides the *default*
/// for a tool with no entry here lives in `settings_model::ai` (ADR-0017),
/// so a tool added to the catalog needs no change in this crate.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct AiToolPolicySetting {
    /// Tool name as the tool catalog spells it, e.g. `"edit_buffer"`.
    #[serde(default)]
    pub tool: String,
    #[serde(default)]
    pub policy: String,
}

pub(crate) fn is_false(b: &bool) -> bool {
    !*b
}

/// Structured application settings, round-tripped to `settings.toml` in the
/// config directory. Every field is `#[serde(default)]` so old or partially
/// written settings files still parse.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Default)]
pub struct Settings {
    #[serde(default)]
    pub theme: String,
    /// Id of the `icon-themes` contribution whose pack draws file and folder
    /// icons. Empty means "never chosen", which resolves to the first icon
    /// theme the loaded plugins offer — so a fresh install gets icons without
    /// the user first finding a setting.
    ///
    /// Global rather than per-project, like [`Settings::theme`]: per-project
    /// settings deliberately exclude theme-like choices, see
    /// [`project_settings`].
    #[serde(default)]
    pub icon_theme: String,
    #[serde(default)]
    pub editor_font_size: u32,
    #[serde(default)]
    pub editor_font_family: String,
    /// Interface font scale in percent for every piece of chrome that has no
    /// scale of its own (tabs, docks, dialogs, status bar). `0` means "never
    /// chosen", which resolves to [`DEFAULT_UI_FONT_SCALE`]. Read through
    /// [`Settings::ui_font_scale_or_default`].
    #[serde(default)]
    pub ui_font_scale: u32,
    /// Interface font scale in percent for the project tree sidebar,
    /// independent of [`Settings::ui_font_scale`].
    #[serde(default)]
    pub project_tree_font_scale: u32,
    /// Project tree sort direction: `false` (default) sorts folders then
    /// files, each group ascending; `true` reverses the name comparison
    /// within each group. A bare `bool` is correct here, unlike
    /// `mcp_enabled` below — "never chosen" and "ascending" are the same
    /// thing, so there is no default-vs-unset distinction to preserve.
    #[serde(default)]
    pub project_tree_sort_descending: bool,
    /// JetBrains-style "show whitespace characters": the master toggle.
    /// Off by default, like `project_tree_sort_descending` above — "never
    /// chosen" and "off" are the same thing here, so a bare `bool` is
    /// correct and there is no default-vs-unset distinction to preserve.
    #[serde(default)]
    pub show_whitespace: bool,
    /// Paint leading whitespace (before the first non-whitespace character
    /// on a line) when [`Settings::show_whitespace`] is on.
    #[serde(default)]
    pub show_whitespace_leading: bool,
    /// Paint inner whitespace (between two non-whitespace characters) when
    /// [`Settings::show_whitespace`] is on.
    #[serde(default)]
    pub show_whitespace_inner: bool,
    /// Paint trailing whitespace (after the last non-whitespace character)
    /// when [`Settings::show_whitespace`] is on.
    #[serde(default)]
    pub show_whitespace_trailing: bool,
    /// Paint a marker at the end of every line. Independent of
    /// `show_whitespace`: a line ending isn't a space or a tab, so this can
    /// be on with the master toggle off (or vice versa).
    #[serde(default)]
    pub show_eol_markers: bool,
    /// Interface font scale in percent for the menu bar and its popup menus,
    /// independent of [`Settings::ui_font_scale`].
    #[serde(default)]
    pub menu_font_scale: u32,
    /// Whether the MCP server listens at all. `None` means "never chosen",
    /// which resolves to [`DEFAULT_MCP_ENABLED`] — a bare `bool` would make
    /// the derived `Default` say "off" and silently disable the server for
    /// everyone whose `settings.toml` predates this field.
    /// Read through [`Settings::mcp_enabled_or_default`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_enabled: Option<bool>,
    /// TCP port the MCP server binds on `127.0.0.1`. `0` means "let the OS
    /// assign one" (ADR-0004's multi-instance property); any other value is
    /// bound exactly, and a bind failure is reported rather than silently
    /// falling back.
    #[serde(default)]
    pub mcp_port: u16,
    /// Color name (e.g. "background", "foreground") to hex string (e.g.
    /// "#1e1e1e"). Kept intentionally simple — a richer color model is the
    /// Editor settings category's job, not this crate's.
    #[serde(default)]
    pub editor_colors: HashMap<String, String>,
    /// Field + type only: no push/dedupe/most-recent-first/max-length
    /// manipulation here, that's a separate task built on top of this crate.
    #[serde(default)]
    pub recent_projects: Vec<PathBuf>,
    /// Most-recently-opened files, newest first — what Search Everywhere
    /// shows before anything has been typed. Maintained by
    /// [`Settings::push_recent_file`].
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    #[serde(default)]
    pub window_geometry: WindowGeometry,
    /// Opaque persisted layout blob, analogous to `QMainWindow::saveState()`.
    #[serde(default)]
    pub window_state: String,
    /// Opaque persisted editor split layout: the tab-group splitter tree plus
    /// the files open in each group, serialized by the view (same
    /// view-owns-the-format arrangement as `window_state` above).
    #[serde(default)]
    pub editor_layout: String,
    /// Keyboard shortcut overrides: action id to `QKeySequence` portable text
    /// (`""` = deliberately unbound). Only overrides live here — an action
    /// absent from the map uses the default from [`keymap::ACTIONS`], so
    /// changing a shipped default still reaches users who never rebound it.
    /// See [`Keymap`] for the rules layered over this map.
    #[serde(default)]
    pub keymap: HashMap<String, String>,
    /// Base syntax colors: scope name -> style, applying to every language.
    ///
    /// Per-language overrides deliberately live in a *separate* top-level
    /// table rather than nesting under `[syntax_colors.<lang>]`: in TOML both
    /// a table-form style (`comment = { fg = "…" }`) and a language sub-table
    /// (`[syntax_colors.python]`) are just tables under the same key, so
    /// nothing in the syntax tells them apart. Disambiguating would mean
    /// guessing from the inner keys — fragile the moment a language id
    /// collides with a scope name, or a scope table uses a key a future style
    /// field also uses. Two flat maps parse unambiguously and cost one extra
    /// TOML header.
    ///
    /// Dotted scope names may be written either quoted
    /// (`"function.method" = "…"`) or as the nested table TOML makes of a
    /// bare dotted key — see [`syntax_colors::deserialize_scope_styles`].
    #[serde(default, deserialize_with = "syntax_colors::deserialize_scope_styles")]
    pub syntax_colors: ScopeStyles,
    /// Per-language syntax color overrides: language id -> (scope name ->
    /// style). Layered over [`Settings::syntax_colors`] by the theme
    /// resolution in `syntax-core` — this crate only stores them.
    #[serde(
        default,
        deserialize_with = "syntax_colors::deserialize_language_scope_styles"
    )]
    pub syntax_colors_by_language: LanguageScopeStyles,
    /// Per-language language-server overrides, written as `[[language_server]]`
    /// blocks. Layered over the shipped catalog by `lsp_core::resolve_servers`;
    /// this crate only stores them.
    #[serde(default, rename = "language_server")]
    pub language_servers: Vec<LanguageServerSetting>,
    /// AI chat providers, written as `[[ai_provider]]` blocks. Only entries
    /// that differ from the default catalog are written, so changing a
    /// shipped default still reaches a user who never touched it — the same
    /// rule the keymap and `[[language_server]]` follow.
    #[serde(default, rename = "ai_provider")]
    pub ai_providers: Vec<AiProviderSetting>,
    /// Id of the provider AI chat sends to. Empty means "never chosen"; the
    /// rules layer picks the first usable entry.
    #[serde(default)]
    pub ai_active_provider: String,
    /// Per-tool agent policies, written as `[[ai_tool_policy]]` blocks. A
    /// tool absent from this list uses the default from
    /// `settings_model::ai::tool_policy`.
    #[serde(default, rename = "ai_tool_policy")]
    pub ai_tool_policies: Vec<AiToolPolicySetting>,
    /// `"ask"` or `"agent"` — whether the panel only answers, or runs the
    /// tool loop. Empty means "never chosen".
    #[serde(default)]
    pub ai_mode: String,
    /// Whether transcripts are written to disk at all. `None` means "never
    /// chosen", which resolves to [`DEFAULT_AI_PERSIST_CONVERSATIONS`], for
    /// the same reason `mcp_enabled` is an `Option`: a bare `bool` would make
    /// the derived `Default` say "off" and silently stop persisting for
    /// everyone whose `settings.toml` predates this field.
    /// Read through [`Settings::ai_persist_conversations_or_default`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ai_persist_conversations: Option<bool>,
    /// How buffers indent, wrap and are written back to disk, with
    /// per-language overrides underneath. Which of those a language may
    /// actually override is `settings_model::editing`'s rule, not this
    /// crate's — see [`editing`].
    #[serde(default)]
    pub editing: EditingSettings,
    /// Which shell the embedded terminal spawns, where it starts, and what
    /// it adds to the environment. Project-scoped like [`Settings::editing`]
    /// — see [`terminal`] for why the shell belongs to the checkout at least
    /// as often as to the person.
    #[serde(default)]
    pub terminal: TerminalSettings,
    /// Gitignore-syntax patterns the project index skips, on top of the
    /// `.gitignore` rules its walker already honours.
    ///
    /// Global here means "the user's own habitual excludes" — a scratch
    /// directory they keep in every checkout. The project's own excludes
    /// live in [`project_settings::ProjectSettings::index_excludes`], and
    /// which of the two applies is `settings_model::scope`'s answer, not
    /// this crate's.
    #[serde(default)]
    pub index_excludes: Vec<String>,
    /// Stable ids of languages the user turned off. A disabled language is
    /// still *listed* by the Languages page — otherwise it could never be
    /// switched back on — but the registry refuses to resolve it, so its
    /// files open as plain text. Ids, not names: names are display-only.
    #[serde(default)]
    pub disabled_languages: Vec<String>,
    /// Ids of plugins the user turned off. Filtered out by
    /// `plugin_host::load`, which never even opens the manifest of a
    /// disabled plugin — so a *broken* plugin the user disabled stops
    /// reporting its error too.
    ///
    /// Plugin ids, not contribution ids: disabling is a statement about the
    /// whole plugin, and one plugin can contribute several things.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
}

/// Cap on remembered recent projects — enough for a useful menu without
/// growing unbounded.
const MAX_RECENT_PROJECTS: usize = 10;

/// Cap on remembered recent files. Larger than the project cap because this
/// list is a search surface, not a menu.
const MAX_RECENT_FILES: usize = 50;

/// Theme name used when `Settings::theme` hasn't been set yet (T2).
const DEFAULT_THEME: &str = "dark";

/// Editor font used when `Settings::editor_font_family`/`_size` haven't
/// been set yet (S2).
const DEFAULT_EDITOR_FONT_FAMILY: &str = "Monospace";
const DEFAULT_EDITOR_FONT_SIZE: u32 = 11;

/// Interface font scale used when the user has never chosen one: the
/// platform's own default UI font size, unscaled.
pub const DEFAULT_UI_FONT_SCALE: u32 = 100;

/// Bounds on an interface font scale. Enforced on read rather than on write
/// so a hand-edited `settings.toml` cannot produce a window whose menus are
/// unreadable or too large to fit the screen.
pub const MIN_UI_FONT_SCALE: u32 = 50;
pub const MAX_UI_FONT_SCALE: u32 = 300;

/// Resolves a stored percentage: `0` (never chosen) becomes the default, and
/// anything else is clamped into [`MIN_UI_FONT_SCALE`]..=[`MAX_UI_FONT_SCALE`].
fn resolve_font_scale(value: u32) -> u32 {
    if value == 0 {
        DEFAULT_UI_FONT_SCALE
    } else {
        value.clamp(MIN_UI_FONT_SCALE, MAX_UI_FONT_SCALE)
    }
}

/// The MCP server is on unless the user turns it off: the IDE's whole point
/// of having one is that an agent can attach to a running instance without
/// the human first hunting for a switch.
const DEFAULT_MCP_ENABLED: bool = true;

/// Conversations are kept unless the user says otherwise: a chat panel that
/// forgets every transcript on restart is a chat panel nobody trusts with a
/// long investigation. The transcripts are written `0600` per project, and
/// the switch is there for the times that is still not enough.
const DEFAULT_AI_PERSIST_CONVERSATIONS: bool = true;

impl Settings {
    /// The active theme name, defaulting to [`DEFAULT_THEME`] when unset —
    /// so the view never has to special-case an empty string itself.
    pub fn theme_name(&self) -> &str {
        if self.theme.is_empty() {
            DEFAULT_THEME
        } else {
            &self.theme
        }
    }

    /// The editor font family, defaulting to [`DEFAULT_EDITOR_FONT_FAMILY`]
    /// when unset.
    pub fn editor_font_family_or_default(&self) -> &str {
        if self.editor_font_family.is_empty() {
            DEFAULT_EDITOR_FONT_FAMILY
        } else {
            &self.editor_font_family
        }
    }

    /// The editor font size, defaulting to [`DEFAULT_EDITOR_FONT_SIZE`]
    /// when unset (0).
    pub fn editor_font_size_or_default(&self) -> u32 {
        if self.editor_font_size == 0 {
            DEFAULT_EDITOR_FONT_SIZE
        } else {
            self.editor_font_size
        }
    }

    /// The interface font scale for general chrome, resolved and clamped.
    pub fn ui_font_scale_or_default(&self) -> u32 {
        resolve_font_scale(self.ui_font_scale)
    }

    /// The interface font scale for the project tree, resolved and clamped.
    pub fn project_tree_font_scale_or_default(&self) -> u32 {
        resolve_font_scale(self.project_tree_font_scale)
    }

    /// The interface font scale for the menu bar, resolved and clamped.
    pub fn menu_font_scale_or_default(&self) -> u32 {
        resolve_font_scale(self.menu_font_scale)
    }

    /// Whether the MCP server should listen, defaulting to
    /// [`DEFAULT_MCP_ENABLED`] when the user has never chosen.
    pub fn mcp_enabled_or_default(&self) -> bool {
        self.mcp_enabled.unwrap_or(DEFAULT_MCP_ENABLED)
    }

    /// Whether AI chat transcripts are persisted, defaulting to
    /// [`DEFAULT_AI_PERSIST_CONVERSATIONS`] when the user has never chosen.
    pub fn ai_persist_conversations_or_default(&self) -> bool {
        self.ai_persist_conversations
            .unwrap_or(DEFAULT_AI_PERSIST_CONVERSATIONS)
    }

    /// The keyboard shortcuts in force, defaults included — the view asks
    /// this for every action's binding rather than resolving fallbacks itself.
    pub fn keymap(&self) -> Keymap {
        Keymap::from_overrides(self.keymap.clone())
    }

    /// Replace the shortcut overrides with `keymap`'s.
    pub fn set_keymap(&mut self, keymap: Keymap) {
        self.keymap = keymap.into_overrides();
    }

    /// Push `path` to the front of `recent_projects`, deduping any existing
    /// entry for it and capping the list at [`MAX_RECENT_PROJECTS`].
    pub fn push_recent_project(&mut self, path: PathBuf) {
        self.recent_projects.retain(|p| p != &path);
        self.recent_projects.insert(0, path);
        self.recent_projects.truncate(MAX_RECENT_PROJECTS);
    }

    /// Push `path` to the front of `recent_files`, deduping any existing
    /// entry for it and capping the list at [`MAX_RECENT_FILES`].
    pub fn push_recent_file(&mut self, path: PathBuf) {
        self.recent_files.retain(|p| p != &path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(MAX_RECENT_FILES);
    }

    /// Turn one language off or back on. Kept sorted and deduped so the
    /// persisted list is stable no matter what order the user toggled in.
    pub fn set_language_disabled(&mut self, id: &str, disabled: bool) {
        self.disabled_languages.retain(|other| other != id);
        if disabled {
            self.disabled_languages.push(id.to_string());
            self.disabled_languages.sort();
        }
    }

    pub fn is_language_disabled(&self, id: &str) -> bool {
        self.disabled_languages.iter().any(|other| other == id)
    }

    /// Turn one plugin off or back on, with the same sorted-and-deduped
    /// persistence [`Settings::set_language_disabled`] gives languages.
    pub fn set_plugin_disabled(&mut self, id: &str, disabled: bool) {
        self.disabled_plugins.retain(|other| other != id);
        if disabled {
            self.disabled_plugins.push(id.to_string());
            self.disabled_plugins.sort();
        }
    }

    pub fn is_plugin_disabled(&self, id: &str) -> bool {
        self.disabled_plugins.iter().any(|other| other == id)
    }
}

/// Why loading or saving settings failed.
#[derive(Debug)]
pub enum ConfigError {
    Io(io::Error),
    Parse(toml::de::Error),
    /// The value could not be turned into TOML. `Settings` cannot hit this,
    /// but a layer carrying arbitrary user keys forward can (see
    /// `project_settings`), and a panic on the save path of a file that has
    /// already caused data loss once is the wrong failure mode.
    Serialize(toml::ser::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::Io(err) => write!(f, "settings I/O error: {err}"),
            ConfigError::Parse(err) => write!(f, "settings file is malformed: {err}"),
            ConfigError::Serialize(err) => {
                write!(f, "settings could not be written: {err}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<io::Error> for ConfigError {
    fn from(err: io::Error) -> Self {
        ConfigError::Io(err)
    }
}

/// The platform config dir the real app persists into (`dirs::config_dir()`
/// joined with `ide`), same convention as `project-model::default_config_dir`.
/// Tests should use their own temp dir instead of this, to avoid touching the
/// developer's real `~/.config`.
pub fn default_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("ide"))
}

// ---------------------------------------------------------------------------
// The path-keyed core. `Settings` (global) and `ProjectSettings` (per project)
// are both persisted through these three functions, so the guarantees below
// hold for both and cannot drift apart:
//
//   * a missing file is not an error — it means nothing has been saved yet;
//   * a malformed file IS an error, and never silently becomes defaults;
//   * the write is atomic (temp file, fsync, rename) so no reader ever sees a
//     half-written file;
//   * read-modify-write aborts on a load failure rather than editing a
//     defaulted value and saving that over the user's real settings.
//
// That last pair is not theoretical: settings.toml was once wiped to defaults
// in exactly this way (see the `save`/`update` docs below).
// ---------------------------------------------------------------------------

/// Load a TOML file into `T`. A missing file yields `T::default()`.
fn load_toml<T: DeserializeOwned + Default>(path: &Path) -> Result<T, ConfigError> {
    let content = match fs::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
        Err(e) => return Err(ConfigError::Io(e)),
    };
    toml::from_str(&content).map_err(ConfigError::Parse)
}

/// Serialize `value` to `path`, creating the parent directory if needed.
///
/// The write is atomic: the content goes to a temporary file beside the real
/// one and is then renamed over it. A plain truncate-then-write leaves a
/// window in which the file is empty or half-written, and anything reading it
/// in that window — another window of the app, the next launch after a crash
/// or a SIGTERM — sees nothing, defaults it, and can then write those defaults
/// back over everything the user configured.
fn save_toml<T: Serialize>(path: &Path, temp_path: &Path, value: &T) -> Result<(), ConfigError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let content = toml::to_string_pretty(value).map_err(ConfigError::Serialize)?;
    // fsync before the rename: the rename can otherwise reach the disk before
    // the content does, which after a power loss leaves an empty file where a
    // complete old one used to be.
    let write_temp = || -> io::Result<()> {
        let mut file = fs::File::create(temp_path)?;
        file.write_all(content.as_bytes())?;
        file.sync_all()
    };
    if let Err(err) = write_temp() {
        let _ = fs::remove_file(temp_path);
        return Err(ConfigError::Io(err));
    }
    if let Err(err) = fs::rename(temp_path, path) {
        let _ = fs::remove_file(temp_path);
        return Err(ConfigError::Io(err));
    }
    Ok(())
}

/// Load, edit, save.
///
/// A load failure aborts the update instead of editing a `T::default()` and
/// saving that: the file on disk holds everything the user configured, so
/// writing defaults over it because it could not be read (or was momentarily
/// unreadable) is data loss, not a fresh start.
fn update_toml<T: DeserializeOwned + Serialize + Default>(
    path: &Path,
    temp_path: &Path,
    edit: impl FnOnce(&mut T),
) -> Result<(), ConfigError> {
    let mut value: T = load_toml(path)?;
    edit(&mut value);
    save_toml(path, temp_path, &value)
}

/// Load settings from `<config_dir>/settings.toml`. A missing file is not an
/// error — it means no settings have been saved yet, so this returns
/// `Settings::default()`. A malformed file is an error.
pub fn load(config_dir: &Path) -> Result<Settings, ConfigError> {
    load_toml(&config_dir.join(SETTINGS_FILE))
}

/// Save `settings` to `<config_dir>/settings.toml`, creating `config_dir` if
/// it doesn't exist yet. Atomic, per [`save_toml`].
pub fn save(config_dir: &Path, settings: &Settings) -> Result<(), ConfigError> {
    save_toml(
        &config_dir.join(SETTINGS_FILE),
        &config_dir.join(TEMP_SETTINGS_FILE),
        settings,
    )
}

/// Load, edit, save — the shape every "change one setting" path needs.
/// Aborts on a load failure rather than defaulting, per [`update_toml`].
pub fn update(config_dir: &Path, edit: impl FnOnce(&mut Settings)) -> Result<(), ConfigError> {
    update_toml(
        &config_dir.join(SETTINGS_FILE),
        &config_dir.join(TEMP_SETTINGS_FILE),
        edit,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_edits_one_field_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            theme: "light".to_string(),
            editor_font_family: "Monospace".to_string(),
            ..Settings::default()
        };
        save(dir.path(), &settings).unwrap();

        update(dir.path(), |settings| {
            settings.window_geometry = WindowGeometry {
                x: 10,
                y: 20,
                width: 900,
                height: 700,
            };
        })
        .unwrap();

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.theme, "light");
        assert_eq!(loaded.editor_font_family, "Monospace");
        assert_eq!(loaded.window_geometry.width, 900);
    }

    #[test]
    fn update_refuses_to_write_defaults_over_an_unreadable_file() {
        // The whole point of the bail-out: a file that cannot be parsed still
        // holds the user's settings, so it must survive the failed update.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(SETTINGS_FILE);
        fs::write(&path, "theme = \"light\"\nthis is not toml").unwrap();

        let err = update(dir.path(), |settings| settings.theme = "dark".to_string()).unwrap_err();

        assert!(matches!(err, ConfigError::Parse(_)));
        assert!(fs::read_to_string(&path)
            .unwrap()
            .contains("this is not toml"));
    }

    #[test]
    fn save_leaves_no_temporary_file_behind() {
        let dir = tempfile::tempdir().unwrap();
        save(dir.path(), &Settings::default()).unwrap();

        assert!(!dir.path().join(TEMP_SETTINGS_FILE).exists());
    }

    #[test]
    fn save_never_exposes_a_half_written_settings_file() {
        // A truncate-then-write save lets a concurrent reader see an empty or
        // partial file and default the settings away; renaming a complete
        // temporary file over the old one cannot.
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings {
            theme: "light".to_string(),
            ..Settings::default()
        };
        settings
            .editor_colors
            .insert("background".to_string(), "#ffffff".to_string());
        save(dir.path(), &settings).unwrap();

        let reader_dir = dir.path().to_path_buf();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let reader_stop = std::sync::Arc::clone(&stop);
        let reader = std::thread::spawn(move || {
            while !reader_stop.load(std::sync::atomic::Ordering::Relaxed) {
                let loaded = load(&reader_dir).expect("settings.toml is always parseable");
                assert_eq!(loaded.theme, "light", "a reader saw defaulted settings");
            }
        });

        for _ in 0..200 {
            save(dir.path(), &settings).unwrap();
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        reader.join().unwrap();
    }

    #[test]
    fn a_zero_sized_window_geometry_is_not_usable() {
        assert!(!WindowGeometry::default().is_usable());
        assert!(!WindowGeometry {
            x: 10,
            y: 10,
            width: 800,
            height: 0,
        }
        .is_usable());
        assert!(WindowGeometry {
            x: 10,
            y: 10,
            width: 800,
            height: 600,
        }
        .is_usable());
    }

    #[test]
    fn mcp_defaults_to_enabled_on_an_os_assigned_port() {
        let settings = Settings::default();
        assert!(settings.mcp_enabled_or_default());
        assert_eq!(settings.mcp_port, 0);
    }

    #[test]
    fn mcp_can_be_turned_off_and_pinned_to_a_port() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            mcp_enabled: Some(false),
            mcp_port: 7337,
            ..Settings::default()
        };

        save(dir.path(), &settings).unwrap();
        let loaded = load(dir.path()).unwrap();

        assert!(!loaded.mcp_enabled_or_default());
        assert_eq!(loaded.mcp_port, 7337);
    }

    #[test]
    fn settings_file_without_mcp_keys_keeps_the_server_enabled() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"light\"\n").unwrap();

        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded.theme_name(), "light");
        assert!(loaded.mcp_enabled_or_default());
        assert_eq!(loaded.mcp_port, 0);
    }

    #[test]
    fn round_trips_non_default_settings() {
        let dir = tempfile::tempdir().unwrap();

        let mut colors = HashMap::new();
        colors.insert("background".to_string(), "#1e1e1e".to_string());

        let settings = Settings {
            theme: "dark".to_string(),
            icon_theme: "material".to_string(),
            disabled_plugins: vec!["noisy-plugin".to_string()],
            editor_font_size: 14,
            editor_font_family: "Fira Code".to_string(),
            ui_font_scale: 130,
            project_tree_font_scale: 150,
            project_tree_sort_descending: true,
            show_whitespace: true,
            show_whitespace_leading: true,
            show_whitespace_inner: false,
            show_whitespace_trailing: true,
            show_eol_markers: true,
            menu_font_scale: 90,
            mcp_enabled: Some(true),
            mcp_port: 7337,
            editor_colors: colors,
            recent_projects: vec![PathBuf::from("/home/user/project-a")],
            recent_files: vec![PathBuf::from("/home/user/project-a/src/main.rs")],
            index_excludes: vec!["scratch/".to_string()],
            window_geometry: WindowGeometry {
                x: 10,
                y: 20,
                width: 1280,
                height: 800,
            },
            editing: EditingSettings {
                tab_width: 2,
                use_spaces: Some(false),
                ..EditingSettings::default()
            },
            terminal: TerminalSettings {
                shell_id: "wsl:Ubuntu".to_string(),
                start_directory: "/srv/checkout".to_string(),
                ..TerminalSettings::default()
            },
            window_state: "opaque-blob".to_string(),
            editor_layout: "{\"groups\":[]}".to_string(),
            keymap: HashMap::from([("view.goToLine".to_string(), "Ctrl+L".to_string())]),
            syntax_colors: HashMap::from([
                (
                    "keyword".to_string(),
                    ScopeStyle::Color("#cc7832".to_string()),
                ),
                (
                    "comment".to_string(),
                    ScopeStyle::Full {
                        fg: Some("#808080".to_string()),
                        bold: false,
                        italic: true,
                        underline: false,
                    },
                ),
            ]),
            syntax_colors_by_language: HashMap::from([(
                "python".to_string(),
                HashMap::from([(
                    "decorator".to_string(),
                    ScopeStyle::Color("#ffc66d".to_string()),
                )]),
            )]),
            language_servers: vec![LanguageServerSetting {
                language_id: "rust".to_string(),
                command: Some("/opt/rust-analyzer".to_string()),
                ..LanguageServerSetting::default()
            }],
            disabled_languages: vec!["vala".to_string()],
            ai_providers: vec![AiProviderSetting {
                id: "local".to_string(),
                kind: "openai_compatible".to_string(),
                base_url: "http://localhost:11434/v1".to_string(),
                model: "qwen2.5-coder".to_string(),
                api_key_env: String::new(),
                enabled: true,
            }],
            ai_active_provider: "local".to_string(),
            ai_tool_policies: vec![AiToolPolicySetting {
                tool: "edit_buffer".to_string(),
                policy: "never".to_string(),
            }],
            ai_mode: "agent".to_string(),
            ai_persist_conversations: Some(false),
        };

        save(dir.path(), &settings).unwrap();
        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded, settings);
    }

    #[test]
    fn keymap_round_trips_through_settings() {
        // Only overrides are persisted; the rest of the catalog keeps its
        // shipped defaults after a save/load cycle.
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        let mut map = settings.keymap();
        map.assign("view.goToLine", "Ctrl+Shift+F");
        settings.set_keymap(map);

        save(dir.path(), &settings).unwrap();
        let loaded = load(dir.path()).unwrap();

        let map = loaded.keymap();
        assert_eq!(map.shortcut_for("view.goToLine"), "Ctrl+Shift+F");
        assert_eq!(map.shortcut_for("edit.findInFiles"), "");
        assert_eq!(map.shortcut_for("file.save"), "Ctrl+S");
    }

    #[test]
    fn push_recent_project_dedupes_and_moves_to_front() {
        let mut settings = Settings::default();
        settings.push_recent_project(PathBuf::from("/a"));
        settings.push_recent_project(PathBuf::from("/b"));
        settings.push_recent_project(PathBuf::from("/a"));

        assert_eq!(
            settings.recent_projects,
            vec![PathBuf::from("/a"), PathBuf::from("/b")]
        );
    }

    #[test]
    fn push_recent_project_caps_at_max() {
        let mut settings = Settings::default();
        for i in 0..(MAX_RECENT_PROJECTS + 5) {
            settings.push_recent_project(PathBuf::from(format!("/project-{i}")));
        }

        assert_eq!(settings.recent_projects.len(), MAX_RECENT_PROJECTS);
        // Most recent push is first.
        assert_eq!(
            settings.recent_projects[0],
            PathBuf::from(format!("/project-{}", MAX_RECENT_PROJECTS + 4))
        );
    }

    #[test]
    fn theme_name_defaults_when_unset() {
        let settings = Settings::default();
        assert_eq!(settings.theme_name(), "dark");
    }

    #[test]
    fn theme_name_returns_the_set_theme() {
        let settings = Settings {
            theme: "light".to_string(),
            ..Settings::default()
        };
        assert_eq!(settings.theme_name(), "light");
    }

    #[test]
    fn editor_font_defaults_when_unset() {
        let settings = Settings::default();
        assert_eq!(settings.editor_font_family_or_default(), "Monospace");
        assert_eq!(settings.editor_font_size_or_default(), 11);
    }

    #[test]
    fn editor_font_returns_the_set_values() {
        let settings = Settings {
            editor_font_family: "Fira Code".to_string(),
            editor_font_size: 14,
            ..Settings::default()
        };
        assert_eq!(settings.editor_font_family_or_default(), "Fira Code");
        assert_eq!(settings.editor_font_size_or_default(), 14);
    }

    #[test]
    fn a_settings_file_with_an_editing_section_parses_it_and_its_languages() {
        let dir = tempfile::tempdir().unwrap();
        let body = "\
theme = \"dark\"

[editing]
tab_width = 2
trim_trailing_whitespace = false

[editing.languages.go]
use_spaces = false
";
        fs::write(dir.path().join(SETTINGS_FILE), body).unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.editing.tab_width_or_default(), 2);
        assert!(!loaded.editing.trim_trailing_whitespace_or_default());
        assert!(!loaded
            .editing
            .for_language("go")
            .unwrap()
            .use_spaces_or_default());
    }

    #[test]
    fn a_settings_file_without_an_editing_section_parses() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"dark\"\n").unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.editing, EditingSettings::default());
        assert_eq!(loaded.editing.tab_width_or_default(), 4);
    }

    #[test]
    fn ui_font_scales_default_to_unscaled_when_unset() {
        let settings = Settings::default();
        assert_eq!(settings.ui_font_scale_or_default(), 100);
        assert_eq!(settings.project_tree_font_scale_or_default(), 100);
        assert_eq!(settings.menu_font_scale_or_default(), 100);
    }

    #[test]
    fn ui_font_scales_return_the_set_values() {
        let settings = Settings {
            ui_font_scale: 130,
            project_tree_font_scale: 150,
            project_tree_sort_descending: true,
            menu_font_scale: 90,
            ..Settings::default()
        };
        assert_eq!(settings.ui_font_scale_or_default(), 130);
        assert_eq!(settings.project_tree_font_scale_or_default(), 150);
        assert_eq!(settings.menu_font_scale_or_default(), 90);
    }

    #[test]
    fn ui_font_scales_clamp_a_hand_edited_file() {
        let settings = Settings {
            ui_font_scale: 5000,
            project_tree_font_scale: 1,
            menu_font_scale: 300,
            ..Settings::default()
        };
        assert_eq!(settings.ui_font_scale_or_default(), 300);
        assert_eq!(settings.project_tree_font_scale_or_default(), 50);
        assert_eq!(settings.menu_font_scale_or_default(), 300);
    }

    #[test]
    fn a_settings_file_without_ui_font_scales_parses() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"light\"\n").unwrap();

        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded.ui_font_scale, 0);
        assert_eq!(loaded.ui_font_scale_or_default(), 100);
        assert_eq!(loaded.project_tree_font_scale_or_default(), 100);
        assert_eq!(loaded.menu_font_scale_or_default(), 100);
    }

    #[test]
    fn missing_file_loads_as_default() {
        let dir = tempfile::tempdir().unwrap();
        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, Settings::default());
    }

    #[test]
    fn partial_toml_fills_in_defaults() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"dark\"\n").unwrap();

        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded.theme, "dark");
        assert_eq!(loaded.editor_font_size, 0);
        assert_eq!(loaded.editor_font_family, "");
        assert!(loaded.editor_colors.is_empty());
        assert!(loaded.recent_projects.is_empty());
        assert_eq!(loaded.window_geometry, WindowGeometry::default());
        assert_eq!(loaded.window_state, "");
        assert_eq!(loaded.editor_layout, "");
    }

    #[test]
    fn malformed_toml_errors_without_panicking() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path()).unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            "theme = \"unterminated string\n[[[not valid",
        )
        .unwrap();

        let result = load(dir.path());
        assert!(matches!(result, Err(ConfigError::Parse(_))));
    }

    #[test]
    fn push_recent_file_dedupes_and_keeps_newest_first() {
        let mut settings = Settings::default();
        settings.push_recent_file(PathBuf::from("/a.rs"));
        settings.push_recent_file(PathBuf::from("/b.rs"));
        settings.push_recent_file(PathBuf::from("/a.rs"));

        assert_eq!(
            settings.recent_files,
            vec![PathBuf::from("/a.rs"), PathBuf::from("/b.rs")]
        );
    }

    #[test]
    fn push_recent_file_caps_the_list() {
        let mut settings = Settings::default();
        for i in 0..MAX_RECENT_FILES + 10 {
            settings.push_recent_file(PathBuf::from(format!("/f{i}.rs")));
        }
        assert_eq!(settings.recent_files.len(), MAX_RECENT_FILES);
        assert_eq!(
            settings.recent_files[0],
            PathBuf::from(format!("/f{}.rs", MAX_RECENT_FILES + 9))
        );
    }

    #[test]
    fn syntax_colors_accept_both_the_string_and_table_spellings() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            concat!(
                "[syntax_colors]\n",
                "keyword = \"#cc7832\"\n",
                "comment = { fg = \"#808080\", italic = true }\n",
                "\n",
                "[syntax_colors_by_language.python]\n",
                "decorator = \"#ffc66d\"\n",
            ),
        )
        .unwrap();

        let loaded = load(dir.path()).unwrap();

        let keyword = &loaded.syntax_colors["keyword"];
        assert_eq!(keyword.fg(), Some("#cc7832"));
        assert!(!keyword.italic());
        let comment = &loaded.syntax_colors["comment"];
        assert_eq!(comment.fg(), Some("#808080"));
        assert!(comment.italic());
        assert!(!comment.bold());
        assert_eq!(
            loaded.syntax_colors_by_language["python"]["decorator"].fg(),
            Some("#ffc66d")
        );

        // And both spellings survive being written back out.
        save(dir.path(), &loaded).unwrap();
        assert_eq!(load(dir.path()).unwrap(), loaded);
    }

    #[test]
    fn syntax_color_table_form_keeps_only_the_flags_it_sets() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            "[syntax_colors]\nerror = { bold = true, underline = true }\n",
        )
        .unwrap();

        let loaded = load(dir.path()).unwrap();
        let style = &loaded.syntax_colors["error"];

        assert_eq!(style.fg(), None);
        assert!(style.bold());
        assert!(style.underline());
        assert!(!style.italic());

        save(dir.path(), &loaded).unwrap();
        assert_eq!(load(dir.path()).unwrap(), loaded);
    }

    #[test]
    fn unknown_scope_names_and_language_ids_survive_a_load_save_cycle() {
        // A newer build may know scopes and languages this one does not; they
        // are stored as opaque strings, never rejected or dropped. Dotted
        // scope names must be quoted in TOML — a bare `a.b.c` key is a nested
        // table per the TOML spec, not a scope named "a.b.c" — and the
        // serializer quotes them again on the way out.
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            concat!(
                "[syntax_colors]\n",
                "\"some.future.scope\" = \"#123456\"\n",
                "\n",
                "[syntax_colors_by_language.brainfuck]\n",
                "\"tape.pointer\" = { fg = \"#abcdef\", bold = true }\n",
            ),
        )
        .unwrap();

        let loaded = load(dir.path()).unwrap();
        save(dir.path(), &loaded).unwrap();
        let reloaded = load(dir.path()).unwrap();

        assert_eq!(reloaded, loaded);
        assert_eq!(
            reloaded.syntax_colors["some.future.scope"].fg(),
            Some("#123456")
        );
        assert!(reloaded.syntax_colors_by_language["brainfuck"]["tape.pointer"].bold());
    }

    #[test]
    fn settings_file_without_syntax_colors_loads_empty_maps() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"light\"\n").unwrap();

        let loaded = load(dir.path()).unwrap();

        assert!(loaded.syntax_colors.is_empty());
        assert!(loaded.syntax_colors_by_language.is_empty());
    }

    #[test]
    fn per_language_overrides_can_be_set_without_a_base_table() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            "[syntax_colors_by_language.rust]\nmacro = \"#bbb529\"\n",
        )
        .unwrap();

        let loaded = load(dir.path()).unwrap();

        assert!(loaded.syntax_colors.is_empty());
        assert_eq!(
            loaded.syntax_colors_by_language["rust"]["macro"].fg(),
            Some("#bbb529")
        );
    }

    #[test]
    fn language_server_overrides_round_trip_as_array_of_tables() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            language_servers: vec![
                LanguageServerSetting {
                    language_id: "rust".into(),
                    command: Some("/opt/ra".into()),
                    args: Some(vec!["--log".into()]),
                    ..LanguageServerSetting::default()
                },
                LanguageServerSetting {
                    language_id: "go".into(),
                    enabled: Some(false),
                    ..LanguageServerSetting::default()
                },
            ],
            ..Settings::default()
        };

        save(dir.path(), &settings).unwrap();
        let toml = fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(toml.contains("[[language_server]]"), "{toml}");

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.language_servers, settings.language_servers);
        // Unset fields stay unset rather than being written as empty strings,
        // so "only disable it" cannot silently wipe the shipped command.
        assert!(loaded.language_servers[1].command.is_none());
    }

    #[test]
    fn a_settings_file_written_before_plugins_existed_still_loads() {
        // The backward-compatibility rule for every field this crate adds:
        // a file that predates it parses, and the field reads as "never
        // chosen" rather than failing the load.
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"light\"\n").unwrap();

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.theme_name(), "light");
        assert_eq!(loaded.icon_theme, "");
        assert!(loaded.disabled_plugins.is_empty());
    }

    #[test]
    fn the_icon_theme_and_disabled_plugins_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings {
            icon_theme: "material".to_string(),
            ..Default::default()
        };
        settings.set_plugin_disabled("zebra-icons", true);
        settings.set_plugin_disabled("material-icons", true);
        settings.set_plugin_disabled("zebra-icons", true);

        save(dir.path(), &settings).unwrap();
        let mut loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.icon_theme, "material");
        assert_eq!(
            loaded.disabled_plugins,
            vec!["material-icons", "zebra-icons"]
        );
        assert!(loaded.is_plugin_disabled("material-icons"));

        loaded.set_plugin_disabled("material-icons", false);
        assert_eq!(loaded.disabled_plugins, vec!["zebra-icons"]);
        assert!(!loaded.is_plugin_disabled("material-icons"));
    }

    #[test]
    fn disabled_languages_round_trip_and_stay_sorted_and_deduped() {
        let dir = tempfile::tempdir().unwrap();
        let mut settings = Settings::default();
        settings.set_language_disabled("zig", true);
        settings.set_language_disabled("rust", true);
        settings.set_language_disabled("zig", true);

        save(dir.path(), &settings).unwrap();
        let mut loaded = load(dir.path()).unwrap();
        assert_eq!(loaded.disabled_languages, vec!["rust", "zig"]);
        assert!(loaded.is_language_disabled("rust"));

        loaded.set_language_disabled("rust", false);
        assert_eq!(loaded.disabled_languages, vec!["zig"]);
        assert!(!loaded.is_language_disabled("rust"));
    }

    #[test]
    fn a_settings_file_without_disabled_languages_parses() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"light\"\n").unwrap();
        assert!(load(dir.path()).unwrap().disabled_languages.is_empty());
    }

    #[test]
    fn ai_providers_and_tool_policies_round_trip_as_arrays_of_tables() {
        let dir = tempfile::tempdir().unwrap();
        let settings = Settings {
            ai_providers: vec![
                AiProviderSetting {
                    id: "anthropic".into(),
                    kind: "anthropic".into(),
                    model: "claude-sonnet-4-5".into(),
                    api_key_env: "ANTHROPIC_API_KEY".into(),
                    enabled: true,
                    ..AiProviderSetting::default()
                },
                AiProviderSetting {
                    id: "ollama".into(),
                    kind: "openai_compatible".into(),
                    base_url: "http://localhost:11434/v1".into(),
                    model: "qwen2.5-coder".into(),
                    enabled: false,
                    ..AiProviderSetting::default()
                },
            ],
            ai_active_provider: "anthropic".into(),
            ai_tool_policies: vec![AiToolPolicySetting {
                tool: "save_buffer".into(),
                policy: "never".into(),
            }],
            ..Settings::default()
        };

        save(dir.path(), &settings).unwrap();
        let toml = fs::read_to_string(dir.path().join(SETTINGS_FILE)).unwrap();
        assert!(toml.contains("[[ai_provider]]"), "{toml}");
        assert!(toml.contains("[[ai_tool_policy]]"), "{toml}");
        // The one field that must never reach the disk.
        assert!(!toml.contains("api_key ="), "{toml}");

        let loaded = load(dir.path()).unwrap();
        assert_eq!(loaded, settings);
    }

    #[test]
    fn a_settings_file_written_before_ai_chat_still_loads() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join(SETTINGS_FILE),
            concat!(
                "theme = \"light\"\n",
                "editor_font_size = 13\n",
                "\n",
                "[[language_server]]\n",
                "language_id = \"rust\"\n",
            ),
        )
        .unwrap();

        let loaded = load(dir.path()).unwrap();

        assert_eq!(loaded.theme_name(), "light");
        assert!(loaded.ai_providers.is_empty());
        assert!(loaded.ai_tool_policies.is_empty());
        assert_eq!(loaded.ai_active_provider, "");
        assert_eq!(loaded.ai_mode, "");
        // Unset means on, so an upgrade does not silently stop keeping
        // transcripts.
        assert_eq!(loaded.ai_persist_conversations, None);
        assert!(loaded.ai_persist_conversations_or_default());
    }

    #[test]
    fn a_settings_file_without_language_servers_parses() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(SETTINGS_FILE), "theme = \"light\"\n").unwrap();
        assert!(load(dir.path()).unwrap().language_servers.is_empty());
    }
}
