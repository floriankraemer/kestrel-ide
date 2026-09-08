//! Settings > Plugins (task P7): what the page lists, and how a typed
//! failure — a manifest the host refused, or a component the sandbox
//! stopped — becomes a sentence a user can act on.
//!
//! The same rule as `languages`, for the same reason: the page's value is
//! that it never prints a Rust error. A `LoadErrorKind` and a `WasmError`
//! each map to one sentence, one detail line and one status word, and that
//! mapping is worth a test. `ui-shell` receives the finished words.
//!
//! What the page is *for* is the second half of ADR-0026's argument for a
//! sandbox over a native tier: a plugin that traps takes nothing down, it
//! lands on a row here with the reason it stopped.

use crate::languages::sentence;
use plugin_api::{LoadErrorKind, PluginLoadError, PluginManifest, MANIFEST_FILE};
use plugin_host::{LoadedPlugin, PluginRegistry, WasmError};

/// Where a plugin came from — the page's grouping, and the first question a
/// user opens it with. `plugin-host`'s own vocabulary, re-exported rather
/// than mirrored, because the page groups by exactly the distinction the
/// host already makes.
pub use plugin_host::PluginSource;

/// The Status column. `Ok` renders as an *empty* cell, like the Languages
/// page: a column of green checks trains the eye to skip the one row that
/// has something to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginStatus {
    Ok,
    /// Turned off by the user, and therefore contributing nothing.
    Disabled,
    /// The host refused it: a manifest it could not read, an id it could
    /// not accept, a contract revision it does not speak.
    Failed,
    /// It loaded, and its component then stopped — trapped, refused to
    /// activate, or could not be instantiated at all.
    Stopped,
}

impl PluginStatus {
    /// The word shown in the Status column; empty for a healthy plugin.
    pub fn text(self) -> &'static str {
        match self {
            PluginStatus::Ok => "",
            PluginStatus::Disabled => "Disabled",
            PluginStatus::Failed => "Failed to load",
            PluginStatus::Stopped => "Stopped",
        }
    }
}

/// The details pane's three parts: what is wrong, the specific detail, and
/// where to look.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PluginProblem {
    /// One plain sentence saying what is wrong.
    pub sentence: String,
    /// The specific detail — a parser message, a trap — or empty when the
    /// sentence says everything.
    pub detail: String,
    /// The plugin's directory, so it can be selected and copied. Empty for
    /// a built-in, which has no directory to point at.
    pub path: String,
}

/// One row of the page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRow {
    pub id: String,
    /// The manifest's display name, falling back to the id for a plugin
    /// whose manifest never parsed — there is no name to show, and a blank
    /// first column would be a row about nothing.
    pub name: String,
    pub version: String,
    pub description: String,
    /// What this plugin adds, in words: the Contributes column.
    pub contributes: String,
    pub source: PluginSource,
    pub status: PluginStatus,
    /// `None` for a healthy plugin: the pane collapses rather than saying
    /// "No problems".
    pub problem: Option<PluginProblem>,
}

/// Every plugin the scan found, healthy or not, plus every one it refused.
///
/// `scanned` must come from a scan with **nothing disabled**
/// (`plugin_host::load(dir, builtins, &[])`), not from the live registry:
/// the live one has already dropped the plugins in `disabled`, and a page
/// that cannot list a disabled plugin is a page that can never switch one
/// back on. The Languages page reads its overlay directly for the same
/// reason.
///
/// `wasm_disabled` is `WasmTier::disabled()` — the plugins that loaded and
/// whose component then failed. A trap is not a load error and does not
/// appear in `scanned.errors()`; the two lists are joined here because the
/// user has one question ("is this plugin working?") and does not care
/// which half of the host answered it.
pub fn rows(
    scanned: &PluginRegistry,
    wasm_disabled: &[(String, WasmError)],
    disabled: &[String],
) -> Vec<PluginRow> {
    let mut rows: Vec<PluginRow> = scanned
        .plugins()
        .iter()
        .map(|plugin| loaded_row(plugin, wasm_disabled, disabled))
        .collect();

    for error in scanned.errors() {
        // A built-in an installed plugin shadowed is reported by the host
        // as a duplicate id, and shadowing is the supported way to replace
        // a bundled plugin — so it is not a problem to show. Any other
        // duplicate has no row yet and still gets one.
        if rows.iter().any(|row| row.id == error.id) {
            continue;
        }
        rows.push(failed_row(error, disabled));
    }
    rows
}

