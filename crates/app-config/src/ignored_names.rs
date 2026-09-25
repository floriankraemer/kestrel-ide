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

/// Add `pattern` (a gitignore-style name/glob, not a path — no
/// absolute/escape rejection applies here) to
/// [`Settings::ignored_names`](Settings::ignored_names). Trims, skips a
/// blank pattern (`false`), dedupes against the existing list (`false` if
/// already present), else pushes and returns `true`.
pub fn add_ignored_name(settings: &mut Settings, pattern: &str) -> bool {
    let trimmed = pattern.trim();
    if trimmed.is_empty() || settings.ignored_names.iter().any(|n| n == trimmed) {
        return false;
    }
    settings.ignored_names.push(trimmed.to_string());
    true
}

/// Remove `pattern` from [`Settings::ignored_names`](Settings::ignored_names)
/// if present. Returns whether the list changed.
pub fn remove_ignored_name(settings: &mut Settings, pattern: &str) -> bool {
    let trimmed = pattern.trim();
    match settings.ignored_names.iter().position(|n| n == trimmed) {
        Some(pos) => {
            settings.ignored_names.remove(pos);
            true
        }
        None => false,
    }
}

/// Reset [`Settings::ignored_names`](Settings::ignored_names) back to
/// [`DEFAULT_IGNORED_NAMES`].
pub fn reset_ignored_names_to_defaults(settings: &mut Settings) {
    settings.ignored_names = default_ignored_names();
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
    fn add_ignored_name_skips_blank_patterns() {
        let mut settings = Settings::default();
        let before = settings.ignored_names.clone();
        assert!(!add_ignored_name(&mut settings, "   "));
        assert_eq!(settings.ignored_names, before);
    }

    #[test]
    fn add_ignored_name_dedupes() {
        let mut settings = Settings::default();
        assert!(!add_ignored_name(&mut settings, ".git"));
        assert!(add_ignored_name(&mut settings, "*.bak"));
        assert!(!add_ignored_name(&mut settings, "*.bak"));
        assert_eq!(
            settings.ignored_names.iter().filter(|n| *n == "*.bak").count(),
            1
        );
    }

    #[test]
    fn remove_ignored_name_of_an_absent_pattern_is_a_no_op() {
        let mut settings = Settings::default();
        let before = settings.ignored_names.clone();
        assert!(!remove_ignored_name(&mut settings, "*.nope"));
        assert_eq!(settings.ignored_names, before);
    }

    #[test]
    fn remove_ignored_name_removes_a_present_pattern() {
        let mut settings = Settings::default();
        assert!(remove_ignored_name(&mut settings, "node_modules"));
        assert!(!settings.ignored_names.contains(&"node_modules".to_string()));
    }

    #[test]
    fn reset_ignored_names_restores_the_defaults_verbatim() {
        let mut settings = Settings::default();
        add_ignored_name(&mut settings, "*.bak");
        remove_ignored_name(&mut settings, "target");
        reset_ignored_names_to_defaults(&mut settings);
        assert_eq!(settings.ignored_names, default_ignored_names());
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
