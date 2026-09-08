//! Discovery and lifecycle for plugins: the scan of
//! `<config_dir>/plugins`, the built-ins embedded in the binary, the
//! user's disabled list, and the live registry (ADR-0026).
//!
//! This is `syntax_core::runtime` plus `syntax_core::registry`, generalised
//! from languages to plugins, and it keeps their idioms because they have
//! already been paid for:
//!
//! * **Fail-soft, always.** One bad plugin is skipped, its reason recorded
//!   as a [`PluginLoadError`], and every other plugin still loads. A user
//!   with a broken plugin gets an editor with a row on the Plugins page,
//!   not an editor that will not start.
//! * **Scan outside the lock, swap the pointer.** [`reload`] builds the
//!   next [`PluginRegistry`] before it takes the write lock, so a reload
//!   never blocks a reader for longer than one pointer assignment, and a
//!   consumer holding the previous `Arc` keeps working untouched.
//! * **`config_dir` is a parameter.** This crate has no `dirs` dependency
//!   and does not get one — the caller owns "where config lives".
//!
//! What it deliberately does not do: interpret a contribution. The
//! registry hands out [`IconThemeContribution`] and [`CommandContribution`]
//! payloads and has no idea what an icon theme is. `icon-theme` (P3) reads
//! them without depending on this crate, and the two are joined in
//! `app-core`.
//!
//! Discovery is declarative and knows nothing about running code. The
//! executable tier — instantiating a component, granting it capabilities,
//! running a contributed command under fuel, an epoch deadline and a
//! memory cap — is the `wasm` module, layered *on top of* this one:
//! [`WasmTier`] is built from a finished [`PluginRegistry`] and can only
//! ever start a plugin that discovery already accepted.
//!
//! ## Layout on disk
//!
//! ```text
//! <config_dir>/plugins/<plugin-id>/plugin.toml
//! <config_dir>/plugins/<plugin-id>/…            assets a contribution names
//! <config_dir>/plugins/.quarantine/<plugin-id>  crash marker
//! ```

mod builtins;
mod plugin;
mod wasm;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, RwLock};

use plugin_api::{
    AnalyzerContribution, ColorThemeContribution, CommandContribution, IconThemeContribution,
    LanguageServerContribution, LoadErrorKind, PluginLoadError, PluginManifest,
    PreviewContribution, TestFrameworkContribution, MANIFEST_FILE, QUARANTINE_DIR,
};

pub use plugin::{BuiltinPlugin, LoadedPlugin, PluginSource};
/// Re-exported because "where do installed plugins live" is a question
/// about the host, and a consumer that only asks it should not have to
/// depend on the contract crate to hear the answer.
pub use plugin_api::PLUGINS_DIR;
pub use wasm::{
    HostServices, LogLevel, StderrServices, WasmError, WasmLimits, WasmPreview, WasmPreviewImage,
    WasmTier,
};

/// The plugins shipped inside the binary.
///
/// [`load`] still takes `builtins` as an argument rather than reading this
/// constant, so a test can push its own fixtures through the real path
/// without the vendored 1.03 MB of Material SVGs in the way.
pub const BUILTIN_PLUGINS: &[BuiltinPlugin] = &[
    builtins::MATERIAL_ICON_THEME,
    builtins::MARKDOWN_PREVIEW,
    builtins::CSHARP,
    builtins::PHP_TOOLS,
];

/// Every plugin that loaded, and every one that did not.
#[derive(Debug, Default)]
pub struct PluginRegistry {
    plugins: Vec<LoadedPlugin>,
    errors: Vec<PluginLoadError>,
}

impl PluginRegistry {
    /// The loaded plugins, installed ones first, each group in id order.
    pub fn plugins(&self) -> &[LoadedPlugin] {
        &self.plugins
    }

    /// Why each skipped plugin was skipped, in the order they were met.
    ///
    /// A *disabled* plugin is not in here: the user's list is a filter,
    /// not a failure, and putting it here would make the Plugins page
    /// report the user's own choice as a problem.
    pub fn errors(&self) -> &[PluginLoadError] {
        &self.errors
    }