fn loaded_row(
    plugin: &LoadedPlugin,
    wasm_disabled: &[(String, WasmError)],
    disabled: &[String],
) -> PluginRow {
    let manifest = plugin.manifest();
    let path = plugin
        .dir()
        .map(|dir| dir.display().to_string())
        .unwrap_or_default();
    let stopped = wasm_disabled
        .iter()
        .find(|(id, _)| id == plugin.id())
        .map(|(_, error)| error);

    let (status, problem) = if disabled.contains(&manifest.id) {
        (PluginStatus::Disabled, Some(disabled_problem(&path)))
    } else if let Some(error) = stopped {
        (PluginStatus::Stopped, Some(explain_wasm(error, &path)))
    } else {
        (PluginStatus::Ok, None)
    };

    PluginRow {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone().unwrap_or_default(),
        contributes: contributes(manifest),
        source: plugin.source(),
        status,
        problem,
    }
}

fn failed_row(error: &PluginLoadError, disabled: &[String]) -> PluginRow {
    let path = error.dir.display().to_string();
    // A plugin the user turned off is not reported as broken even when it
    // also happens to be broken: the user's own choice is the truer answer,
    // and the load error would only tell them off for a plugin they already
    // switched away from.
    let (status, problem) = if disabled.contains(&error.id) {
        (PluginStatus::Disabled, disabled_problem(&path))
    } else {
        (PluginStatus::Failed, explain(error))
    };
    PluginRow {
        id: error.id.clone(),
        name: error.id.clone(),
        version: String::new(),
        description: String::new(),
        contributes: String::new(),
        source: PluginSource::Installed,
        status,
        problem: Some(problem),
    }
}

/// The Contributes column: what this plugin adds, counted rather than
/// enumerated once there is more than one of a kind.
///
/// `language-servers` has no arm here by design, predating this task: it is
/// left out rather than overlooked (`analyzers` is described below,
/// following the exact one/many shape every other point already uses).
/// `test-frameworks` (phase D) will need its own arm once
/// `TestFrameworkContribution` exists; until then an unrecognised or
/// not-yet-described point simply contributes nothing to this column, the
/// same as any point a manifest names that this function has no arm for.
fn contributes(manifest: &PluginManifest) -> String {
    let mut parts = Vec::new();
    match manifest.contributes.icon_themes.as_slice() {
        [] => {}
        [only] => parts.push(format!("Icon theme: {}", only.label)),
        many => parts.push(format!("{} icon themes", many.len())),
    }
    match manifest.contributes.commands.as_slice() {
        [] => {}
        [only] => parts.push(format!("Command: {}", only.title)),
        many => parts.push(format!("{} commands", many.len())),
    }
    match manifest.contributes.previews.as_slice() {
        [] => {}
        [only] => parts.push(format!("Preview: {}", only.label)),
        many => parts.push(format!("{} previews", many.len())),
    }
    match manifest.contributes.analyzers.as_slice() {
        [] => {}
        [only] => parts.push(format!("Analyzer: {}", only.name)),
        many => parts.push(format!("{} analyzers", many.len())),
    }
    parts.join(", ")
}

/// The details pane for a plugin the user turned off.
fn disabled_problem(path: &str) -> PluginProblem {
    PluginProblem {
        sentence: "This plugin is turned off. Nothing it contributes is loaded.".to_string(),
        detail: String::new(),
        path: path.to_string(),
    }
}

