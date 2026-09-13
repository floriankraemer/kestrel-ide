//! Settings > Editing (task F1-10): what the page edits, and what the editor
//! actually does with it.
//!
//! `app-config` stores the `[editing]` section and nothing more. The rules —
//! which fields a language may override, what a nonsensical value is worth,
//! and how the persisted strings become the parameters the editing crates
//! take — are here, per ADR-0017.
//!
//! # What a language may override, and why
//!
//! Overridable: **tab width**, **spaces-vs-tabs**, **trim trailing
//! whitespace**, **insert final newline** and the **wrap column**. Every one
//! of them is a property of the language's own conventions, and every one of
//! them is settled per file: Go indents with tabs while Python cannot, and a
//! Markdown file where trailing spaces mean "line break" wants the trimming
//! off that a Rust file wants on.
//!
//! Not overridable: **encoding** and **line endings**. Both are properties of
//! the *file*, not of its language, and both are decided at a moment when the
//! language is the wrong thing to ask:
//!
//! - Encoding is what the bytes are read as. It has to be chosen before the
//!   text exists, and the language is normally only known after — resolving
//!   it per language would mean re-reading the file to find out how to read
//!   it.
//! - Line endings are a property of the checkout. A project whose Go files
//!   end in CRLF and whose Python files end in LF has a bug, not a
//!   preference, and offering the knob that produces it is not a kindness.
//!   The per-project layer is where a repository states this.
//!
//! A language table that sets one of the two is not silently ignored: it is a
//! [`EditingProblem`] the page shows, because a setting that parses and then
//! does nothing is worse than one that is refused.

use app_config::editing::{
    EditingSettings, MAX_TAB_WIDTH, MAX_WRAP_COLUMN, MIN_TAB_WIDTH, MIN_WRAP_COLUMN,
};
use app_config::Settings;
use edit_ops::indent::IndentStyle;
use editor_core::save_rules::{LineEnding, SaveRules};

/// The line-ending policy names the `[editing]` section accepts.
const LINE_ENDING_NAMES: [&str; 4] = ["preserve", "lf", "crlf", "platform"];

/// Which field a [`EditingProblem`] is about, so the page can focus it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditingField {
    TabWidth,
    UseSpaces,
    TrimTrailingWhitespace,
    InsertFinalNewline,
    WrapColumn,
    DefaultEncoding,
    LineEndings,
}

/// Something the page must say out loud before the settings are saved.
///
/// `language_id` is `None` for the global section and `Some` for one of the
/// per-language tables, which is what lets the page put the user back on the
/// row that is wrong.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditingProblem {
    pub language_id: Option<String>,
    pub field: EditingField,
    pub sentence: String,
}

/// The resolved answer for one buffer: global defaults, with the language's
/// overrides layered over them, clamped.
///
/// This is deliberately the parameter object the editing crates already take
/// — [`EditingRules::indent_style`] and [`EditingRules::save_rules`] hand it
/// straight to `edit_ops::indent` and `editor_core::save_rules` rather than
/// making the adapter unpack it into loose arguments and risk pairing a tab
/// width with the wrong buffer's spaces flag.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditingRules {
    pub tab_width: usize,
    pub use_spaces: bool,
    pub trim_trailing_whitespace: bool,
    pub insert_final_newline: bool,
    /// `0` means "never wrap".
    pub wrap_column: u32,
    /// Whether the editor reflows text at `wrap_column` rather than only
    /// painting the guide line there.
    pub soft_wrap: bool,
    /// Encoding name assumed when the file gives no clue about its own.
    pub encoding: String,
    /// The terminator to normalise to on save, or `None` to keep the file's
    /// own — what `"preserve"` resolves to.
    pub line_endings: Option<LineEnding>,
}

impl EditingRules {
    pub fn indent_style(&self) -> IndentStyle {
        IndentStyle {
            tab_width: self.tab_width,
            use_spaces: self.use_spaces,
        }
    }

    pub fn save_rules(&self) -> SaveRules {
        SaveRules {
            trim_trailing_whitespace: self.trim_trailing_whitespace,
            insert_final_newline: self.insert_final_newline,
            line_endings: self.line_endings,
        }
    }
}

