//! `plugin.toml` as written on disk, plus every rule that can be checked
//! without touching the filesystem.
//!
//! Validation lives here rather than in `plugin-host` on purpose: these are
//! rules about what a manifest *means*, they deserve unit tests, and the
//! host should be left with the parts that genuinely need a disk — reading
//! directories, opening components, granting capabilities.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::error::LoadErrorKind;
use crate::API_VERSION;

/// File a plugin directory is recognised by.
pub const MANIFEST_FILE: &str = "plugin.toml";

/// Longest an id may be. Long enough for `org.example.some-plugin`, short
/// enough that it can never be mistaken for a path.
pub const ID_MAX_LEN: usize = 64;

/// The one substitution a capability path may use: the plugin's own
/// directory, filled in by the host at grant time.
pub const PLUGIN_DIR_TOKEN: &str = "${plugin_dir}";

/// The extension points this contract defines.
///
/// A point is a name plus a payload shape; nothing here knows how a
/// contribution is *used*. `icon-themes` is consumed by `icon-theme`,
/// `commands` by the host's wasm tier — neither crate is named here, which
/// is what keeps this crate a leaf.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContributionPoint {
    IconThemes,
    ColorThemes,
    Commands,
    Previews,
    LanguageServers,
    Analyzers,
    TestFrameworks,
}

impl ContributionPoint {
    /// The key this point appears under in `[contributes]`.
    pub const fn key(self) -> &'static str {
        match self {
            Self::IconThemes => "icon-themes",
            Self::ColorThemes => "color-themes",
            Self::Commands => "commands",
            Self::Previews => "previews",
            Self::LanguageServers => "language-servers",
            Self::Analyzers => "analyzers",
            Self::TestFrameworks => "test-frameworks",
        }
    }
}

/// One icon theme a plugin offers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IconThemeContribution {
    /// Stable id, persisted in `settings.toml` as the chosen icon theme.
    pub id: String,
    /// What the settings page shows.
    pub label: String,
    /// The pack description, relative to the plugin directory.
    pub pack: PathBuf,
}

/// One colour theme a plugin offers.
///
/// The id is what `settings.toml` persists as the chosen colour theme.
/// `path` is the theme file, relative to the plugin directory; its
/// extension decides which parser reads it (`.toml` for the native
/// format, `.json` for a VS Code theme) — that dispatch is a caller
/// concern (`app-core`), not this crate's.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ColorThemeContribution {
    /// Stable id, persisted in `settings.toml` as the chosen colour theme.
    pub id: String,
    /// What the settings page shows.
    pub label: String,
    /// The theme file, relative to the plugin directory.
    pub path: PathBuf,
}

/// One command a plugin offers.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandContribution {
    /// Stable id, handed back to the component as the argument of
    /// `on-command`.
    pub id: String,
    /// What the command palette shows.
    pub title: String,
}

/// One document preview a plugin offers.
///
/// Unlike [`CommandContribution`], a preview needs no `[wasm]` component:
/// the built-in Markdown preview is served by a native renderer the host
/// already ships, and a manifest naming a preview with no component is not
/// an error, in contrast to `CommandsWithoutComponent`. A component is
/// still how a *third-party* preview renders — the host tries it when both
/// are present.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreviewContribution {
    /// Stable id, handed to the host's own renderer table or to the guest's
    /// `render` export.
    pub id: String,
    /// What the Preview dock's empty state and the Plugins page show.
    pub label: String,
    /// File extensions this preview claims, lowercase and without the
    /// leading dot (`"md"`, not `".md"` or `"MD"`).
    pub extensions: Vec<String>,
}

/// One language server a plugin offers.
///
/// Unlike [`CommandContribution`], this needs no `[wasm]` component: the
/// server is a native process the host launches by `command`/`args`, the
/// same shape `lsp_core::catalog::ServerDef` already has for the built-in
/// table. What a manifest adds over that table is the two fields a const
/// `&'static str` slice cannot carry: a settings section a server pulls
/// over `workspace/configuration`, and the defaults for it.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageServerContribution {
    /// Stable id, distinct from `language_id` — a language may one day have
    /// more than one server on offer.
    pub id: String,
    /// LSP language id this server serves, e.g. `"csharp"`. The catalog key
    /// `lsp_core::enabled_server` looks up by.
    #[serde(rename = "language-id")]
    pub language_id: String,
    /// What the Language Servers page shows.
    pub name: String,
    /// Executable, looked up on `PATH` — nothing here is installed by the
    /// host, matching `ServerDef`'s rule.
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// The `workspace/configuration` section this server's settings answer
    /// under, e.g. `"csharp"`. Absent means the server takes no pulled
    /// configuration.
    #[serde(default, rename = "settings-section")]
    pub settings_section: Option<String>,
    /// Default settings for `settings_section`, overridable by the user.
    /// Free-form because each server defines its own shape; sent to the
    /// server as JSON, never interpreted here.
    #[serde(default)]
    pub settings: toml::value::Table,
}

