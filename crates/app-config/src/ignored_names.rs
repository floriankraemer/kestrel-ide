//! [`Settings::ignored_names`](crate::Settings::ignored_names)'s default
//! list and [`Settings`](crate::Settings)'s [`Default`] impl (ADR-0064).

use crate::Settings;

/// The names [`Settings::ignored_names`](crate::Settings::ignored_names)
/// starts with — never source code in any project, so every fresh install
/// and every settings file written before ADR-0064 gets them without the
/// user typing a single one.
pub const DEFAULT_IGNORED_NAMES: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    "CVS",
    "_svn",
    ".DS_Store",
    "__pycache__",
    "*.pyc",
    "*.pyo",
    "*.rbc",
    "*.yarb",
    "*~",
    "vssver.scc",
    "vssver2.scc",
    ".ide-index",
    "node_modules",
    "target",
];

pub(crate) fn default_ignored_names() -> Vec<String> {
    DEFAULT_IGNORED_NAMES
        .iter()
        .map(|s| s.to_string())
        .collect()
}

// `Settings` cannot derive `Default`: `ignored_names` must default to
// `DEFAULT_IGNORED_NAMES`, not an empty `Vec`, and a derived `Default` has
// no way to call a field's `#[serde(default = "...")]` function. Round-
// tripping an empty TOML document instead reuses every field's own serde
// default — including this one — so `Settings::default()` and "a
// `settings.toml` with no keys at all" can never drift apart.
impl Default for Settings {
    fn default() -> Self {
        toml::from_str("").expect("every field of Settings has a serde default")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_settings_defaults_ignored_names_to_the_adr_0064_list() {
        assert_eq!(
            Settings::default().ignored_names,
            default_ignored_names(),
            "Settings::default() must agree with what an empty settings.toml parses to"
        );
    }

    #[test]
    fn a_settings_toml_missing_the_key_entirely_also_gets_the_default_list() {
        let loaded: Settings = toml::from_str("theme = \"dark\"\n").unwrap();
        assert_eq!(loaded.ignored_names, default_ignored_names());
    }

    #[test]
    fn the_old_index_excludes_key_is_read_as_an_alias_with_exactly_its_own_list() {
        // ADR-0064: a user who had set the old key keeps exactly their own
        // list, without the new defaults folded in — the alias reads the
        // key, it does not merge it with `DEFAULT_IGNORED_NAMES`.
        let loaded: Settings = toml::from_str("index_excludes = [\"scratch/\"]\n").unwrap();
        assert_eq!(loaded.ignored_names, vec!["scratch/".to_string()]);
    }
}