/// Turn one load failure into the details pane's three parts.
///
/// The underlying `LoadErrorKind` is never rendered on its own; it stays
/// available through its own `Display` for the log.
pub fn explain(error: &PluginLoadError) -> PluginProblem {
    let path = error.dir.display().to_string();
    let problem = |sentence: String, detail: String| PluginProblem {
        sentence,
        detail,
        path: path.clone(),
    };
    match &error.kind {
        LoadErrorKind::Unreadable { file, message } => {
            problem(format!("{file} could not be read."), sentence(message))
        }
        LoadErrorKind::MalformedManifest(message) => problem(
            format!("{MANIFEST_FILE} could not be read."),
            sentence(message),
        ),
        LoadErrorKind::UnsupportedApiVersion(version) => problem(
            format!(
                "This plugin is written for version {version} of the plugin contract, \
                 which this build of the editor does not speak. Update the editor, \
                 or install a version of the plugin built for it."
            ),
            String::new(),
        ),
        LoadErrorKind::MalformedId { field, value } => problem(
            format!("The `{field}` `{value}` is not a usable id."),
            "An id is at most 64 characters of a-z, 0-9, `.`, `_` or `-`, and starts \
             with a letter or a digit."
                .to_string(),
        ),
        LoadErrorKind::EmptyField(field) => problem(
            format!("{MANIFEST_FILE} leaves `{field}` empty."),
            String::new(),
        ),
        LoadErrorKind::UnsafePath { field, value } => problem(
            format!("`{field}` points outside the plugin's own folder."),
            format!("A plugin may only name files inside itself, but `{field}` is `{value}`."),
        ),
        LoadErrorKind::DuplicateContributionId { point, id } => problem(
            format!("Two `{point}` contributions both claim the id `{id}`."),
            String::new(),
        ),
        LoadErrorKind::CommandsWithoutComponent => problem(
            "This plugin offers commands but ships nothing to run them.".to_string(),
            "A command needs a `[wasm]` component to answer it.".to_string(),
        ),
        LoadErrorKind::UnscopedCapabilityPath(value) => problem(
            "This plugin asks to read files outside its own folder.".to_string(),
            format!("The request was `{value}`, and the editor grants no such thing."),
        ),
        LoadErrorKind::InvalidExtension(value) => problem(
            format!("The preview extension `{value}` is not a usable one."),
            "An extension is lowercase letters and digits only, with no leading dot.".to_string(),
        ),
        LoadErrorKind::DuplicateId => problem(
            format!("Another plugin already uses the id `{}`.", error.id),
            String::new(),
        ),
        LoadErrorKind::Quarantined { marker } => problem(
            "This plugin was loading when the editor last stopped, so it was disabled \
             automatically."
                .to_string(),
            format!("Delete {} to try it again.", marker.display()),
        ),
    }
}

/// Turn one sandbox failure into the same three parts.
///
/// This is the payoff ADR-0026 bought with the sandbox, so it is worth
/// wording carefully: none of these sentences blames the user, and every
/// one of them says which side stopped — the plugin's, not the editor's.
pub fn explain_wasm(error: &WasmError, path: &str) -> PluginProblem {
    let (headline, detail) = match error {
        WasmError::Unloadable(message) => (
            "This plugin's component could not be loaded.",
            message.clone(),
        ),
        WasmError::Instantiate(message) => (
            "This plugin's component could not be started. It may expect a newer \
             editor than this one.",
            message.clone(),
        ),
        WasmError::Activate(message) => ("This plugin refused to start.", message.clone()),
        WasmError::Trapped(message) => (
            "This plugin was stopped because it ran past the limits every plugin runs \
             under. The editor was not affected.",
            message.clone(),
        ),
        WasmError::Command(message) => ("A command from this plugin failed.", message.clone()),
        WasmError::UnknownCommand(id) => (
            "This plugin was asked for a command it does not offer.",
            id.clone(),
        ),
        WasmError::NoPreviewExport => (
            "This plugin offers a preview but its component does not render one.",
            "It contributes `previews` and a `[wasm]` component, but the component \
             does not implement the wider preview world."
                .to_string(),
        ),
        // Reached only through a call made against an already-disabled
        // plugin, so the cause is the row's real story and the wrapper is
        // not worth a sentence of its own.
        WasmError::Disabled(cause) => return explain_wasm(cause, path),
    };
    PluginProblem {
        sentence: headline.to_string(),
        detail: sentence(&detail),
        path: path.to_string(),
    }
}