/// The terminator a policy name asks for. `"preserve"` and anything
/// unrecognised are `None` — a file whose policy nobody can read keeps what
/// it has, which is the answer that damages nothing.
fn line_ending_for(policy: &str) -> Option<LineEnding> {
    match policy {
        "lf" => Some(LineEnding::Lf),
        "crlf" => Some(LineEnding::Crlf),
        "platform" => Some(LineEnding::platform()),
        _ => None,
    }
}

/// The editing rules in force for a buffer of `language_id`.
///
/// Layering is per field, not per section: a language table that sets only a
/// tab width leaves everything else showing through from the global section.
/// That is what makes an override an override rather than a replacement.
pub fn resolve_for_language(settings: &Settings, language_id: &str) -> EditingRules {
    let global = &settings.editing;
    let language = global.for_language(language_id);
    // Each overridable field: the language's answer if it gave one, else the
    // global one. Both sides resolve and clamp through `app-config`'s own
    // accessors, so no caller is ever handed a tab width of 0 or 200.
    let pick_u32 = |from_language: Option<u32>, global_value: u32| match from_language {
        Some(value) => value,
        None => global_value,
    };

    EditingRules {
        tab_width: pick_u32(
            language
                .filter(|l| l.tab_width != 0)
                .map(|l| l.tab_width_or_default()),
            global.tab_width_or_default(),
        ) as usize,
        use_spaces: language
            .and_then(|l| l.use_spaces)
            .unwrap_or_else(|| global.use_spaces_or_default()),
        trim_trailing_whitespace: language
            .and_then(|l| l.trim_trailing_whitespace)
            .unwrap_or_else(|| global.trim_trailing_whitespace_or_default()),
        insert_final_newline: language
            .and_then(|l| l.insert_final_newline)
            .unwrap_or_else(|| global.insert_final_newline_or_default()),
        wrap_column: pick_u32(
            language
                .filter(|l| l.wrap_column.is_some())
                .map(|l| l.wrap_column_or_default()),
            global.wrap_column_or_default(),
        ),
        soft_wrap: language
            .and_then(|l| l.soft_wrap)
            .unwrap_or_else(|| global.soft_wrap_or_default()),
        // The two the language does not get a say in — see the module docs.
        encoding: global.default_encoding_or_default().to_string(),
        line_endings: line_ending_for(global.line_endings_or_default()),
    }
}

/// The page's draft: the global section and its language tables, committed on
/// OK. Held rather than written through, so Cancel really cancels.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditingDraft {
    editing: EditingSettings,
}

impl EditingDraft {
    pub fn from_settings(settings: &Settings) -> Self {
        Self {
            editing: settings.editing.clone(),
        }
    }

    pub fn global(&self) -> &EditingSettings {
        &self.editing
    }

    pub fn global_mut(&mut self) -> &mut EditingSettings {
        &mut self.editing
    }

    /// One language's overrides, if it has any.
    pub fn language(&self, language_id: &str) -> Option<&EditingSettings> {
        self.editing.for_language(language_id)
    }

    /// Language ids with overrides, sorted — the page's row order.
    pub fn languages(&self) -> Vec<&str> {
        let mut ids: Vec<&str> = self.editing.languages.keys().map(String::as_str).collect();
        ids.sort_unstable();
        ids
    }

    /// Replace one language's overrides.
    ///
    /// An override that says nothing is removed rather than stored: the same
    /// "only what differs is persisted" rule the keymap and
    /// `[[language_server]]` follow, and it keeps a settings file from
    /// collecting empty tables for every language a user ever clicked on.
    pub fn set_language(&mut self, language_id: &str, overrides: EditingSettings) {
        if is_unset(&overrides) {
            self.editing.languages.remove(language_id);
        } else {
            self.editing
                .languages
                .insert(language_id.to_string(), overrides);
        }
    }

    pub fn clear_language(&mut self, language_id: &str) {
        self.editing.languages.remove(language_id);
    }

