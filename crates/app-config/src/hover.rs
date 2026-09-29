//! The `[hover]` section: what the editor's hover card shows when the pointer
//! dwells on a symbol or squiggle, and how long it waits first.
//!
//! Also home to the one rule those switches feed, [`HoverSettings::scope`]:
//! the bridge asks it what a request should fetch, so neither the bridge nor
//! the view re-derives it.

use serde::{Deserialize, Serialize};

/// Bounds on the dwell delay, in milliseconds. Enforced on read, like the
/// other bounded settings: a hand-edited `delay_ms = 0` must not make the card
/// flicker under every mouse move, and minutes is not a preference either.
pub const MIN_HOVER_DELAY_MS: u32 = 100;
pub const MAX_HOVER_DELAY_MS: u32 = 3000;
pub const DEFAULT_HOVER_DELAY_MS: u32 = 500;

/// What one hover request should fetch and show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HoverScope {
    /// Signature and documentation (an LSP hover, the index fallback).
    pub docs: bool,
    /// Diagnostics and their quick fixes.
    pub problems: bool,
}

#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq)]
#[serde(default)]
pub struct HoverSettings {
    /// Show quick documentation when the pointer dwells on a symbol.
    pub docs_on_hover: bool,
    /// Dwell time before the card appears; read through
    /// [`HoverSettings::delay`].
    pub delay_ms: u32,
    /// Show problems and their quick fixes when the pointer dwells on a squiggle.
    pub problems_on_hover: bool,
}

impl HoverSettings {
    /// The built-in values, usable in a `static`.
    pub const DEFAULT: HoverSettings = HoverSettings {
        docs_on_hover: true,
        delay_ms: DEFAULT_HOVER_DELAY_MS,
        problems_on_hover: true,
    };

    /// `delay_ms` clamped to `MIN_HOVER_DELAY_MS..=MAX_HOVER_DELAY_MS`.
    pub fn delay(&self) -> u32 {
        self.delay_ms.clamp(MIN_HOVER_DELAY_MS, MAX_HOVER_DELAY_MS)
    }

    /// The dwell delay, or `None` when both switches are off and hovering
    /// should not start a request at all.
    pub fn dwell_delay_ms(&self) -> Option<u32> {
        self.scope(false).map(|_| self.delay())
    }

    /// What a request shows. A dwell honours the switches (`None` when both
    /// are off); an explicit quick-documentation request (Ctrl+Alt+Q) always
    /// shows everything.
    pub fn scope(&self, quick: bool) -> Option<HoverScope> {
        if quick {
            return Some(HoverScope {
                docs: true,
                problems: true,
            });
        }
        (self.docs_on_hover || self.problems_on_hover).then_some(HoverScope {
            docs: self.docs_on_hover,
            problems: self.problems_on_hover,
        })
    }
}

impl Default for HoverSettings {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_show_everything_after_half_a_second() {
        let s = HoverSettings::default();
        assert!(s.docs_on_hover && s.problems_on_hover);
        assert_eq!(s.delay(), 500);
    }

    #[test]
    fn delay_is_clamped_on_read() {
        let low = HoverSettings {
            delay_ms: 0,
            ..Default::default()
        };
        let high = HoverSettings {
            delay_ms: 99_999,
            ..Default::default()
        };
        assert_eq!(low.delay(), MIN_HOVER_DELAY_MS);
        assert_eq!(high.delay(), MAX_HOVER_DELAY_MS);
    }

    #[test]
    fn toml_round_trips() {
        let s = HoverSettings {
            docs_on_hover: false,
            delay_ms: 1500,
            problems_on_hover: true,
        };
        let text = toml::to_string(&s).unwrap();
        assert_eq!(toml::from_str::<HoverSettings>(&text).unwrap(), s);
    }

    #[test]
    fn a_settings_file_without_the_section_loads_the_defaults() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(crate::SETTINGS_FILE), "theme = \"light\"\n").unwrap();
        assert_eq!(
            crate::load(dir.path()).unwrap().hover,
            HoverSettings::default()
        );
    }

    #[test]
    fn a_partial_section_keeps_the_other_defaults() {
        let s: HoverSettings = toml::from_str("docs_on_hover = false").unwrap();
        assert!(!s.docs_on_hover && s.problems_on_hover);
        assert_eq!(s.delay_ms, DEFAULT_HOVER_DELAY_MS);
    }

    #[test]
    fn docs_off_keeps_problems_only() {
        let s = HoverSettings {
            docs_on_hover: false,
            ..Default::default()
        };
        assert_eq!(
            s.scope(false),
            Some(HoverScope {
                docs: false,
                problems: true
            })
        );
    }

    #[test]
    fn problems_off_keeps_docs_only() {
        let s = HoverSettings {
            problems_on_hover: false,
            ..Default::default()
        };
        assert_eq!(
            s.scope(false),
            Some(HoverScope {
                docs: true,
                problems: false
            })
        );
    }

    #[test]
    fn both_off_disables_the_dwell_but_not_quick_documentation() {
        let s = HoverSettings {
            docs_on_hover: false,
            problems_on_hover: false,
            delay_ms: 500,
        };
        assert_eq!(s.scope(false), None);
        assert_eq!(s.dwell_delay_ms(), None);
        assert_eq!(
            s.scope(true),
            Some(HoverScope {
                docs: true,
                problems: true
            })
        );
    }
}
