//! The contract between the IDE and a plugin.
//!
//! This crate is deliberately a leaf: `serde` and `toml` and nothing else.
//! It names no consumer — not `icon-theme`, not `plugin-host` — because
//! everything on both sides of the seam has to agree on it, and a contract
//! that depends on one of its parties is not a contract.
//!
//! Three things live here:
//!
//! * [`PluginManifest`] — `plugin.toml`, and every rule about it that can
//!   be decided without a filesystem. A manifest that exists has been
//!   validated.
//! * [`LoadErrorKind`] / [`PluginLoadError`] — why one plugin was
//!   rejected, as data rather than a formatted string, so the Settings page
//!   can group and act on it.
//! * `wit/plugin.wit` — the WebAssembly component world. It is not
//!   compiled here; `plugin-host` generates bindings from it, which is what
//!   makes a broken world a build failure rather than a runtime surprise.
//!
//! ## Layout on disk
//!
//! ```text
//! <config_dir>/plugins/<plugin-id>/plugin.toml
//! <config_dir>/plugins/<plugin-id>/…            assets a contribution names
//! <config_dir>/plugins/<plugin-id>/plugin.wasm  optional component
//! <config_dir>/plugins/.quarantine/<plugin-id>
//! ```
//!
//! Same shape as the runtime language overlay in
//! `syntax_core::runtime`, and for the same reasons: a directory per item,
//! a manifest naming it, dot-directories skipped by the scan, and every
//! failure fail-soft — one bad plugin is skipped with its reason recorded,
//! and the rest load.
//!
//! ## Versioning
//!
//! [`API_VERSION`] is the single lever. A manifest declaring an older
//! revision keeps working, because every revision may only add optional
//! fields; one declaring a newer revision is refused whole rather than
//! understood in part. Adding a *required* field, removing a field, or
//! changing what one means is a bump.

mod error;
mod manifest;

pub use error::{LoadErrorKind, PluginLoadError};
pub use manifest::{
    check_api_version, expand_capability_path, AnalyzerContribution, Capabilities,
    CommandContribution, Contributes, ContributionPoint, IconThemeContribution,
    LanguageServerContribution, PluginManifest, PreviewContribution, TestFrameworkContribution,
    WasmSection, ID_MAX_LEN, MANIFEST_FILE, PLUGIN_DIR_TOKEN,
};

/// The newest contract revision this build speaks.
///
/// Bump only for a change an older host could not honour: a new required
/// field, a removed field, or a field whose meaning changed. New optional
/// fields and new contribution points do not need one — an older host
/// ignores a point it does not know, which is exactly the intended
/// behaviour.
pub const API_VERSION: u32 = 1;

/// Sub-directory of the config directory plugins are read from.
pub const PLUGINS_DIR: &str = "plugins";

/// Crash markers live here, inside the plugins root but hidden from the
/// scan, which skips dot-directories.
pub const QUARANTINE_DIR: &str = ".quarantine";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_revision_is_supported_and_the_next_one_is_not() {
        assert!(check_api_version(API_VERSION).is_ok());
        assert_eq!(
            check_api_version(API_VERSION + 1),
            Err(LoadErrorKind::UnsupportedApiVersion(API_VERSION + 1))
        );
    }

    /// The quarantine directory has to be invisible to the plugin scan, or
    /// the marker directory would itself be read as a plugin. The scan
    /// skips dot-directories, so the name must start with a dot — the same
    /// coupling `syntax_core::runtime` relies on.
    #[test]
    fn the_quarantine_directory_is_hidden_from_the_scan() {
        assert!(QUARANTINE_DIR.starts_with('.'));
    }
}