/// One static analyzer a plugin offers (the PHP tooling plan's B2).
///
/// Unlike [`CommandContribution`], this needs no `[wasm]` component: a wasm
/// guest can neither spawn a process nor be trusted with a linter's raw
/// bytes (`wit/plugin.wit`), so an analyzer is native process launch data,
/// the same shape [`LanguageServerContribution`] already has. What a manifest
/// adds over that shape is the two fields a language-server row has no use
/// for: several candidate programs to probe (a Composer project's tool
/// lives at `vendor/bin/phpstan`, a global install just as `phpstan`), and
/// an output *format id* that resolves to a parser `analysis-core` owns —
/// this crate has no opinion on what checkstyle-xml or TeamCity look like,
/// only that a manifest names one.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AnalyzerContribution {
    /// Stable id, e.g. `"phpstan"`.
    pub id: String,
    /// What the Analysis settings page and Problems dock's Source column
    /// show.
    pub name: String,
    /// Programs to probe, in order — the first one detection finds wins.
    /// Bare names are looked up on `PATH`; a path relative to the project
    /// root (e.g. `vendor/bin/phpstan`) is looked up there instead. At
    /// least one candidate is required, or nothing could ever be detected.
    #[serde(rename = "program-candidates")]
    pub program_candidates: Vec<String>,
    /// Fixed arguments before whatever `analysis-core` appends for a given
    /// run.
    #[serde(default)]
    pub args: Vec<String>,
    /// Id of the parser in `analysis-core` that understands this tool's
    /// output, e.g. `"checkstyle-xml"`. Free-form here because the parser
    /// table is native code this crate never sees, exactly as
    /// [`LanguageServerContribution::settings`] is free-form JSON the
    /// server alone interprets.
    #[serde(rename = "output-format")]
    pub output_format: String,
    /// The tool's own severity vocabulary (e.g. PHPCS's `"error"`/
    /// `"warning"`), mapped to this crate's neutral spelling. Free-form
    /// strings rather than `diagnostics_core::Severity` directly — this
    /// crate stays a leaf and does not depend on `diagnostics-core` — so
    /// `analysis-core` parses the value side into its own `Severity`.
    #[serde(default, rename = "severity-map")]
    pub severity_map: BTreeMap<String, String>,
}

/// One test framework a plugin offers (the PHP tooling plan's D1).
///
/// Shaped after [`AnalyzerContribution`] for the same reason: a wasm guest
/// can neither spawn a process nor be trusted with a test runner's raw
/// bytes, so this is native process launch data too. What a manifest adds
/// over an analyzer's shape is the two fields a test run needs that a
/// one-shot lint never does: a spelling for "run only this subset"
/// ([`Self::filter_flag`]), because rerunning one failed test is the whole
/// point of a Tests dock, and a list of config filenames to discover
/// ([`Self::config_file_candidates`]), so the settings page can show which
/// `phpunit.xml` a project's run actually uses.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestFrameworkContribution {
    /// Stable id, e.g. `"phpunit"`.
    pub id: String,
    /// What the Tests dock and settings page show.
    pub name: String,
    /// Programs to probe, in order — the first one detection finds wins.
    /// Same rule as [`AnalyzerContribution::program_candidates`]: a bare
    /// name is looked up on `PATH`, a path relative to the project root is
    /// looked up there instead.
    #[serde(rename = "program-candidates")]
    pub program_candidates: Vec<String>,
    /// Fixed arguments every run gets, before whatever a specific run adds
    /// (a filter, a report path), e.g. `["--teamcity"]`.
    #[serde(default)]
    pub args: Vec<String>,
    /// The flag that selects a subset of tests, e.g. `"--filter"`. Rerun
    /// (D6) appends `[filter_flag, pattern]` to [`Self::args`]; `pattern`
    /// is built from the node the user reruns, in the tool's own `--filter`
    /// regex dialect, which this crate has no opinion on.
    #[serde(rename = "filter-flag")]
    pub filter_flag: String,
    /// Id of the parser in `test-core` that understands this tool's
    /// streaming output, e.g. `"teamcity"`. Free-form here for the same
    /// reason [`AnalyzerContribution::output_format`] is: the parser table
    /// is native code this crate never sees.
    #[serde(rename = "output-format")]
    pub output_format: String,
    /// Filenames to look for in the project root, in preference order
    /// (`["phpunit.xml", "phpunit.xml.dist"]`), so the settings page can
    /// report which one a run would actually use. Empty is valid — some
    /// test frameworks have no project-level config file at all.
    #[serde(default, rename = "config-file-candidates")]
    pub config_file_candidates: Vec<String>,
}

