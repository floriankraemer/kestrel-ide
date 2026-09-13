//! The `[editing]` section: how a buffer indents, wraps, and is written back
//! to disk.
//!
//! Persistence only, like the rest of this crate. What a value *means* — what
//! a language may override, what a nonsensical tab width resolves to on the
//! page, which of these become a save transaction — is
//! `settings_model::editing`'s job (ADR-0017).
//!
//! Two things are worth knowing before adding a field here:
//!
//! - Every field carries an "unset" state, because the same struct is used
//!   for the global defaults *and* for a `[editing.languages.<id>]` override,
//!   where unset means "inherit". `0` for a count, `""` for a name and `None`
//!   for a flag all mean the user never chose.
//! - [`EditingSettings::wrap_column`] is the exception that proves the rule:
//!   `0` is a value a user can mean ("never wrap"), so it is an `Option`
//!   rather than a sentinel. A field whose zero is meaningful cannot use the
//!   zero-is-unset idiom.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Tab width used when the user has never chosen one.
pub const DEFAULT_TAB_WIDTH: u32 = 4;

/// Bounds on a tab width. Enforced on read, like the interface font scales:
/// a hand-edited `settings.toml` saying `tab_width = 0` must not produce a
/// buffer that indents by nothing, and `200` is not a preference either.
pub const MIN_TAB_WIDTH: u32 = 1;
pub const MAX_TAB_WIDTH: u32 = 16;

/// Bounds on a non-zero wrap column. Below the minimum the guide sits inside
/// ordinary indentation; above the maximum it is off the side of any window.
/// `0` is outside this range on purpose — it means "do not wrap at all".
pub const MIN_WRAP_COLUMN: u32 = 20;
pub const MAX_WRAP_COLUMN: u32 = 500;

/// Encoding assumed when the user has never chosen one.
pub const DEFAULT_ENCODING: &str = "utf-8";

/// Line-ending policy assumed when the user has never chosen one: keep what
/// the file already uses. Rewriting every terminator in a file the user only
/// opened is a whole-file diff nobody asked for.
pub const DEFAULT_LINE_ENDINGS: &str = "preserve";

/// Editing behaviour, as the global defaults or as one language's overrides.
///
/// Written as `[editing]`, with per-language tables under
/// `[editing.languages.<language_id>]` so a Go file and a Python file open at
/// once can disagree about tabs without either of them being wrong.
///
/// A language table's own `languages` map is meaningless and ignored — the
/// table is one level deep by design. It exists in the type only because the
/// global section and an override are otherwise the same shape, and one
/// struct beats two that drift apart.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq, Eq, Default)]
pub struct EditingSettings {
    /// Columns one indentation level is worth. `0` means "never chosen",
    /// which resolves to [`DEFAULT_TAB_WIDTH`]; see
    /// [`EditingSettings::tab_width_or_default`] for the clamp.
    #[serde(default)]
    pub tab_width: u32,
    /// Indent with spaces rather than tab characters. `None` means "never
    /// chosen" — a bare `bool` would make the derived `Default` say "tabs"
    /// and silently change how every existing user's files are indented.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub use_spaces: Option<bool>,
    /// Strip trailing whitespace on save. `None` means "never chosen".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim_trailing_whitespace: Option<bool>,
    /// Make sure the file ends with a newline on save. `None` means "never
    /// chosen".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub insert_final_newline: Option<bool>,
    /// Column the wrap guide sits at, `Some(0)` for "never wrap". `None`
    /// means "never chosen"; see the module docs for why this one field is
    /// not a zero-sentinel.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wrap_column: Option<u32>,
    /// Whether the editor actually reflows text at `wrap_column` (soft
    /// wrap), rather than only painting the guide line there. `None` means
    /// "never chosen" — a bare `bool` would make the derived `Default` say
    /// "on" and start reflowing every existing user's files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub soft_wrap: Option<bool>,
    /// Encoding name used when a file gives no clue about its own, e.g.
    /// `"utf-8"`. Opaque to this crate. Empty means "never chosen".
    #[serde(default)]
    pub default_encoding: String,
    /// `"preserve"`, `"lf"`, `"crlf"` or `"platform"`. A plain string for the
    /// same reason `AiProviderSetting::kind` is one: this crate stores the
    /// vocabulary, `settings-model` owns it. Empty means "never chosen".
    #[serde(default)]
    pub line_endings: String,
    /// Per-language overrides, keyed by language id.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub languages: HashMap<String, EditingSettings>,
}

impl EditingSettings {
    /// The tab width, defaulted when unset and clamped into
    /// [`MIN_TAB_WIDTH`]..=[`MAX_TAB_WIDTH`] — so no caller can be handed a
    /// width that indents by nothing or runs off the screen.
    pub fn tab_width_or_default(&self) -> u32 {
        if self.tab_width == 0 {
            DEFAULT_TAB_WIDTH
        } else {
            self.tab_width.clamp(MIN_TAB_WIDTH, MAX_TAB_WIDTH)
        }
    }