    /// The plugin with this id, if one loaded.
    pub fn by_id(&self, id: &str) -> Option<&LoadedPlugin> {
        self.plugins.iter().find(|plugin| plugin.id() == id)
    }

    /// Every `icon-themes` contribution, with the plugin that offers it —
    /// which is what the consumer needs in order to read the pack file the
    /// payload names.
    ///
    /// A typed accessor per contribution point rather than a map of erased
    /// payloads: the payload types are known at compile time, so `dyn Any`
    /// would buy nothing but a downcast that can fail.
    pub fn icon_themes(&self) -> impl Iterator<Item = (&LoadedPlugin, &IconThemeContribution)> {
        self.plugins.iter().flat_map(|plugin| {
            plugin
                .manifest()
                .contributes
                .icon_themes
                .iter()
                .map(move |theme| (plugin, theme))
        })
    }

    /// Every `color-themes` contribution, with the plugin that offers it.
    pub fn color_themes(&self) -> impl Iterator<Item = (&LoadedPlugin, &ColorThemeContribution)> {
        self.plugins.iter().flat_map(|plugin| {
            plugin
                .manifest()
                .contributes
                .color_themes
                .iter()
                .map(move |theme| (plugin, theme))
        })
    }

    /// Every `commands` contribution, with the plugin that offers it.
    pub fn commands(&self) -> impl Iterator<Item = (&LoadedPlugin, &CommandContribution)> {
        self.plugins.iter().flat_map(|plugin| {
            plugin
                .manifest()
                .contributes
                .commands
                .iter()
                .map(move |command| (plugin, command))
        })
    }

    /// Every `previews` contribution, with the plugin that offers it.
    pub fn previews(&self) -> impl Iterator<Item = (&LoadedPlugin, &PreviewContribution)> {
        self.plugins.iter().flat_map(|plugin| {
            plugin
                .manifest()
                .contributes
                .previews
                .iter()
                .map(move |preview| (plugin, preview))
        })
    }

    /// Every `language-servers` contribution, with the plugin that offers
    /// it.
    pub fn language_servers(
        &self,
    ) -> impl Iterator<Item = (&LoadedPlugin, &LanguageServerContribution)> {
        self.plugins.iter().flat_map(|plugin| {
            plugin
                .manifest()
                .contributes
                .language_servers
                .iter()
                .map(move |server| (plugin, server))
        })
    }

    /// Every `analyzers` contribution, with the plugin that offers it (the
    /// PHP tooling plan's B8).
    pub fn analyzers(&self) -> impl Iterator<Item = (&LoadedPlugin, &AnalyzerContribution)> {
        self.plugins.iter().flat_map(|plugin| {
            plugin
                .manifest()
                .contributes
                .analyzers
                .iter()
                .map(move |analyzer| (plugin, analyzer))
        })
    }

    /// Every `test-frameworks` contribution, with the plugin that offers
    /// it (the PHP tooling plan's D4).
    pub fn test_frameworks(
        &self,
    ) -> impl Iterator<Item = (&LoadedPlugin, &TestFrameworkContribution)> {
        self.plugins.iter().flat_map(|plugin| {
            plugin
                .manifest()
                .contributes
                .test_frameworks
                .iter()
                .map(move |framework| (plugin, framework))
        })
    }

    /// Take an id, or record why it cannot be taken twice.
    ///
    /// Installed beats built-in, and the built-in is recorded as the
    /// loser. That direction is the point of shadowing: replacing a
    /// bundled plugin with a newer or patched copy is the only way a user
    /// can fix one without a new build of the editor. Two *installed*
    /// plugins cannot collide — the id is the directory name — so the
    /// remaining case is a built-in arriving after the scan.
    fn claim(&mut self, plugin: LoadedPlugin, dir: PathBuf) -> bool {
        if self.by_id(plugin.id()).is_some() {
            self.errors.push(PluginLoadError {
                id: plugin.id().to_string(),
                dir,
                kind: LoadErrorKind::DuplicateId,
            });
            return false;
        }
        self.plugins.push(plugin);
        true
    }
}