/// Everything a plugin contributes, by point.
///
/// Deliberately *not* `deny_unknown_fields`: [`API_VERSION`]'s doc comment
/// promises that an older host ignores a contribution point it does not
/// recognise rather than refusing the whole manifest, and every other
/// struct in this module enforces the opposite rule — a typo in a *known*
/// field is still a load error. `unknown` is where a point this build has
/// never heard of goes to be silently dropped; nothing reads it.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct Contributes {
    #[serde(default, rename = "icon-themes")]
    pub icon_themes: Vec<IconThemeContribution>,
    #[serde(default, rename = "color-themes")]
    pub color_themes: Vec<ColorThemeContribution>,
    #[serde(default)]
    pub commands: Vec<CommandContribution>,
    #[serde(default)]
    pub previews: Vec<PreviewContribution>,
    #[serde(default, rename = "language-servers")]
    pub language_servers: Vec<LanguageServerContribution>,
    #[serde(default)]
    pub analyzers: Vec<AnalyzerContribution>,
    #[serde(default, rename = "test-frameworks")]
    pub test_frameworks: Vec<TestFrameworkContribution>,
    #[serde(flatten)]
    unknown: BTreeMap<String, toml::Value>,
}

impl Contributes {
    /// True when the plugin contributes nothing at all — a manifest that
    /// declares no contributions and no component does nothing, but it is
    /// not an error: it is how a plugin is emptied out without deleting it.
    pub fn is_empty(&self) -> bool {
        self.icon_themes.is_empty()
            && self.color_themes.is_empty()
            && self.commands.is_empty()
            && self.previews.is_empty()
            && self.language_servers.is_empty()
            && self.analyzers.is_empty()
            && self.test_frameworks.is_empty()
    }
}

/// The executable half of a plugin: a WebAssembly component.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WasmSection {
    /// The component, relative to the plugin directory.
    pub component: PathBuf,
}

/// What the component is allowed to reach outside its own sandbox.
///
/// Absent means "nothing": a component with no `[capabilities]` can log and
/// nothing else. Every field is additive and defaults to the closed state,
/// so a capability can only ever be gained by naming it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    /// Path prefixes the component may read, each of which must start with
    /// [`PLUGIN_DIR_TOKEN`] in version 1.
    #[serde(default, rename = "read-files")]
    pub read_files: Vec<String>,
    /// May raise a user-visible notification.
    #[serde(default)]
    pub notify: bool,
    /// May ask for the open project's root path.
    #[serde(default, rename = "workspace-root")]
    pub workspace_root: bool,
}

/// `plugin.toml`, parsed and validated.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginManifest {
    /// Stable id. Also the directory name a plugin is installed under.
    pub id: String,
    /// Display name for the Plugins page.
    pub name: String,
    /// The plugin's own version, shown but never interpreted — ordering
    /// plugin versions is a package-manager problem this build does not
    /// have.
    pub version: String,
    /// Which revision of this contract the manifest is written against.
    pub api_version: u32,
    /// SPDX identifier, shown on the Plugins page. Optional because a
    /// local, private plugin has no licence to declare.
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub contributes: Contributes,
    #[serde(default)]
    pub wasm: Option<WasmSection>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

impl PluginManifest {
    /// Parse and validate one manifest.
    ///
    /// The two steps are deliberately not separable from outside: a
    /// `PluginManifest` that exists has been validated, so no later caller
    /// has to wonder whether it was.
    pub fn from_toml_str(text: &str) -> Result<Self, LoadErrorKind> {
        let manifest: Self = toml::from_str(text)
            .map_err(|err| LoadErrorKind::MalformedManifest(err.message().to_string()))?;
        manifest.validate()?;
        Ok(manifest)
    }

    /// Path of this plugin's component, relative to its directory, if it
    /// has one.
    pub fn component_path(&self) -> Option<&Path> {
        self.wasm.as_ref().map(|wasm| wasm.component.as_path())
    }