/// The page's one selection-driven control: the bottom strip's toggle. Its
/// caption follows the selected row, because a control that says `Disable
/// Plugin` while pointing at a plugin that is already off is lying about
/// what pressing it does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PluginToggle {
    pub label: &'static str,
    /// False when nothing is selected: the strip acts on a row, so with no
    /// row it is greyed rather than hidden.
    pub enabled: bool,
    /// What to pass to "set disabled" when pressed.
    pub disable: bool,
}

/// What the toggle says and does for the selected row.
pub fn toggle(row: Option<&PluginRow>) -> PluginToggle {
    let Some(row) = row else {
        return PluginToggle {
            label: "Disable Plugin",
            enabled: false,
            disable: true,
        };
    };
    let off = row.status == PluginStatus::Disabled;
    PluginToggle {
        label: if off {
            "Enable Plugin"
        } else {
            "Disable Plugin"
        },
        enabled: true,
        disable: !off,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// A manifest good enough to load, parameterised only by what the tests
    /// actually vary.
    fn manifest(id: &str) -> String {
        format!(
            "id = \"{id}\"\n\
             name = \"Plugin {id}\"\n\
             version = \"1.0.0\"\n\
             api_version = 1\n\
             \n\
             [[contributes.icon-themes]]\n\
             id = \"{id}-theme\"\n\
             label = \"Theme {id}\"\n\
             pack = \"pack.toml\"\n"
        )
    }

    fn install(root: &Path, id: &str, text: &str) {
        let dir = root.join("plugins").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("plugin.toml"), text).unwrap();
    }

    /// The static a built-in needs. Leaked because `BuiltinPlugin` holds
    /// `&'static str`, and a test binary that leaks one manifest is a test
    /// binary that leaks one manifest.
    fn builtin(id: &str) -> plugin_host::BuiltinPlugin {
        plugin_host::BuiltinPlugin {
            manifest: Box::leak(manifest(id).into_boxed_str()),
            files: &[],
        }
    }

    fn scan(root: &Path, builtins: &[plugin_host::BuiltinPlugin]) -> PluginRegistry {
        // Nothing disabled, deliberately: the page needs the plugins the
        // live registry has already dropped.
        plugin_host::load(root, builtins, &[])
    }

    fn row<'a>(rows: &'a [PluginRow], id: &str) -> &'a PluginRow {
        rows.iter()
            .find(|row| row.id == id)
            .unwrap_or_else(|| panic!("no row for `{id}` in {rows:?}"))
    }

    #[test]
    fn the_page_merges_built_in_installed_failed_and_disabled_plugins() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "good", &manifest("good"));
        install(dir.path(), "off", &manifest("off"));
        install(dir.path(), "broken", "this is not toml");

        let scanned = scan(dir.path(), &[builtin("bundled")]);
        let rows = rows(&scanned, &[], &["off".to_string()]);

        assert_eq!(rows.len(), 4, "{rows:?}");
        assert_eq!(row(&rows, "good").status, PluginStatus::Ok);
        assert_eq!(row(&rows, "good").source, PluginSource::Installed);
        assert!(row(&rows, "good").problem.is_none());

        assert_eq!(row(&rows, "bundled").source, PluginSource::Builtin);
        assert_eq!(row(&rows, "bundled").status, PluginStatus::Ok);

        assert_eq!(row(&rows, "off").status, PluginStatus::Disabled);
        assert!(row(&rows, "off")
            .problem
            .as_ref()
            .unwrap()
            .sentence
            .contains("turned off"));

        let broken = row(&rows, "broken");
        assert_eq!(broken.status, PluginStatus::Failed);
        // The name column still says something for a plugin whose manifest
        // never parsed.
        assert_eq!(broken.name, "broken");
        assert!(!broken.problem.as_ref().unwrap().sentence.is_empty());
    }

    #[test]
    fn an_installed_plugin_shadowing_a_built_in_leaves_one_row_and_no_error() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "material-icons", &manifest("material-icons"));

        let scanned = scan(dir.path(), &[builtin("material-icons")]);
        // The host does record the shadowed built-in as a duplicate...
        assert_eq!(scanned.errors().len(), 1);

        // ...but replacing a bundled plugin is the supported way to fix one,
        // so the page shows the installed copy and nothing else.
        let rows = rows(&scanned, &[], &[]);
        assert_eq!(rows.len(), 1, "{rows:?}");
        assert_eq!(rows[0].source, PluginSource::Installed);
        assert_eq!(rows[0].status, PluginStatus::Ok);
    }

    #[test]
    fn a_plugin_that_trapped_in_the_sandbox_shows_why_and_stays_listed() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "runaway", &manifest("runaway"));

        let scanned = scan(dir.path(), &[]);
        let stopped = vec![(
            "runaway".to_string(),
            WasmError::Trapped("all fuel consumed by WebAssembly".to_string()),
        )];
        let rows = rows(&scanned, &stopped, &[]);

        let row = row(&rows, "runaway");
        assert_eq!(row.status, PluginStatus::Stopped);
        let problem = row.problem.as_ref().unwrap();
        // The point of the sandbox, said in words on the page.
        assert!(problem.sentence.contains("The editor was not affected"));
        assert_eq!(problem.detail, "all fuel consumed by WebAssembly.");
    }

    #[test]
    fn a_disabled_plugin_is_reported_as_the_users_choice_not_as_a_failure() {
        // A broken plugin the user switched off must not keep shouting: it
        // is off, which is a truer answer than "it is broken".
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "broken", "id = 1");

        let scanned = scan(dir.path(), &[]);
        let rows = rows(&scanned, &[], &["broken".to_string()]);
        assert_eq!(rows[0].status, PluginStatus::Disabled);
    }

    #[test]
    fn the_toggle_follows_the_selected_row() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path(), "good", &manifest("good"));
        let scanned = scan(dir.path(), &[]);

        let on = rows(&scanned, &[], &[]);
        assert_eq!(
            toggle(Some(&on[0])),
            PluginToggle {
                label: "Disable Plugin",
                enabled: true,
                disable: true,
            }
        );

        let off = rows(&scanned, &[], &["good".to_string()]);
        assert_eq!(
            toggle(Some(&off[0])),
            PluginToggle {
                label: "Enable Plugin",
                enabled: true,
                disable: false,
            }
        );

        // No selection: the control stays put and is greyed.
        assert!(!toggle(None).enabled);
    }

    #[test]
    fn the_contributes_column_names_one_of_a_kind_and_counts_the_rest() {
        let one = PluginManifest::from_toml_str(&manifest("one")).unwrap();
        assert_eq!(contributes(&one), "Icon theme: Theme one");

        let none = PluginManifest::from_toml_str(
            "id = \"bare\"\nname = \"Bare\"\nversion = \"1\"\napi_version = 1\n",
        )
        .unwrap();
        assert_eq!(contributes(&none), "");

        let many = PluginManifest::from_toml_str(
            "id = \"many\"\nname = \"Many\"\nversion = \"1\"\napi_version = 1\n\
             \n[wasm]\ncomponent = \"plugin.wasm\"\n\
             \n[[contributes.commands]]\nid = \"a\"\ntitle = \"Do A\"\n\
             \n[[contributes.commands]]\nid = \"b\"\ntitle = \"Do B\"\n",
        )
        .unwrap();
        assert_eq!(contributes(&many), "2 commands");

        let preview = PluginManifest::from_toml_str(
            "id = \"markdown-preview\"\nname = \"Markdown Preview\"\nversion = \"1\"\napi_version = 1\n\
             \n[[contributes.previews]]\nid = \"markdown\"\nlabel = \"Markdown\"\nextensions = [\"md\"]\n",
        )
        .unwrap();
        assert_eq!(contributes(&preview), "Preview: Markdown");

        let one_analyzer = PluginManifest::from_toml_str(
            "id = \"php-tools\"\nname = \"PHP Tools\"\nversion = \"1\"\napi_version = 1\n\
             \n[[contributes.analyzers]]\nid = \"phpstan\"\nname = \"PHPStan\"\n\
             program-candidates = [\"phpstan\"]\noutput-format = \"checkstyle-xml\"\n",
        )
        .unwrap();
        assert_eq!(contributes(&one_analyzer), "Analyzer: PHPStan");

        let two_analyzers = PluginManifest::from_toml_str(
            "id = \"php-tools\"\nname = \"PHP Tools\"\nversion = \"1\"\napi_version = 1\n\
             \n[[contributes.analyzers]]\nid = \"phpstan\"\nname = \"PHPStan\"\n\
             program-candidates = [\"phpstan\"]\noutput-format = \"checkstyle-xml\"\n\
             \n[[contributes.analyzers]]\nid = \"phpcs\"\nname = \"PHP_CodeSniffer\"\n\
             program-candidates = [\"phpcs\"]\noutput-format = \"checkstyle-xml\"\n",
        )
        .unwrap();
        assert_eq!(contributes(&two_analyzers), "2 analyzers");
    }

    #[test]
    fn an_unrecognised_contribution_point_breaks_nothing() {
        // A point this build has never heard of loads via `Contributes`'s
        // `unknown` catch-all (ADR-0033's precedent) and this column
        // simply says nothing about it, the same as any point with no arm
        // here yet.
        let manifest = PluginManifest::from_toml_str(
            "id = \"php-tools\"\nname = \"PHP Tools\"\nversion = \"1\"\napi_version = 1\n\
             \n[[contributes.some-future-point]]\nid = \"whatever\"\n",
        )
        .expect("an unrecognised point must not fail the whole manifest");
        assert_eq!(contributes(&manifest), "");
    }

    #[test]
    fn every_load_failure_gets_a_sentence_that_is_not_the_rust_error() {
        let kinds = [
            LoadErrorKind::Unreadable {
                file: "plugin.toml".into(),
                message: "permission denied".into(),
            },
            LoadErrorKind::MalformedManifest("expected a table".into()),
            LoadErrorKind::UnsupportedApiVersion(9),
            LoadErrorKind::MalformedId {
                field: "id",
                value: "Nope!".into(),
            },
            LoadErrorKind::EmptyField("name"),
            LoadErrorKind::UnsafePath {
                field: "pack",
                value: "../../etc".into(),
            },
            LoadErrorKind::DuplicateContributionId {
                point: "icon-themes",
                id: "material".into(),
            },
            LoadErrorKind::CommandsWithoutComponent,
            LoadErrorKind::UnscopedCapabilityPath("/etc".into()),
            LoadErrorKind::DuplicateId,
            LoadErrorKind::Quarantined {
                marker: PathBuf::from("/tmp/.quarantine/x"),
            },
        ];
        for kind in kinds {
            let raw = kind.to_string();
            let problem = explain(&PluginLoadError {
                id: "x".to_string(),
                dir: PathBuf::from("/plugins/x"),
                kind,
            });
            assert!(!problem.sentence.is_empty());
            assert_ne!(problem.sentence, raw);
            assert_eq!(problem.path, "/plugins/x");
        }
    }

    #[test]
    fn a_disabled_wasm_error_reports_the_cause_it_carries() {
        let wrapped = WasmError::Disabled(Box::new(WasmError::Activate("no api key".into())));
        assert_eq!(
            explain_wasm(&wrapped, "/plugins/x"),
            explain_wasm(&WasmError::Activate("no api key".into()), "/plugins/x")
        );
    }
}