/// Scan `<config_dir>/plugins`, add `builtins`, and drop everything named
/// in `disabled`.
///
/// A missing plugins directory is not an error — it means the user has
/// installed nothing.
///
/// `builtins` is a parameter rather than [`BUILTIN_PLUGINS`] so a test can
/// exercise the embedded path with a small fixture; `reload` is the entry
/// point that passes the real catalog.
pub fn load(config_dir: &Path, builtins: &[BuiltinPlugin], disabled: &[String]) -> PluginRegistry {
    let root = config_dir.join(PLUGINS_DIR);
    let quarantine = root.join(QUARANTINE_DIR);
    let mut registry = PluginRegistry::default();

    for dir in installed_dirs(&root) {
        let name = dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        // Disabling is checked on the directory name, before the manifest
        // is even opened, so that disabling a *broken* plugin silences it
        // completely instead of leaving its error row on the page. The
        // directory name is a safe key to do that with because a mismatch
        // between it and the manifest id is refused below.
        if disabled.contains(&name) {
            continue;
        }
        match load_installed(&dir, &name, &quarantine) {
            Ok(plugin) => {
                registry.claim(plugin, dir);
            }
            Err(kind) => registry.errors.push(PluginLoadError {
                id: name,
                dir,
                kind,
            }),
        }
    }

    for builtin in builtins {
        // A built-in has no directory; the placeholder is what an error
        // row shows in the "where" column.
        let dir = PathBuf::from("<built-in>");
        match load_builtin(builtin, &quarantine, disabled) {
            Ok(Some(plugin)) => {
                registry.claim(plugin, dir);
            }
            Ok(None) => {}
            Err((id, kind)) => registry.errors.push(PluginLoadError { id, dir, kind }),
        }
    }

    registry
}

/// The plugin directories under `root`, in a stable order.
///
/// Dot-directories are skipped, which is what hides `.quarantine` — the
/// markers live inside the plugins root and must never be read as a
/// plugin. `plugin-api` asserts that coupling in its own tests.
fn installed_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        // Missing, or unreadable: either way the editor runs on its
        // built-ins alone, which is a working editor.
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(|name| name.as_encoded_bytes().starts_with(b"."))
        })
        .collect();
    dirs.sort();
    dirs
}

fn load_installed(
    dir: &Path,
    dir_name: &str,
    quarantine: &Path,
) -> Result<LoadedPlugin, LoadErrorKind> {
    let manifest_path = dir.join(MANIFEST_FILE);
    let text = fs::read_to_string(&manifest_path).map_err(|err| LoadErrorKind::Unreadable {
        file: manifest_path.display().to_string(),
        message: err.to_string(),
    })?;
    let manifest = PluginManifest::from_toml_str(&text)?;

    // The directory name and the declared id must agree, and the manifest
    // is not allowed to win: `disabled_plugins`, the settings page and the
    // quarantine marker all key on the id, so a plugin whose id is not its
    // directory name would be one the user cannot disable by name and the
    // host cannot quarantine by path. Reported as a malformed manifest
    // because that is what it is — the file says something untrue about
    // where it lives.
    if manifest.id != dir_name {
        return Err(LoadErrorKind::MalformedManifest(format!(
            "`id` is `{}` but the plugin directory is named `{dir_name}`; they must match",
            manifest.id
        )));
    }
    check_quarantine(quarantine, &manifest.id)?;
    Ok(LoadedPlugin::installed(manifest, dir.to_path_buf()))
}

/// Parse one embedded plugin. `Ok(None)` means the user disabled it.
fn load_builtin(
    builtin: &BuiltinPlugin,
    quarantine: &Path,
    disabled: &[String],
) -> Result<Option<LoadedPlugin>, (String, LoadErrorKind)> {
    // A built-in has no directory name to key on, so its manifest has to
    // be parsed before it can be matched against the disabled list.
    let manifest =
        PluginManifest::from_toml_str(builtin.manifest).map_err(|kind| (String::new(), kind))?;
    if disabled.contains(&manifest.id) {
        return Ok(None);
    }
    if let Err(kind) = check_quarantine(quarantine, &manifest.id) {
        return Err((manifest.id, kind));
    }
    Ok(Some(LoadedPlugin::builtin(manifest, builtin)))
}