    fn validate(&self) -> Result<(), LoadErrorKind> {
        check_api_version(self.api_version)?;
        check_id("id", &self.id)?;
        non_empty("name", &self.name)?;
        non_empty("version", &self.version)?;

        for theme in &self.contributes.icon_themes {
            check_id("contributes.icon-themes.id", &theme.id)?;
            non_empty("contributes.icon-themes.label", &theme.label)?;
            check_relative("contributes.icon-themes.pack", &theme.pack)?;
        }
        check_unique(
            ContributionPoint::IconThemes,
            self.contributes.icon_themes.iter().map(|t| t.id.as_str()),
        )?;

        for theme in &self.contributes.color_themes {
            check_id("contributes.color-themes.id", &theme.id)?;
            non_empty("contributes.color-themes.label", &theme.label)?;
            check_relative("contributes.color-themes.path", &theme.path)?;
        }
        check_unique(
            ContributionPoint::ColorThemes,
            self.contributes.color_themes.iter().map(|t| t.id.as_str()),
        )?;

        for command in &self.contributes.commands {
            check_id("contributes.commands.id", &command.id)?;
            non_empty("contributes.commands.title", &command.title)?;
        }
        check_unique(
            ContributionPoint::Commands,
            self.contributes.commands.iter().map(|c| c.id.as_str()),
        )?;

        for preview in &self.contributes.previews {
            check_id("contributes.previews.id", &preview.id)?;
            non_empty("contributes.previews.label", &preview.label)?;
            if preview.extensions.is_empty() {
                return Err(LoadErrorKind::EmptyField("contributes.previews.extensions"));
            }
            for extension in &preview.extensions {
                check_extension(extension)?;
            }
        }
        check_unique(
            ContributionPoint::Previews,
            self.contributes.previews.iter().map(|p| p.id.as_str()),
        )?;

        // Unlike `commands`, a `previews` contribution needs no `[wasm]`
        // component: it may be served entirely by the host's own native
        // renderer table (the built-in Markdown preview is). A component is
        // only how a *third-party* preview renders, so its absence here is
        // never `CommandsWithoutComponent`'s twin.

        for server in &self.contributes.language_servers {
            check_id("contributes.language-servers.id", &server.id)?;
            non_empty(
                "contributes.language-servers.language-id",
                &server.language_id,
            )?;
            non_empty("contributes.language-servers.name", &server.name)?;
            non_empty("contributes.language-servers.command", &server.command)?;
        }
        check_unique(
            ContributionPoint::LanguageServers,
            self.contributes
                .language_servers
                .iter()
                .map(|s| s.id.as_str()),
        )?;

        // A language server needs no `[wasm]` component either: it is a
        // native process launched by `command`/`args`, the same shape the
        // built-in catalog table already has.

        for analyzer in &self.contributes.analyzers {
            check_id("contributes.analyzers.id", &analyzer.id)?;
            non_empty("contributes.analyzers.name", &analyzer.name)?;
            if analyzer.program_candidates.is_empty() {
                return Err(LoadErrorKind::EmptyField(
                    "contributes.analyzers.program-candidates",
                ));
            }
            for candidate in &analyzer.program_candidates {
                non_empty("contributes.analyzers.program-candidates", candidate)?;
            }
            non_empty(
                "contributes.analyzers.output-format",
                &analyzer.output_format,
            )?;
        }
        check_unique(
            ContributionPoint::Analyzers,
            self.contributes.analyzers.iter().map(|a| a.id.as_str()),
        )?;

        // An analyzer needs no `[wasm]` component either, for the same
        // reason a language server doesn't: it is a native process, and a
        // wasm guest cannot spawn one (`wit/plugin.wit`).

        for framework in &self.contributes.test_frameworks {
            check_id("contributes.test-frameworks.id", &framework.id)?;
            non_empty("contributes.test-frameworks.name", &framework.name)?;
            if framework.program_candidates.is_empty() {
                return Err(LoadErrorKind::EmptyField(
                    "contributes.test-frameworks.program-candidates",
                ));
            }
            for candidate in &framework.program_candidates {
                non_empty("contributes.test-frameworks.program-candidates", candidate)?;
            }
            non_empty(
                "contributes.test-frameworks.filter-flag",
                &framework.filter_flag,
            )?;
            non_empty(
                "contributes.test-frameworks.output-format",
                &framework.output_format,
            )?;
        }
        check_unique(
            ContributionPoint::TestFrameworks,
            self.contributes
                .test_frameworks
                .iter()
                .map(|t| t.id.as_str()),
        )?;

        // Same reasoning again: a test framework is a native process, not a
        // wasm guest.

        if let Some(wasm) = &self.wasm {
            check_relative("wasm.component", &wasm.component)?;
        } else if !self.contributes.commands.is_empty() {
            return Err(LoadErrorKind::CommandsWithoutComponent);
        }

        for pattern in &self.capabilities.read_files {
            check_capability_path(pattern)?;
        }
        Ok(())
    }
}

/// Is `version` a contract revision this build speaks?
///
/// Older manifests keep working: every revision so far only ever added
/// optional fields, and `serde` defaults fill in what an old manifest does
/// not name. A *newer* manifest is refused outright — the alternative is
/// loading half of it and silently dropping the parts that carry the
/// meaning.
pub fn check_api_version(version: u32) -> Result<(), LoadErrorKind> {
    if (1..=API_VERSION).contains(&version) {
        Ok(())
    } else {
        Err(LoadErrorKind::UnsupportedApiVersion(version))
    }
}

/// Resolve one capability path pattern against a plugin's directory.
///
/// The pattern has already been checked by [`check_capability_path`], so
/// the token is known to be the prefix; what is left is the join.
pub fn expand_capability_path(pattern: &str, plugin_dir: &Path) -> PathBuf {
    match pattern.strip_prefix(PLUGIN_DIR_TOKEN) {
        Some(rest) => {
            let rest = rest.trim_start_matches(['/', '\\']);
            if rest.is_empty() {
                plugin_dir.to_path_buf()
            } else {
                plugin_dir.join(rest)
            }
        }
        None => plugin_dir.to_path_buf(),
    }
}

/// Ids double as directory names and as settings keys, so the charset is
/// the narrow one both can carry losslessly.
fn check_id(field: &'static str, value: &str) -> Result<(), LoadErrorKind> {
    let malformed = || LoadErrorKind::MalformedId {
        field,
        value: value.to_string(),
    };
    if value.is_empty() || value.len() > ID_MAX_LEN {
        return Err(malformed());
    }
    let mut chars = value.chars();
    let first = chars.next().expect("id is not empty");
    if !first.is_ascii_lowercase() && !first.is_ascii_digit() {
        return Err(malformed());
    }
    if chars
        .any(|c| !(c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '.' | '_' | '-')))
    {
        return Err(malformed());
    }
    Ok(())
}