    /// The rules a buffer of `language_id` would get if the draft were saved.
    /// What the page's preview row shows.
    pub fn resolved(&self, language_id: &str) -> EditingRules {
        let settings = Settings {
            editing: self.editing.clone(),
            ..Settings::default()
        };
        resolve_for_language(&settings, language_id)
    }

    /// Everything wrong with the draft, in the order the page should walk the
    /// user through it.
    ///
    /// Out-of-range numbers are reported even though the accessors clamp
    /// them: clamping is the guarantee that no caller is handed a broken
    /// value, not a licence to swallow what the user typed. Silently turning
    /// `200` into `16` teaches the user that the field does nothing.
    pub fn validate(&self) -> Vec<EditingProblem> {
        let mut problems = validate_section(&self.editing, None);
        let mut ids: Vec<&String> = self.editing.languages.keys().collect();
        ids.sort();
        for id in ids {
            let overrides = &self.editing.languages[id];
            problems.extend(validate_section(overrides, Some(id)));
            for (field, name) in [
                (
                    EditingField::DefaultEncoding,
                    (!overrides.default_encoding.is_empty()).then_some("encoding"),
                ),
                (
                    EditingField::LineEndings,
                    (!overrides.line_endings.is_empty()).then_some("line endings"),
                ),
            ] {
                let Some(name) = name else { continue };
                problems.push(EditingProblem {
                    language_id: Some(id.clone()),
                    field,
                    sentence: format!(
                        "{name} is a property of the file, not of its language, \
                         so it cannot be set for {id} alone. Set it for the \
                         project or for everything."
                    ),
                });
            }
        }
        problems
    }

    /// Write the draft into `settings`.
    pub fn apply_to(&self, settings: &mut Settings) {
        settings.editing = self.editing.clone();
    }
}

/// Whether an override table says anything at all.
fn is_unset(overrides: &EditingSettings) -> bool {
    overrides.tab_width == 0
        && overrides.use_spaces.is_none()
        && overrides.trim_trailing_whitespace.is_none()
        && overrides.insert_final_newline.is_none()
        && overrides.wrap_column.is_none()
        && overrides.soft_wrap.is_none()
        && overrides.default_encoding.is_empty()
        && overrides.line_endings.is_empty()
}