/// A marker at `<config_dir>/plugins/.quarantine/<id>` means this plugin
/// was loading when the editor last died, so it is disabled until the user
/// deletes the file.
///
/// Only *reading* markers lives here, and nothing in this crate writes
/// one. That is a conclusion, not an omission (ADR-0028): the marker
/// mechanism was designed in `syntax_core::runtime` for `dlopen`, where
/// foreign native code can take the process down before any error can be
/// returned. A component cannot. Every failure the wasm tier can have —
/// a malformed component, a failed instantiation, a trap — arrives as a
/// `Result` on the host's side of the sandbox, and is answered by
/// disabling that one plugin. There is no window where the editor dies
/// with a plugin half-loaded, so there is nothing to mark. A user can
/// still write a marker by hand, and a future tier that runs native code
/// would write one around exactly its own window.
fn check_quarantine(quarantine: &Path, id: &str) -> Result<(), LoadErrorKind> {
    let marker = quarantine.join(id);
    if marker.exists() {
        Err(LoadErrorKind::Quarantined { marker })
    } else {
        Ok(())
    }
}

/// The live registry.
///
/// `RwLock<Arc<_>>` rather than `RwLock<PluginRegistry>` for the reason
/// `syntax_core::registry` gives: every read clones the `Arc` and drops
/// the lock immediately, so no lock is ever held across a caller's work,
/// and a reload swaps a freshly built pointer in without disturbing whoever
/// is still using the old snapshot.
static REGISTRY: LazyLock<RwLock<Arc<PluginRegistry>>> =
    LazyLock::new(|| RwLock::new(Arc::new(PluginRegistry::default())));

/// A snapshot of the current registry. Cheap (one `Arc` clone).
pub fn registry() -> Arc<PluginRegistry> {
    REGISTRY
        .read()
        .expect("plugin registry lock poisoned")
        .clone()
}

/// Re-scan `<config_dir>/plugins` and swap in a registry built from the
/// built-ins plus what it finds, minus the ids in `disabled`. Returns the
/// load errors so the Plugins page can show them; an empty vec means
/// everything loaded.
///
/// Safe to call with the editor running: the scan happens before the write
/// lock is taken, so a reload blocks a reader for one pointer assignment
/// and no longer, and anything holding the previous snapshot keeps using
/// it until it is done.
pub fn reload(config_dir: &Path, disabled: &[String]) -> Vec<PluginLoadError> {
    let rebuilt = load(config_dir, BUILTIN_PLUGINS, disabled);
    let errors = rebuilt.errors.clone();
    *REGISTRY.write().expect("plugin registry lock poisoned") = Arc::new(rebuilt);
    errors
}

/// The wasm tier this process is running, in the same shape and for the
/// same reasons as [`REGISTRY`] above.
///
/// It starts empty rather than lazily starting itself over whatever the
/// registry happens to hold: activating a component runs the plugin's own
/// code, and that must happen at a moment the application chose, not the
/// first time something asks a question about it.
static TIER: LazyLock<RwLock<Arc<WasmTier>>> =
    LazyLock::new(|| RwLock::new(Arc::new(WasmTier::default())));

/// The running tier. Cheap (one `Arc` clone).
pub fn tier() -> Arc<WasmTier> {
    TIER.read().expect("plugin tier lock poisoned").clone()
}

/// Start — or restart — the tier over the registry as it stands now, and
/// return it.
///
/// Called after [`reload`], because the two are one fact: the tier's slots
/// come from the registry's manifests, and a tier built over a registry
/// that has since been swapped would keep running a plugin the user just
/// disabled. Dropping the previous tier is what deactivates those plugins,
/// and it happens here rather than being something a caller must remember.
///
/// `services` is the host's side of the sandbox — where a plugin's `log`
/// and `notify` go. [`StderrServices`] is the honest default until there is
/// a UI surface to route them to.
pub fn start_tier(services: Arc<dyn HostServices>, limits: WasmLimits) -> Arc<WasmTier> {
    let started = Arc::new(WasmTier::start(registry(), services, limits));
    *TIER.write().expect("plugin tier lock poisoned") = Arc::clone(&started);
    started
}

#[cfg(test)]
mod tests;