fn non_empty(field: &'static str, value: &str) -> Result<(), LoadErrorKind> {
    if value.trim().is_empty() {
        Err(LoadErrorKind::EmptyField(field))
    } else {
        Ok(())
    }
}

/// A manifest may only ever point at files inside its own directory.
///
/// Both halves matter: an absolute path escapes by ignoring the plugin
/// directory entirely, and a `..` component escapes by climbing out of it.
fn check_relative(field: &'static str, path: &Path) -> Result<(), LoadErrorKind> {
    let unsafe_path = || LoadErrorKind::UnsafePath {
        field,
        value: path.display().to_string(),
    };
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(unsafe_path());
    }
    let mut components = path.components().peekable();
    if components.peek().is_none() {
        return Err(unsafe_path());
    }
    for component in components {
        match component {
            std::path::Component::Normal(_) | std::path::Component::CurDir => {}
            _ => return Err(unsafe_path()),
        }
    }
    Ok(())
}

/// Version 1 grants reads inside the plugin's own directory and nowhere
/// else, so the token is not a convenience — it is the whole grammar.
fn check_capability_path(pattern: &str) -> Result<(), LoadErrorKind> {
    let unscoped = || LoadErrorKind::UnscopedCapabilityPath(pattern.to_string());
    let Some(rest) = pattern.strip_prefix(PLUGIN_DIR_TOKEN) else {
        return Err(unscoped());
    };
    let rest = rest.trim_start_matches(['/', '\\']);
    if rest.is_empty() {
        return Ok(());
    }
    check_relative("capabilities.read-files", Path::new(rest)).map_err(|_| unscoped())
}

fn check_unique<'a>(
    point: ContributionPoint,
    ids: impl Iterator<Item = &'a str>,
) -> Result<(), LoadErrorKind> {
    let mut seen: Vec<&str> = Vec::new();
    for id in ids {
        if seen.contains(&id) {
            return Err(LoadErrorKind::DuplicateContributionId {
                point: point.key(),
                id: id.to_string(),
            });
        }
        seen.push(id);
    }
    Ok(())
}

/// An extension is what the Preview dock keys a provider by, so it is held
/// to a narrower charset than an id: lowercase ASCII letters and digits
/// only, no leading dot (`"md"`, never `".md"`), no path separator, and
/// short enough that a typo reads as a typo rather than a path.
const EXTENSION_MAX_LEN: usize = 16;

