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
    BuildTools,
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
            Self::BuildTools => "build-tools",
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
    ///
    /// Exactly one of this and [`Self::filter_template`] must be set
    /// (jvm-build-tools plan, A1): a flag-based tool takes the pattern as a
    /// separate argument, a template-based one (Maven's `-Dtest={pattern}`)
    /// needs the pattern spliced into one argument string, and a manifest
    /// naming both or neither is ambiguous rather than merely redundant.
    #[serde(default, rename = "filter-flag")]
    pub filter_flag: Option<String>,
    /// A single argument template containing the literal `{pattern}`
    /// placeholder, e.g. `"-Dtest={pattern}"`. See [`Self::filter_flag`]
    /// for the mutual-exclusion rule.
    #[serde(default, rename = "filter-template")]
    pub filter_template: Option<String>,
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
    /// The [`BuildToolContribution::toolchain`] this framework needs to be
    /// runnable at all, e.g. `"gradle"`. Absent means no toolchain
    /// precondition (PHPUnit's case): detection just probes
    /// [`Self::program_candidates`] directly.
    #[serde(default, rename = "requires-toolchain")]
    pub requires_toolchain: Option<String>,
    /// A glob, relative to the run's working directory, of report files to
    /// read once the process exits — Maven's Surefire/Failsafe XML has no
    /// streaming format, so `output-format = "junit-xml"` frameworks read
    /// their result from disk instead of from stdout.
    #[serde(default, rename = "report-glob")]
    pub report_glob: Option<String>,
}

/// One build tool a plugin offers (the jvm-build-tools plan's A1).
///
/// Shaped after [`AnalyzerContribution`]: a native process, no `[wasm]`
/// component needed. What a manifest adds over an analyzer's shape is a
/// [`Self::toolchain`] id joining this contribution to
/// `run_core::ToolchainId` (ADR-0039's one toolchain table — this crate
/// stays a leaf and does not depend on `run-core`, so the join is a plain
/// string both sides agree on), the files whose presence marks a project as
/// using this tool, and an optional init script asset a sync provider runs
/// through the tool itself (Gradle's case; Maven needs none).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildToolContribution {
    /// Stable id, e.g. `"gradle"`.
    pub id: String,
    /// What the Build Tools dock and settings page show.
    pub name: String,
    /// `run_core::ToolchainId::as_str()` this contribution joins to.
    /// Free-form here, resolved with `ToolchainId::from_id` at the seam;
    /// an id this build does not recognise is a Plugins-page problem row,
    /// not a load error, since the set of known toolchains can grow
    /// without a manifest format change.
    pub toolchain: String,
    /// Marker files (project-relative, glob-capable, e.g. `"buildSrc/**"`)
    /// whose presence means a project uses this tool, and whose change
    /// triggers a reload per the project's auto-reload setting.
    #[serde(rename = "build-files")]
    pub build_files: Vec<String>,
    /// A text asset, relative to the plugin directory, a sync provider
    /// passes to the tool itself (Gradle's `--init-script`). Absent when
    /// the tool needs none (Maven's static/effective-pom reads).
    #[serde(default, rename = "init-script")]
    pub init_script: Option<PathBuf>,
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
    #[serde(default, rename = "build-tools")]
    pub build_tools: Vec<BuildToolContribution>,
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
            && self.build_tools.is_empty()
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
            match (&framework.filter_flag, &framework.filter_template) {
                (Some(flag), None) => non_empty("contributes.test-frameworks.filter-flag", flag)?,
                (None, Some(template)) => {
                    non_empty("contributes.test-frameworks.filter-template", template)?;
                    if !template.contains("{pattern}") {
                        return Err(LoadErrorKind::MalformedManifest(format!(
                            "contributes.test-frameworks.filter-template `{template}` must contain \
                             the literal `{{pattern}}` placeholder"
                        )));
                    }
                }
                (Some(_), Some(_)) | (None, None) => {
                    return Err(LoadErrorKind::MalformedManifest(
                        "contributes.test-frameworks must set exactly one of filter-flag or \
                         filter-template"
                            .to_string(),
                    ))
                }
            }
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

        for tool in &self.contributes.build_tools {
            check_id("contributes.build-tools.id", &tool.id)?;
            non_empty("contributes.build-tools.name", &tool.name)?;
            non_empty("contributes.build-tools.toolchain", &tool.toolchain)?;
            if tool.build_files.is_empty() {
                return Err(LoadErrorKind::EmptyField(
                    "contributes.build-tools.build-files",
                ));
            }
            for pattern in &tool.build_files {
                non_empty("contributes.build-tools.build-files", pattern)?;
            }
            if let Some(init_script) = &tool.init_script {
                check_relative("contributes.build-tools.init-script", init_script)?;
            }
        }
        check_unique(
            ContributionPoint::BuildTools,
            self.contributes.build_tools.iter().map(|t| t.id.as_str()),
        )?;

        // A build tool is a native process too — no `[wasm]` component.

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
mod tests;
