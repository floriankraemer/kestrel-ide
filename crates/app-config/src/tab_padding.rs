//! The `[tab_padding]` section: how much air surrounds an editor tab's
//! label, on each of its four sides.
//!
//! Persistence only, like the rest of this crate (ADR-0017) — this module
//! stores the numbers and enforces the one bound the issue that added it
//! asked for; it has no opinion about how a project's override interacts
//! with the global layer (`settings_model::scope`'s job) or how the numbers
//! reach a stylesheet (`ui-shell`'s).
//!
//! Every field is `Option<u32>`: `0` is a value a user can mean (no padding
//! on that side, which is also the built-in top/bottom default), so it
//! cannot double as the "never chosen" sentinel the way `EditingSettings::
//! tab_width` uses it — the same reasoning `EditingSettings::wrap_column`
//! documents for itself.
//!
//! # Error, not clamp
//!
//! [`EditingSettings`](crate::EditingSettings)'s bounds are enforced by
//! clamping on read: a `tab_width` of `200` in a hand-edited file quietly
//! becomes [`crate::editing::MAX_TAB_WIDTH`]. Padding does the opposite on
//! purpose, because the issue that added it asked for an error message
//! rather than a silently-different number: [`TabPaddingSettings::validate`]
//! is called from [`crate::load`] and [`crate::project_settings::load`], and
//! a value over [`MAX_TAB_PADDING`] fails the load instead of being
//! reinterpreted.

use serde::{Deserialize, Serialize};

use crate::ConfigError;

/// Upper bound on one side's padding, in pixels. Zero is always allowed —
/// `u32` already rules out negative — so this is the only number
/// [`TabPaddingSettings::validate`] checks against.
pub const MAX_TAB_PADDING: u32 = 100;

/// Built-in padding, matching the pixel values `theme.cpp` hardcoded before
/// this setting existed (`tokens::kSp3` and `tokens::kSp1`), so a user who
/// never opens the settings page sees no change at all.
pub const DEFAULT_TAB_PADDING_TOP: u32 = 0;
pub const DEFAULT_TAB_PADDING_BOTTOM: u32 = 0;
pub const DEFAULT_TAB_PADDING_LEFT: u32 = 12;
pub const DEFAULT_TAB_PADDING_RIGHT: u32 = 4;

/// Padding around an editor tab's label, as the global default or as a
/// project's override of it — the same "one struct, two roles" shape
/// [`EditingSettings`](crate::EditingSettings) uses.
#[derive(Serialize, Deserialize, Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TabPaddingSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub top: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bottom: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub left: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub right: Option<u32>,
}

impl TabPaddingSettings {
    pub fn top_or_default(&self) -> u32 {
        self.top.unwrap_or(DEFAULT_TAB_PADDING_TOP)
    }

    pub fn bottom_or_default(&self) -> u32 {
        self.bottom.unwrap_or(DEFAULT_TAB_PADDING_BOTTOM)
    }

    pub fn left_or_default(&self) -> u32 {
        self.left.unwrap_or(DEFAULT_TAB_PADDING_LEFT)
    }

    pub fn right_or_default(&self) -> u32 {
        self.right.unwrap_or(DEFAULT_TAB_PADDING_RIGHT)
    }

    /// `Err` naming the first side that is over [`MAX_TAB_PADDING`], if any.
    /// Negative is not a state this type can even represent (`u32`), so the
    /// upper bound is the only thing there is to check.
    pub fn validate(&self) -> Result<(), ConfigError> {
        for (side, value) in [
            ("top", self.top),
            ("bottom", self.bottom),
            ("left", self.left),
            ("right", self.right),
        ] {
            if let Some(value) = value {
                if value > MAX_TAB_PADDING {
                    return Err(ConfigError::OutOfRange(format!(
                        "tab padding {side} is {value}px, but the maximum is {MAX_TAB_PADDING}px"
                    )));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unset_section_answers_with_the_current_hardcoded_values() {
        let padding = TabPaddingSettings::default();
        assert_eq!(padding.top_or_default(), 0);
        assert_eq!(padding.bottom_or_default(), 0);
        assert_eq!(padding.left_or_default(), 12);
        assert_eq!(padding.right_or_default(), 4);
        assert!(padding.validate().is_ok());
    }

    #[test]
    fn zero_is_a_real_value_not_unset() {
        let padding = TabPaddingSettings {
            top: Some(0),
            ..TabPaddingSettings::default()
        };
        assert_eq!(padding.top_or_default(), 0);
        assert!(padding.validate().is_ok());
    }

    #[test]
    fn the_maximum_is_accepted_but_one_over_it_is_an_error() {
        let at_max = TabPaddingSettings {
            right: Some(MAX_TAB_PADDING),
            ..TabPaddingSettings::default()
        };
        assert!(at_max.validate().is_ok());

        let over_max = TabPaddingSettings {
            right: Some(MAX_TAB_PADDING + 1),
            ..TabPaddingSettings::default()
        };
        let err = over_max.validate().unwrap_err();
        assert!(matches!(err, ConfigError::OutOfRange(_)));
        assert!(err.to_string().contains("right"), "{err}");
    }

    #[test]
    fn every_side_is_checked_independently() {
        for side in ["top", "bottom", "left", "right"] {
            let mut padding = TabPaddingSettings::default();
            let over = Some(MAX_TAB_PADDING + 50);
            match side {
                "top" => padding.top = over,
                "bottom" => padding.bottom = over,
                "left" => padding.left = over,
                "right" => padding.right = over,
                _ => unreachable!(),
            }
            assert!(padding.validate().is_err(), "{side} was not checked");
        }
    }
}
