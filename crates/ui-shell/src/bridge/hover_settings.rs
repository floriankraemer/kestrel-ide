//! H6: the hover card's settings (`[hover]`): the `AppSettings` slots behind
//! the Editor page, and the slots that set `LanguageService`'s live copy.
//!
//! What a dwell shows is `app_config::HoverSettings::scope`'s rule; this file
//! only carries the values across the seam.

use core::pin::Pin;

use app_config::HoverSettings;

use crate::bridge::ffi::{self, FfiHoverOptions};

impl ffi::AppSettings {
    pub fn hover_options(&self) -> FfiHoverOptions {
        let hover = app_config::load(&app_core::resolve_config_dir())
            .unwrap_or_default()
            .hover;
        FfiHoverOptions {
            docs_on_hover: hover.docs_on_hover,
            delay_ms: hover.delay(),
            problems_on_hover: hover.problems_on_hover,
        }
    }

    pub fn save_hover_options(&self, options: &FfiHoverOptions) {
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.hover = HoverSettings {
            docs_on_hover: options.docs_on_hover,
            delay_ms: options.delay_ms,
            problems_on_hover: options.problems_on_hover,
        };
        let _ = app_config::save(&config_dir, &settings);
    }
}

impl ffi::LanguageService {
    /// Apply the hover settings live.
    pub fn set_hover_options(self: Pin<&mut Self>, options: &FfiHoverOptions) {
        self.hover_settings.set(HoverSettings {
            docs_on_hover: options.docs_on_hover,
            delay_ms: options.delay_ms,
            problems_on_hover: options.problems_on_hover,
        });
    }

    /// The editor's dwell delay; 0 turns the dwell off.
    pub fn hover_dwell_delay_ms(&self) -> u32 {
        self.hover_settings.get().dwell_delay_ms().unwrap_or(0)
    }
}