    /// Whether to indent with spaces. Spaces by default: they render the
    /// same everywhere, which is the property a shared file needs most.
    pub fn use_spaces_or_default(&self) -> bool {
        self.use_spaces.unwrap_or(true)
    }

    /// Whether to strip trailing whitespace on save. On by default:
    /// whitespace nobody can see is a diff nobody wants to review.
    pub fn trim_trailing_whitespace_or_default(&self) -> bool {
        self.trim_trailing_whitespace.unwrap_or(true)
    }

    /// Whether to end the file with a newline on save. On by default —
    /// POSIX says a text file's last line ends with one, and the tools that
    /// disagree all complain about the same missing byte.
    pub fn insert_final_newline_or_default(&self) -> bool {
        self.insert_final_newline.unwrap_or(true)
    }

    /// The wrap column: `0` for "never wrap", otherwise clamped into
    /// [`MIN_WRAP_COLUMN`]..=[`MAX_WRAP_COLUMN`]. Unset means no wrapping,
    /// which is what an editor that has never been configured should do.
    pub fn wrap_column_or_default(&self) -> u32 {
        match self.wrap_column {
            None | Some(0) => 0,
            Some(column) => column.clamp(MIN_WRAP_COLUMN, MAX_WRAP_COLUMN),
        }
    }

    /// Whether to soft-wrap at `wrap_column`. Off by default: the guide
    /// line alone is the IntelliJ-style default, and turning reflow on
    /// underneath a user who never asked for it would move every line on
    /// screen.
    pub fn soft_wrap_or_default(&self) -> bool {
        self.soft_wrap.unwrap_or(false)
    }

    /// The assumed encoding name, defaulted when unset.
    pub fn default_encoding_or_default(&self) -> &str {
        if self.default_encoding.is_empty() {
            DEFAULT_ENCODING
        } else {
            &self.default_encoding
        }
    }

    /// The line-ending policy name, defaulted when unset. Still just a
    /// string here — `settings_model::editing` turns it into a decision.
    pub fn line_endings_or_default(&self) -> &str {
        if self.line_endings.is_empty() {
            DEFAULT_LINE_ENDINGS
        } else {
            &self.line_endings
        }
    }

    /// The overrides for one language, if it has any.
    pub fn for_language(&self, language_id: &str) -> Option<&EditingSettings> {
        self.languages.get(language_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_section_answers_with_the_defaults() {
        let editing = EditingSettings::default();
        assert_eq!(editing.tab_width_or_default(), 4);
        assert!(editing.use_spaces_or_default());
        assert!(editing.trim_trailing_whitespace_or_default());
        assert!(editing.insert_final_newline_or_default());
        assert_eq!(editing.wrap_column_or_default(), 0);
        assert_eq!(editing.default_encoding_or_default(), "utf-8");
        assert_eq!(editing.line_endings_or_default(), "preserve");
    }

    #[test]
    fn a_hand_edited_tab_width_is_clamped_on_read() {
        let wide = EditingSettings {
            tab_width: 200,
            ..EditingSettings::default()
        };
        assert_eq!(wide.tab_width_or_default(), MAX_TAB_WIDTH);
        // 0 is not "a tab width of zero", it is "never chosen".
        let unset = EditingSettings {
            tab_width: 0,
            ..EditingSettings::default()
        };
        assert_eq!(unset.tab_width_or_default(), DEFAULT_TAB_WIDTH);
    }

    #[test]
    fn a_zero_wrap_column_survives_because_it_means_never_wrap() {
        let with = |column| EditingSettings {
            wrap_column: Some(column),
            ..EditingSettings::default()
        };
        assert_eq!(with(0).wrap_column_or_default(), 0);
        assert_eq!(with(3).wrap_column_or_default(), MIN_WRAP_COLUMN);
        assert_eq!(with(9_000).wrap_column_or_default(), MAX_WRAP_COLUMN);
    }

    #[test]
    fn the_language_table_round_trips_through_toml() {
        let text = "\
tab_width = 2
use_spaces = true

[languages.go]
tab_width = 8
use_spaces = false

[languages.python]
tab_width = 4
";
        let editing: EditingSettings = toml::from_str(text).unwrap();
        assert_eq!(editing.tab_width_or_default(), 2);
        let go = editing.for_language("go").unwrap();
        assert_eq!(go.tab_width_or_default(), 8);
        assert!(!go.use_spaces_or_default());
        // Python sets a width and nothing else; the rest is still unset,
        // which is what lets the global layer show through.
        let python = editing.for_language("python").unwrap();
        assert_eq!(python.tab_width, 4);
        assert_eq!(python.use_spaces, None);

        let round_tripped: EditingSettings =
            toml::from_str(&toml::to_string(&editing).unwrap()).unwrap();
        assert_eq!(round_tripped, editing);
    }

    #[test]
    fn an_unset_section_writes_almost_nothing() {
        // Every "never chosen" field is skipped or empty, so a settings file
        // written by a user who never opened the page gains no opinions.
        let written = toml::to_string(&EditingSettings::default()).unwrap();
        assert!(!written.contains("use_spaces"), "{written}");
        assert!(!written.contains("wrap_column"), "{written}");
        assert!(!written.contains("languages"), "{written}");
    }
}