fn check_extension(value: &str) -> Result<(), LoadErrorKind> {
    let ok = !value.is_empty()
        && value.len() <= EXTENSION_MAX_LEN
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    if ok {
        Ok(())
    } else {
        Err(LoadErrorKind::InvalidExtension(value.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MINIMAL: &str = r#"
        id = "material-icons"
        name = "Material Icon Theme"
        version = "5.38.1"
        api_version = 1
    "#;

    fn with(extra: &str) -> String {
        format!("{MINIMAL}\n{extra}")
    }

    #[test]
    fn a_minimal_manifest_parses() {
        let manifest = PluginManifest::from_toml_str(MINIMAL).expect("valid");
        assert_eq!(manifest.id, "material-icons");
        assert!(manifest.contributes.is_empty());
        assert!(manifest.wasm.is_none());
        assert!(manifest.capabilities.read_files.is_empty());
        assert!(!manifest.capabilities.notify);
    }

    #[test]
    fn an_icon_theme_contribution_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "pack.toml"
            "#,
        ))
        .expect("valid");
        let themes = &manifest.contributes.icon_themes;
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].id, "material");
        assert_eq!(themes[0].label, "Material");
        assert_eq!(themes[0].pack, PathBuf::from("pack.toml"));
    }

    #[test]
    fn a_color_theme_contribution_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.color-themes]]
            id = "midnight"
            label = "Midnight"
            path = "midnight.toml"
            "#,
        ))
        .expect("valid");
        let themes = &manifest.contributes.color_themes;
        assert_eq!(themes.len(), 1);
        assert_eq!(themes[0].id, "midnight");
        assert_eq!(themes[0].label, "Midnight");
        assert_eq!(themes[0].path, PathBuf::from("midnight.toml"));
        assert!(!manifest.contributes.is_empty());
        assert_eq!(ContributionPoint::ColorThemes.key(), "color-themes");
    }

    #[test]
    fn two_color_theme_contributions_may_not_claim_one_id() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.color-themes]]
            id = "midnight"
            label = "Midnight"
            path = "a.toml"

            [[contributes.color-themes]]
            id = "midnight"
            label = "Midnight, again"
            path = "b.toml"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::DuplicateContributionId {
                point: "color-themes",
                id: "midnight".to_string(),
            }
        );
    }

    #[test]
    fn a_typo_in_a_key_is_refused_rather_than_ignored() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.icon-themes]]
            id = "material"
            lable = "Material"
            pack = "pack.toml"
            "#,
        ))
        .unwrap_err();
        assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
    }

    #[test]
    fn a_newer_contract_is_refused_outright() {
        let text = MINIMAL.replace("api_version = 1", "api_version = 2");
        assert_eq!(
            PluginManifest::from_toml_str(&text).unwrap_err(),
            LoadErrorKind::UnsupportedApiVersion(2)
        );
        let text = MINIMAL.replace("api_version = 1", "api_version = 0");
        assert_eq!(
            PluginManifest::from_toml_str(&text).unwrap_err(),
            LoadErrorKind::UnsupportedApiVersion(0)
        );
    }

    #[test]
    fn an_id_that_could_climb_out_of_the_plugins_directory_is_refused() {
        for bad in ["../evil", "/etc", "Material", "café", "", &"x".repeat(65)] {
            let text = MINIMAL.replace("material-icons", bad);
            let err = PluginManifest::from_toml_str(&text).unwrap_err();
            assert!(
                matches!(err, LoadErrorKind::MalformedId { field: "id", .. }),
                "`{bad}` was accepted as an id: {err}"
            );
        }
    }

    #[test]
    fn an_id_may_be_dotted_lowercase() {
        let text = MINIMAL.replace("material-icons", "org.example.plugin_2");
        assert!(PluginManifest::from_toml_str(&text).is_ok());
    }

    #[test]
    fn a_pack_path_may_not_escape_the_plugin_directory() {
        for bad in ["/etc/passwd", "../../pack.toml", "themes/../../pack.toml"] {
            let err = PluginManifest::from_toml_str(&with(&format!(
                r#"
                [[contributes.icon-themes]]
                id = "material"
                label = "Material"
                pack = "{bad}"
                "#
            )))
            .unwrap_err();
            assert!(
                matches!(err, LoadErrorKind::UnsafePath { .. }),
                "`{bad}` was accepted as a pack path: {err}"
            );
        }
    }

    #[test]
    fn a_nested_pack_path_is_fine() {
        assert!(PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "themes/material/pack.toml"
            "#,
        ))
        .is_ok());
    }

    #[test]
    fn two_contributions_may_not_claim_one_id() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "a.toml"

            [[contributes.icon-themes]]
            id = "material"
            label = "Material Light"
            pack = "b.toml"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::DuplicateContributionId {
                point: "icon-themes",
                id: "material".to_string(),
            }
        );
    }

    #[test]
    fn commands_need_a_component_to_run_them() {
        let commands = r#"
            [[contributes.commands]]
            id = "example.hello"
            title = "Example: Hello"
        "#;
        assert_eq!(
            PluginManifest::from_toml_str(&with(commands)).unwrap_err(),
            LoadErrorKind::CommandsWithoutComponent
        );

        let manifest = PluginManifest::from_toml_str(&with(&format!(
            "{commands}\n[wasm]\ncomponent = \"plugin.wasm\"\n"
        )))
        .expect("valid");
        assert_eq!(
            manifest.component_path(),
            Some(Path::new("plugin.wasm")),
            "the component path is what the host opens"
        );
    }

    #[test]
    fn a_capability_path_must_be_scoped_to_the_plugin_directory() {
        for bad in ["/etc", "${workspace_root}/secrets", "../elsewhere"] {
            let err = PluginManifest::from_toml_str(&with(&format!(
                "[capabilities]\nread-files = [\"{bad}\"]\n"
            )))
            .unwrap_err();
            assert_eq!(err, LoadErrorKind::UnscopedCapabilityPath(bad.to_string()));
        }
    }

    #[test]
    fn a_capability_path_climbing_out_through_the_token_is_refused() {
        let bad = "${plugin_dir}/../../etc";
        let err = PluginManifest::from_toml_str(&with(&format!(
            "[capabilities]\nread-files = [\"{bad}\"]\n"
        )))
        .unwrap_err();
        assert_eq!(err, LoadErrorKind::UnscopedCapabilityPath(bad.to_string()));
    }

    #[test]
    fn a_scoped_capability_path_expands_under_the_plugin_directory() {
        let manifest = PluginManifest::from_toml_str(&with(
            "[capabilities]\nread-files = [\"${plugin_dir}/data\"]\nnotify = true\n",
        ))
        .expect("valid");
        assert!(manifest.capabilities.notify);
        let dir = Path::new("/home/u/.config/ide/plugins/material-icons");
        assert_eq!(
            expand_capability_path(&manifest.capabilities.read_files[0], dir),
            dir.join("data")
        );
        assert_eq!(expand_capability_path("${plugin_dir}", dir), dir);
    }

    #[test]
    fn contribution_point_keys_match_the_manifest_keys() {
        // The enum is what the host looks contributions up by; the strings
        // are what a plugin author writes. They must not drift.
        assert_eq!(ContributionPoint::IconThemes.key(), "icon-themes");
        assert_eq!(ContributionPoint::Commands.key(), "commands");
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "pack.toml"
            "#,
        ))
        .expect("valid");
        assert!(!manifest.contributes.is_empty());
    }

    #[test]
    fn a_previews_contribution_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = ["md", "markdown"]
            "#,
        ))
        .expect("valid");
        let previews = &manifest.contributes.previews;
        assert_eq!(previews.len(), 1);
        assert_eq!(previews[0].id, "markdown");
        assert_eq!(previews[0].label, "Markdown");
        assert_eq!(previews[0].extensions, vec!["md", "markdown"]);
        assert!(!manifest.contributes.is_empty());
    }

    #[test]
    fn a_previews_contribution_needs_at_least_one_extension() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = []
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::EmptyField("contributes.previews.extensions")
        );
    }

    #[test]
    fn an_extension_with_a_dot_or_a_separator_or_uppercase_is_rejected() {
        // A backslash is not exercised here: TOML's own string escaping
        // rejects a bare `\` before this code ever sees it, so that case is
        // already covered by `a_typo_in_a_key_is_refused_rather_than_ignored`'s
        // sibling, `MalformedManifest`, not `InvalidExtension`.
        for bad in [".md", "md/x", "MD", "m d", ""] {
            let err = PluginManifest::from_toml_str(&with(&format!(
                r#"
                [[contributes.previews]]
                id = "markdown"
                label = "Markdown"
                extensions = ["{bad}"]
                "#
            )))
            .unwrap_err();
            assert_eq!(
                err,
                LoadErrorKind::InvalidExtension(bad.to_string()),
                "`{bad}` should have been rejected"
            );
        }
    }

    #[test]
    fn duplicate_preview_ids_in_one_manifest_are_rejected() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = ["md"]

            [[contributes.previews]]
            id = "markdown"
            label = "Markdown, again"
            extensions = ["mkd"]
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::DuplicateContributionId {
                point: "previews",
                id: "markdown".to_string(),
            }
        );
    }

    #[test]
    fn previews_without_a_component_are_accepted_unlike_commands() {
        // The asymmetry with `commands_need_a_component_to_run_them` is the
        // point: a `previews` contribution may be served entirely by the
        // host's own native renderer table.
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.previews]]
            id = "markdown"
            label = "Markdown"
            extensions = ["md"]
            "#,
        ))
        .expect("valid");
        assert!(manifest.wasm.is_none());
    }

    #[test]
    fn an_unknown_contribution_point_is_ignored_not_an_error() {
        // The property `API_VERSION`'s doc comment promises: a manifest
        // naming a point this build has never heard of still loads, with
        // everything else about it intact.
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.icon-themes]]
            id = "material"
            label = "Material"
            pack = "pack.toml"

            [[contributes.some-future-point]]
            id = "whatever"
            "#,
        ))
        .expect("an unrecognised point must not fail the whole manifest");
        assert_eq!(manifest.contributes.icon_themes.len(), 1);
    }

    #[test]
    fn a_typo_in_a_known_previews_field_is_still_refused() {
        // `Contributes` dropped `deny_unknown_fields` so an *unrecognised
        // point* is tolerated; a typo inside a point this build does know
        // must still be a load error, or nobody would ever notice one.
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.previews]]
            id = "markdown"
            lable = "Markdown"
            extensions = ["md"]
            "#,
        ))
        .unwrap_err();
        assert!(matches!(err, LoadErrorKind::MalformedManifest(_)), "{err}");
    }

    #[test]
    fn a_language_server_contribution_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = "csharp-ls"
            args = ["--loglevel", "warning"]
            settings-section = "csharp"

            [contributes.language-servers.settings]
            analyzersEnabled = true
            "#,
        ))
        .expect("valid");
        let servers = &manifest.contributes.language_servers;
        assert_eq!(servers.len(), 1);
        assert_eq!(servers[0].id, "csharp-ls");
        assert_eq!(servers[0].language_id, "csharp");
        assert_eq!(servers[0].name, "csharp-ls");
        assert_eq!(servers[0].command, "csharp-ls");
        assert_eq!(servers[0].args, vec!["--loglevel", "warning"]);
        assert_eq!(servers[0].settings_section.as_deref(), Some("csharp"));
        assert_eq!(
            servers[0].settings.get("analyzersEnabled"),
            Some(&toml::Value::Boolean(true))
        );
        assert!(!manifest.contributes.is_empty());
        assert_eq!(ContributionPoint::LanguageServers.key(), "language-servers");
    }

    #[test]
    fn a_language_server_needs_no_wasm_component() {
        // Unlike `commands`, a language server is a native process the host
        // launches by `command`/`args` — the same shape `ServerDef` already
        // has for the built-in catalog table.
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = "csharp-ls"
            "#,
        ))
        .expect("valid");
        assert!(manifest.wasm.is_none());
    }

    #[test]
    fn a_language_server_without_a_command_is_rejected() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = ""
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::EmptyField("contributes.language-servers.command")
        );
    }

    #[test]
    fn duplicate_language_server_ids_in_one_manifest_are_rejected() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls"
            command = "csharp-ls"

            [[contributes.language-servers]]
            id = "csharp-ls"
            language-id = "csharp"
            name = "csharp-ls, again"
            command = "csharp-ls"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::DuplicateContributionId {
                point: "language-servers",
                id: "csharp-ls".to_string(),
            }
        );
    }

    #[test]
    fn an_analyzer_contribution_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["vendor/bin/phpstan", "phpstan"]
            args = ["analyse", "--error-format=checkstyle"]
            output-format = "checkstyle-xml"

            [contributes.analyzers.severity-map]
            error = "error"
            warning = "warning"
            "#,
        ))
        .expect("valid");
        let analyzers = &manifest.contributes.analyzers;
        assert_eq!(analyzers.len(), 1);
        assert_eq!(analyzers[0].id, "phpstan");
        assert_eq!(analyzers[0].name, "PHPStan");
        assert_eq!(
            analyzers[0].program_candidates,
            vec!["vendor/bin/phpstan", "phpstan"]
        );
        assert_eq!(
            analyzers[0].args,
            vec!["analyse", "--error-format=checkstyle"]
        );
        assert_eq!(analyzers[0].output_format, "checkstyle-xml");
        assert_eq!(
            analyzers[0].severity_map.get("error").map(String::as_str),
            Some("error")
        );
        assert!(!manifest.contributes.is_empty());
        assert_eq!(ContributionPoint::Analyzers.key(), "analyzers");
    }

    #[test]
    fn an_analyzer_needs_no_wasm_component() {
        // Same reasoning as a language server: a native process launched
        // by argv needs no sandboxed guest to run it.
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["phpstan"]
            output-format = "checkstyle-xml"
            "#,
        ))
        .expect("valid");
        assert!(manifest.wasm.is_none());
    }

    #[test]
    fn an_analyzer_needs_at_least_one_program_candidate() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = []
            output-format = "checkstyle-xml"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::EmptyField("contributes.analyzers.program-candidates")
        );
    }

    #[test]
    fn an_analyzer_without_an_output_format_is_rejected() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["phpstan"]
            output-format = ""
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::EmptyField("contributes.analyzers.output-format")
        );
    }

    #[test]
    fn duplicate_analyzer_ids_in_one_manifest_are_rejected() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan"
            program-candidates = ["phpstan"]
            output-format = "checkstyle-xml"

            [[contributes.analyzers]]
            id = "phpstan"
            name = "PHPStan, again"
            program-candidates = ["phpstan"]
            output-format = "checkstyle-xml"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::DuplicateContributionId {
                point: "analyzers",
                id: "phpstan".to_string(),
            }
        );
    }

    #[test]
    fn a_test_framework_contribution_round_trips() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["vendor/bin/phpunit", "phpunit"]
            args = ["--teamcity"]
            filter-flag = "--filter"
            output-format = "teamcity"
            config-file-candidates = ["phpunit.xml", "phpunit.xml.dist"]
            "#,
        ))
        .expect("valid");
        let frameworks = &manifest.contributes.test_frameworks;
        assert_eq!(frameworks.len(), 1);
        assert_eq!(frameworks[0].id, "phpunit");
        assert_eq!(frameworks[0].name, "PHPUnit");
        assert_eq!(
            frameworks[0].program_candidates,
            vec!["vendor/bin/phpunit", "phpunit"]
        );
        assert_eq!(frameworks[0].args, vec!["--teamcity"]);
        assert_eq!(frameworks[0].filter_flag, "--filter");
        assert_eq!(frameworks[0].output_format, "teamcity");
        assert_eq!(
            frameworks[0].config_file_candidates,
            vec!["phpunit.xml", "phpunit.xml.dist"]
        );
        assert!(!manifest.contributes.is_empty());
        assert_eq!(ContributionPoint::TestFrameworks.key(), "test-frameworks");
    }

    #[test]
    fn a_test_framework_needs_no_wasm_component() {
        let manifest = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            output-format = "teamcity"
            "#,
        ))
        .expect("valid");
        assert!(manifest.wasm.is_none());
    }

    #[test]
    fn a_test_framework_needs_at_least_one_program_candidate() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = []
            filter-flag = "--filter"
            output-format = "teamcity"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::EmptyField("contributes.test-frameworks.program-candidates")
        );
    }

    #[test]
    fn a_test_framework_needs_a_filter_flag() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = ""
            output-format = "teamcity"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::EmptyField("contributes.test-frameworks.filter-flag")
        );
    }

    #[test]
    fn duplicate_test_framework_ids_in_one_manifest_are_rejected() {
        let err = PluginManifest::from_toml_str(&with(
            r#"
            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            output-format = "teamcity"

            [[contributes.test-frameworks]]
            id = "phpunit"
            name = "PHPUnit, again"
            program-candidates = ["phpunit"]
            filter-flag = "--filter"
            output-format = "teamcity"
            "#,
        ))
        .unwrap_err();
        assert_eq!(
            err,
            LoadErrorKind::DuplicateContributionId {
                point: "test-frameworks",
                id: "phpunit".to_string(),
            }
        );
    }
}
