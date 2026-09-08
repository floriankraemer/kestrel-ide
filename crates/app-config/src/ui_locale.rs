//! The UI language accessor and its supported-locale list. Split out of
//! `lib.rs` (see the `mod ui_locale;` doc comment there) purely to keep that
//! file under its ADR-0025 size ceiling — this is a plain extension of
//! `Settings`, not a bounded concept of its own.

use crate::Settings;

/// UI locale used when `Settings::ui_locale` hasn't been set yet, or holds a
/// value we don't ship a translation for.
const DEFAULT_UI_LOCALE: &str = "en";

/// BCP-47 tags this build ships a translation for. `en` is the `tr()` source
/// text itself, not a `.ts`/`.qm` file.
pub const SUPPORTED_UI_LOCALES: &[&str] = &["en", "de", "es", "fr"];

impl Settings {
    /// The active UI locale, defaulting to `"en"` when unset or set to a tag
    /// this build has no translation for.
    pub fn ui_locale_or_default(&self) -> &str {
        if SUPPORTED_UI_LOCALES.contains(&self.ui_locale.as_str()) {
            &self.ui_locale
        } else {
            DEFAULT_UI_LOCALE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ui_locale_defaults_when_unset() {
        let settings = Settings::default();
        assert_eq!(settings.ui_locale_or_default(), "en");
    }

    #[test]
    fn ui_locale_returns_the_set_locale() {
        let settings = Settings {
            ui_locale: "de".to_string(),
            ..Settings::default()
        };
        assert_eq!(settings.ui_locale_or_default(), "de");
    }

    #[test]
    fn ui_locale_falls_back_on_unsupported_value() {
        let settings = Settings {
            ui_locale: "xx".to_string(),
            ..Settings::default()
        };
        assert_eq!(settings.ui_locale_or_default(), "en");
    }
}