fn validate_section(section: &EditingSettings, language_id: Option<&str>) -> Vec<EditingProblem> {
    let mut problems = Vec::new();
    let about = |field, sentence: String| EditingProblem {
        language_id: language_id.map(str::to_string),
        field,
        sentence,
    };

    if section.tab_width != 0 && !(MIN_TAB_WIDTH..=MAX_TAB_WIDTH).contains(&section.tab_width) {
        problems.push(about(
            EditingField::TabWidth,
            format!(
                "A tab width of {} is not a preference — pick one between {MIN_TAB_WIDTH} and \
                 {MAX_TAB_WIDTH}.",
                section.tab_width
            ),
        ));
    }
    if let Some(column) = section.wrap_column.filter(|column| *column != 0) {
        if !(MIN_WRAP_COLUMN..=MAX_WRAP_COLUMN).contains(&column) {
            problems.push(about(
                EditingField::WrapColumn,
                format!(
                    "A wrap column of {column} is outside {MIN_WRAP_COLUMN}..{MAX_WRAP_COLUMN}. \
                     Use 0 to never wrap."
                ),
            ));
        }
    }
    if !section.line_endings.is_empty()
        && !LINE_ENDING_NAMES.contains(&section.line_endings.as_str())
    {
        problems.push(about(
            EditingField::LineEndings,
            format!(
                "\"{}\" is not a line-ending policy. Use one of: {}.",
                section.line_endings,
                LINE_ENDING_NAMES.join(", ")
            ),
        ));
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(editing: EditingSettings) -> Settings {
        Settings {
            editing,
            ..Settings::default()
        }
    }

    fn language(editing: &mut EditingSettings, id: &str, overrides: EditingSettings) {
        editing.languages.insert(id.to_string(), overrides);
    }

    #[test]
    fn an_untouched_configuration_resolves_to_the_defaults() {
        let rules = resolve_for_language(&Settings::default(), "rust");
        assert_eq!(rules.tab_width, 4);
        assert!(rules.use_spaces);
        assert!(rules.trim_trailing_whitespace);
        assert!(rules.insert_final_newline);
        assert_eq!(rules.wrap_column, 0);
        assert!(!rules.soft_wrap);
        assert_eq!(rules.encoding, "utf-8");
        // "preserve": a file the user only opened keeps its own terminators.
        assert_eq!(rules.line_endings, None);
    }

    #[test]
    fn a_language_overrides_only_the_fields_it_sets() {
        let mut editing = EditingSettings {
            tab_width: 2,
            use_spaces: Some(true),
            wrap_column: Some(100),
            ..EditingSettings::default()
        };
        language(
            &mut editing,
            "go",
            EditingSettings {
                tab_width: 8,
                use_spaces: Some(false),
                ..EditingSettings::default()
            },
        );
        let settings = settings(editing);

        let go = resolve_for_language(&settings, "go");
        assert_eq!(go.tab_width, 8);
        assert!(!go.use_spaces);
        // Not overridden, so the global answer still shows through.
        assert_eq!(go.wrap_column, 100);

        let python = resolve_for_language(&settings, "python");
        assert_eq!(python.tab_width, 2);
        assert!(python.use_spaces);
    }

    #[test]
    fn two_languages_open_at_once_disagree() {
        let mut editing = EditingSettings::default();
        language(
            &mut editing,
            "go",
            EditingSettings {
                tab_width: 8,
                use_spaces: Some(false),
                ..EditingSettings::default()
            },
        );
        language(
            &mut editing,
            "python",
            EditingSettings {
                tab_width: 4,
                use_spaces: Some(true),
                ..EditingSettings::default()
            },
        );
        let settings = settings(editing);
        assert_eq!(
            resolve_for_language(&settings, "go").indent_style(),
            IndentStyle {
                tab_width: 8,
                use_spaces: false
            }
        );
        assert_eq!(
            resolve_for_language(&settings, "python").indent_style(),
            IndentStyle {
                tab_width: 4,
                use_spaces: true
            }
        );
    }

    #[test]
    fn soft_wrap_is_off_unless_a_language_or_the_global_section_turns_it_on() {
        let mut editing = EditingSettings {
            soft_wrap: Some(true),
            ..EditingSettings::default()
        };
        language(
            &mut editing,
            "markdown",
            EditingSettings {
                soft_wrap: Some(false),
                ..EditingSettings::default()
            },
        );
        let settings = settings(editing);
        assert!(resolve_for_language(&settings, "rust").soft_wrap);
        assert!(!resolve_for_language(&settings, "markdown").soft_wrap);
    }

    #[test]
    fn a_language_may_turn_the_save_rules_off_for_itself() {
        // Markdown: two trailing spaces are a line break, so trimming them
        // silently changes the rendered document.
        let mut editing = EditingSettings::default();
        language(
            &mut editing,
            "markdown",
            EditingSettings {
                trim_trailing_whitespace: Some(false),
                ..EditingSettings::default()
            },
        );
        let settings = settings(editing);
        assert!(
            !resolve_for_language(&settings, "markdown")
                .save_rules()
                .trim_trailing_whitespace
        );
        assert!(
            resolve_for_language(&settings, "rust")
                .save_rules()
                .trim_trailing_whitespace
        );
    }

    #[test]
    fn encoding_and_line_endings_ignore_the_language_table() {
        let mut editing = EditingSettings {
            default_encoding: "utf-8".into(),
            line_endings: "lf".into(),
            ..EditingSettings::default()
        };
        language(
            &mut editing,
            "go",
            EditingSettings {
                default_encoding: "latin1".into(),
                line_endings: "crlf".into(),
                ..EditingSettings::default()
            },
        );
        let settings = settings(editing);
        let go = resolve_for_language(&settings, "go");
        assert_eq!(go.encoding, "utf-8");
        assert_eq!(go.line_endings, Some(LineEnding::Lf));
    }

    #[test]
    fn a_language_table_setting_one_of_those_two_is_told_so() {
        let mut draft = EditingDraft::default();
        draft.set_language(
            "go",
            EditingSettings {
                line_endings: "crlf".into(),
                ..EditingSettings::default()
            },
        );
        let problems = draft.validate();
        assert!(
            problems
                .iter()
                .any(|p| p.field == EditingField::LineEndings
                    && p.language_id.as_deref() == Some("go")),
            "{problems:?}"
        );
    }

    #[test]
    fn a_hand_edited_tab_width_is_clamped_and_reported() {
        let editing = EditingSettings {
            tab_width: 200,
            ..EditingSettings::default()
        };
        // Clamped, so no caller is handed it...
        assert_eq!(
            resolve_for_language(&settings(editing.clone()), "rust").tab_width,
            MAX_TAB_WIDTH as usize
        );
        // ...and reported, so the user is not left thinking the field works.
        let draft = EditingDraft { editing };
        let problems = draft.validate();
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].field, EditingField::TabWidth);
        assert_eq!(problems[0].language_id, None);
    }

    #[test]
    fn a_wrap_column_of_zero_is_never_wrap_not_an_error() {
        let draft = EditingDraft {
            editing: EditingSettings {
                wrap_column: Some(0),
                ..EditingSettings::default()
            },
        };
        assert!(draft.validate().is_empty());
        assert_eq!(draft.resolved("rust").wrap_column, 0);

        let too_narrow = EditingDraft {
            editing: EditingSettings {
                wrap_column: Some(4),
                ..EditingSettings::default()
            },
        };
        assert_eq!(too_narrow.validate()[0].field, EditingField::WrapColumn);
        assert_eq!(too_narrow.resolved("rust").wrap_column, MIN_WRAP_COLUMN);
    }

    #[test]
    fn an_unreadable_line_ending_policy_is_reported_and_preserves() {
        let draft = EditingDraft {
            editing: EditingSettings {
                line_endings: "windows".into(),
                ..EditingSettings::default()
            },
        };
        assert_eq!(draft.validate()[0].field, EditingField::LineEndings);
        assert_eq!(draft.resolved("rust").line_endings, None);
    }

    #[test]
    fn every_policy_name_resolves() {
        assert_eq!(line_ending_for("preserve"), None);
        assert_eq!(line_ending_for("lf"), Some(LineEnding::Lf));
        assert_eq!(line_ending_for("crlf"), Some(LineEnding::Crlf));
        assert_eq!(line_ending_for("platform"), Some(LineEnding::platform()));
    }

    #[test]
    fn an_override_that_says_nothing_is_not_persisted() {
        let mut draft = EditingDraft::default();
        draft.set_language("go", EditingSettings::default());
        assert!(draft.languages().is_empty());

        draft.set_language(
            "go",
            EditingSettings {
                tab_width: 8,
                ..EditingSettings::default()
            },
        );
        assert_eq!(draft.languages(), vec!["go"]);

        draft.clear_language("go");
        assert!(draft.language("go").is_none());
    }

    #[test]
    fn a_draft_round_trips_through_settings() {
        let mut settings = Settings::default();
        let mut draft = EditingDraft::from_settings(&settings);
        draft.global_mut().tab_width = 2;
        draft.set_language(
            "go",
            EditingSettings {
                use_spaces: Some(false),
                ..EditingSettings::default()
            },
        );
        draft.apply_to(&mut settings);

        assert_eq!(EditingDraft::from_settings(&settings), draft);
        assert_eq!(resolve_for_language(&settings, "go").tab_width, 2);
        assert!(!resolve_for_language(&settings, "go").use_spaces);
    }

    #[test]
    fn the_rules_hand_themselves_to_the_editing_crates_unchanged() {
        let rules = EditingRules {
            tab_width: 3,
            use_spaces: false,
            trim_trailing_whitespace: false,
            insert_final_newline: true,
            wrap_column: 80,
            soft_wrap: true,
            encoding: "utf-8".into(),
            line_endings: Some(LineEnding::Crlf),
        };
        assert_eq!(rules.indent_style().unit(), "\t");
        let save = rules.save_rules();
        assert!(!save.trim_trailing_whitespace);
        assert!(save.insert_final_newline);
        assert_eq!(save.line_endings, Some(LineEnding::Crlf));
    }
}
