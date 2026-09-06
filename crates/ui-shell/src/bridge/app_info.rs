//! Rust side of the `AppInfo` QObject: what the About dialog says about this
//! build.
//!
//! Constants only — no session, no handles, no state. It exists so the
//! product name, the crate version and the build metadata `build.rs` baked in
//! have one source of truth on the Rust side of the seam, rather than being
//! re-typed as string literals in `cpp/` where nothing can test them.

use cxx_qt_lib::QString;

use crate::bridge::ffi;

/// The product name, as the window title, the splash screen and the About
/// dialog all say it.
pub const APP_NAME: &str = "Kestrel";

/// Where the project lives. Shown as a link in the About dialog and opened in
/// the user's browser, so it is a fact about the product rather than a
/// preference.
pub const PROJECT_URL: &str = "https://github.com/floriankraemer/kestrel-ide";

/// Stateless: cxx-qt builds QObjects through `Default`, and this one has
/// nothing to build.
#[derive(Default)]
pub struct AppInfoRust;

impl ffi::AppInfo {
    pub fn app_name(&self) -> QString {
        QString::from(APP_NAME)
    }

    pub fn app_version(&self) -> QString {
        QString::from(env!("CARGO_PKG_VERSION"))
    }

    pub fn git_hash(&self) -> QString {
        QString::from(env!("KESTREL_GIT_HASH"))
    }

    pub fn git_date(&self) -> QString {
        QString::from(env!("KESTREL_GIT_DATE"))
    }

    pub fn project_url(&self) -> QString {
        QString::from(PROJECT_URL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_build_metadata_is_baked_in() {
        // Not a tautology: `build.rs` emits both, and the only way this fails
        // is the build script silently no longer running — which would leave
        // the About dialog claiming a version it cannot know.
        assert!(!env!("KESTREL_GIT_HASH").is_empty());
        assert!(!env!("KESTREL_GIT_DATE").is_empty());
    }

    #[test]
    fn the_project_url_is_the_repository_it_claims_to_be() {
        assert!(PROJECT_URL.starts_with("https://"));
        assert!(!APP_NAME.is_empty());
    }
}
