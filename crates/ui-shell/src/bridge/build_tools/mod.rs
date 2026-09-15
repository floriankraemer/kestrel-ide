//! `BuildToolsService` (the jvm-build-tools plan's B1): detects Gradle and
//! Maven, runs a sync on a worker thread through `jvm_build_core::{gradle,
//! maven}::sync`, and turns the result into the dock's tree
//! (`jvm_build_core::view::rows`) and the reload-banner/status-bar state.
//!
//! `BuildToolsEditor` (B5's settings page) lives in this module too: the
//! draft over `settings_model::build_tools::BuildToolsDraft`, following
//! `AnalysisEditor`'s begin_edit/rows/set_*/is_dirty/commit shape — except
//! it edits the **global** file only. `trusted_roots` is deliberately
//! global-only (ADR-0057 §3), and the project-scoped override plumbing
//! `settings_model::scope::resolve` would need for the Gradle/Maven
//! sub-tables (à la `[containers]`) was never built in phase A — extending
//! `scope::resolve` to overlay only *part* of a section (never
//! `trusted_roots`) is a real follow-up, not attempted here.
//!
//! # Threading
//!
//! One registered `#[qobject]` owning at most one in-flight sync, mirroring
//! `AnalysisServiceRust`'s shape (`crate::bridge::analysis`): a sync spawns
//! its own `std::thread`, and reports back through `CxxQtThread::queue`.
//!
//! Translation only, per `docs/architecture/layering.md`: which tool is
//! contributed, what its build files are, and how to run a sync are
//! `jvm-build-core`'s and `plugin-api`'s; this adapter only detects,
//! resolves settings, and moves bytes between them. Errors cross the seam
//! as `FfiResult`'s typed code + message (ADR-0003).

use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use cxx_qt::Threading;
use cxx_qt_lib::QString;

use jvm_build_core::model::{BuildModel, Tool};
use jvm_build_core::sync::ChangeOrigin;

use crate::bridge::errors;
use crate::bridge::ffi;
use crate::bridge::registry::SharedDiagnostics;

/// This source's key in the shared store (ADR-0046): a sync failure's rows
/// never clobber (or get clobbered by) a build's, an analyzer's or a
/// language server's.
const SOURCE_KEY: &str = "build-tools:sync";

fn current_project_root() -> Option<PathBuf> {
    crate::bridge::convert::current_project_root()
}

/// Every `build-tools` contribution any loaded plugin offers, with the
/// plugin that owns it — Gradle's init-script asset lives under that
/// plugin's own materialised directory (`LoadedPlugin::asset_dir`, A3).
/// Gathered fresh on every call, the same freshness
/// `bridge::testing::contributed_frameworks` gives test frameworks.
fn contributed_build_tools() -> Vec<(plugin_host::LoadedPlugin, plugin_api::BuildToolContribution)>
{
    plugin_host::registry()
        .build_tools()
        .map(|(plugin, contribution)| (plugin.clone(), contribution.clone()))
        .collect()
}

/// Which contributed tools are actually present in `root` — detection stays
/// marker-only (ADR-0057 §4): `run_core::detect_toolchains`'s marker
/// intersection with what the registry contributes, so a contribution whose
/// `toolchain` id this build does not recognise (`ToolchainId::from_id`
/// fails) is silently dropped rather than crashing detection.
fn detected_build_tools(
    root: &Path,
) -> Vec<(plugin_host::LoadedPlugin, plugin_api::BuildToolContribution)> {
    let detected: Vec<String> = run_core::toolchain::detect_toolchains(root)
        .into_iter()
        .map(|id| id.as_str().to_string())
        .collect();
    contributed_build_tools()
        .into_iter()
        .filter(|(_, contribution)| detected.contains(&contribution.toolchain))
        .collect()
}

fn tool_for(contribution: &plugin_api::BuildToolContribution) -> Option<Tool> {
    match contribution.toolchain.as_str() {
        "gradle" => Some(Tool::Gradle),
        "maven" => Some(Tool::Maven),
        _ => None,
    }
}

/// The auto-reload mode in force for whichever tool(s) are detected: the
/// first detected tool's own setting, Gradle before Maven — a project
/// rarely runs both, and when it does the two share one banner/auto-sync
/// decision rather than two independent ones.
fn auto_reload_mode(
    settings: &app_config::Settings,
    detected: &[(plugin_host::LoadedPlugin, plugin_api::BuildToolContribution)],
) -> settings_model::build_tools::AutoReload {
    let has_gradle = detected.iter().any(|(_, c)| c.toolchain == "gradle");
    let id = if has_gradle {
        settings.build_tools.gradle.auto_reload.as_deref()
    } else {
        settings.build_tools.maven.auto_reload.as_deref()
    };
    id.and_then(settings_model::build_tools::AutoReload::from_id)
        .unwrap_or(settings_model::build_tools::AutoReload::DEFAULT)
}

/// Internal banner state — kept as a plain Rust enum rather than the FFI
/// one so the FFI mapping stays a single `match` at the seam, the same
/// split `test_core::TestStatus` -> `FfiTestStatusKind` already draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum BannerState {
    #[default]
    None,
    TrustGradle,
    TrustMaven,
    ReloadNeeded,
}

/// Rust side of the `BuildToolsService` QObject.
#[derive(Default)]
pub struct BuildToolsServiceRust {
    models: RefCell<Vec<BuildModel>>,
    syncing: Cell<bool>,
    sync_error: RefCell<Option<String>>,
    /// `ponytail:` a cooperative flag, not a process kill — neither
    /// `gradle::sync` nor `maven::sync` exposes a cancel handle into the
    /// blocking `process_exec::run` they call (only a wall-clock timeout).
    /// `cancelSync` stops this adapter from waiting on the result, but the
    /// underlying `gradle`/`mvn` process keeps running until it exits or
    /// times out on its own. Upgrade path: thread a `process_exec::spawn`-
    /// based kill handle through `gradle::sync`/`maven::sync` when a real
    /// cancel matters (a `BuildHandle`-shaped follow-up, mirroring
    /// `build_core::BuildHandle`).
    cancel: RefCell<Option<Arc<AtomicBool>>>,
    offline: Cell<bool>,
    skip_tests: Cell<bool>,
    banner: RefCell<BannerState>,
    save_tracker: RefCell<jvm_build_core::sync::SaveTracker>,
    /// Maven only (B3): which of the synced model's declared profiles the
    /// dock's checkboxes have ticked — fed into every task/goal run's
    /// `-P<id>` flags (`run::RunOptions::profiles`). Kept here rather than
    /// in `jvm_build_core::view`, which shapes rows from a model and knows
    /// nothing about UI-held check state.
    checked_profiles: RefCell<std::collections::HashSet<String>>,
    store: SharedDiagnostics,
}

fn to_ffi_node_kind(kind: jvm_build_core::view::NodeKind) -> ffi::FfiBuildToolNodeKind {
    use jvm_build_core::view::NodeKind;
    match kind {
        NodeKind::ToolRoot => ffi::FfiBuildToolNodeKind::ToolRoot,
        NodeKind::Group => ffi::FfiBuildToolNodeKind::Group,
        NodeKind::Task => ffi::FfiBuildToolNodeKind::Task,
        NodeKind::Module => ffi::FfiBuildToolNodeKind::Module,
        NodeKind::SourceRoot => ffi::FfiBuildToolNodeKind::SourceRoot,
        NodeKind::Dependency => ffi::FfiBuildToolNodeKind::Dependency,
        NodeKind::Profile => ffi::FfiBuildToolNodeKind::Profile,
        NodeKind::Plugin => ffi::FfiBuildToolNodeKind::Plugin,
        NodeKind::Goal => ffi::FfiBuildToolNodeKind::Goal,
    }
}

fn to_ffi_node(node: &jvm_build_core::view::Node, tool: Tool) -> ffi::FfiBuildToolNode {
    ffi::FfiBuildToolNode {
        id: QString::from(node.id.as_str()),
        parent_id: QString::from(node.parent_id.as_str()),
        kind: to_ffi_node_kind(node.kind),
        label: QString::from(node.label.as_str()),
        detail: QString::from(node.detail.as_str()),
        tool: QString::from(tool.toolchain_id()),
        build_file: QString::from(node.build_file.as_str()),
        checked: node.checked,
    }
}

/// A sync failure's one-line message, for the sync-state getter and the
/// Build dock's/diagnostics store's row.
fn gradle_error_message(err: &jvm_build_core::gradle::sync::SyncError) -> String {
    use jvm_build_core::gradle::sync::SyncError;
    match err {
        SyncError::NotFound => "Gradle was not found (no wrapper, no gradle on PATH)".to_string(),
        SyncError::TimedOut => "Gradle sync timed out".to_string(),
        SyncError::Io(message) => format!("Gradle sync failed: {message}"),
        SyncError::BuildFailed { stderr } => stderr.clone(),
        SyncError::ModelUnreadable(message) => format!("Gradle sync produced no model: {message}"),
        SyncError::ModelInvalid(message) => {
            format!("Gradle sync produced an unreadable model: {message}")
        }
    }
}

fn maven_error_message(err: &jvm_build_core::maven::sync::SyncError) -> String {
    use jvm_build_core::maven::sync::SyncError;
    match err {
        SyncError::NotFound => "Maven was not found (no wrapper, no mvn on PATH)".to_string(),
        SyncError::TimedOut => "Maven sync timed out".to_string(),
        SyncError::Io(message) => format!("Maven sync failed: {message}"),
        SyncError::BuildFailed { stderr } => stderr.clone(),
        SyncError::EffectivePomUnreadable(message) => {
            format!("Maven sync produced no effective POM: {message}")
        }
        SyncError::EffectivePomInvalid(message) => {
            format!("Maven sync produced an unreadable effective POM: {message}")
        }
        SyncError::RootPomInvalid(message) => {
            format!("this project's pom.xml is invalid: {message}")
        }
    }
}

/// Publish a sync failure into the shared diagnostics store, resolving any
/// `file:line` the tool's own message carries through `build-core`'s text
/// parser (the plan's own reuse note) — a Gradle/Maven script error is
/// exactly the shape that parser already reads for a plain build.
fn publish_sync_error(store: &SharedDiagnostics, root: &Path, message: &str) {
    let mut store = store.borrow_mut();
    store.clear_source(SOURCE_KEY);
    let diagnostic = build_core::text::parse_line(message, root, None);
    let Some(diagnostic) = diagnostic else {
        return;
    };
    let uri = diagnostics_core::uri_from_path(&diagnostic.path.display().to_string());
    let core_diagnostic = diagnostics_core::Diagnostic {
        range: diagnostics_core::Range {
            start: diagnostics_core::Position {
                line: diagnostic.line.saturating_sub(1),
                character: diagnostic.column.saturating_sub(1),
            },
            end: None,
        },
        severity: diagnostic.severity,
        message: diagnostic.message.clone(),
        source: SOURCE_KEY.to_string(),
        raw: None,
    };
    store.replace(SOURCE_KEY, &uri, vec![core_diagnostic]);
}

impl ffi::BuildToolsService {
    pub fn rows(&self) -> Vec<ffi::FfiBuildToolNode> {
        let checked = self.checked_profiles.borrow();
        self.models
            .borrow()
            .iter()
            .flat_map(|model| {
                jvm_build_core::view::rows(model, &checked)
                    .into_iter()
                    .map(|node| to_ffi_node(&node, model.tool))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// A profile checkbox in the dock was toggled (B3). `modelChanged` is
    /// reused to signal it rather than a new signal: the tree rebuilds the
    /// same way it does for any other row change, and nothing about the
    /// synced model itself changed.
    pub fn set_profile_checked(mut self: Pin<&mut Self>, profile: &QString, checked: bool) {
        let profile = profile.to_string();
        if checked {
            self.checked_profiles.borrow_mut().insert(profile);
        } else {
            self.checked_profiles.borrow_mut().remove(&profile);
        }
        self.as_mut().model_changed();
    }

    pub fn title_kind(&self) -> ffi::FfiBuildToolTitleKind {
        let has_gradle = self.models.borrow().iter().any(|m| m.tool == Tool::Gradle);
        let has_maven = self.models.borrow().iter().any(|m| m.tool == Tool::Maven);
        match (has_gradle, has_maven) {
            (true, true) => ffi::FfiBuildToolTitleKind::Both,
            (true, false) => ffi::FfiBuildToolTitleKind::Gradle,
            (false, true) => ffi::FfiBuildToolTitleKind::Maven,
            (false, false) => ffi::FfiBuildToolTitleKind::None,
        }
    }

    pub fn sync_state_kind(&self) -> ffi::FfiSyncStateKind {
        if self.syncing.get() {
            ffi::FfiSyncStateKind::Syncing
        } else if self.sync_error.borrow().is_some() {
            ffi::FfiSyncStateKind::Failed
        } else {
            ffi::FfiSyncStateKind::Idle
        }
    }

    pub fn sync_message(&self) -> QString {
        QString::from(
            self.sync_error
                .borrow()
                .clone()
                .unwrap_or_default()
                .as_str(),
        )
    }

    pub fn banner_kind(&self) -> ffi::FfiBannerKind {
        match *self.banner.borrow() {
            BannerState::None => ffi::FfiBannerKind::None,
            BannerState::TrustGradle => ffi::FfiBannerKind::TrustGradle,
            BannerState::TrustMaven => ffi::FfiBannerKind::TrustMaven,
            BannerState::ReloadNeeded => ffi::FfiBannerKind::ReloadNeeded,
        }
    }

    pub fn dismiss_banner(mut self: Pin<&mut Self>) {
        *self.banner.borrow_mut() = BannerState::None;
        self.as_mut().banner_changed();
    }

    pub fn set_offline(&self, offline: bool) {
        self.offline.set(offline);
    }

    pub fn set_skip_tests(&self, skip_tests: bool) {
        self.skip_tests.set(skip_tests);
    }

    /// A project just opened (or reopened): re-detect and either show the
    /// trust banner or sync automatically for an already-trusted root.
    pub fn project_opened(mut self: Pin<&mut Self>, root: &QString) {
        let root = PathBuf::from(root.to_string());
        self.models.borrow_mut().clear();
        crate::bridge::registry::shared_build_models()
            .borrow_mut()
            .clear();
        *self.sync_error.borrow_mut() = None;
        self.as_mut().model_changed();
        self.as_mut().refresh_banner(&root);
    }

    fn refresh_banner(mut self: Pin<&mut Self>, root: &Path) {
        let detected = detected_build_tools(root);
        if detected.is_empty() {
            *self.banner.borrow_mut() = BannerState::None;
            self.as_mut().banner_changed();
            return;
        }
        let settings = crate::bridge::convert::load_resolved_settings();
        if jvm_build_core::sync::is_trusted(root, &settings.build_tools.trusted_roots) {
            *self.banner.borrow_mut() = BannerState::None;
            self.as_mut().banner_changed();
            self.as_mut().sync();
            return;
        }
        *self.banner.borrow_mut() = if detected.iter().any(|(_, c)| c.toolchain == "gradle") {
            BannerState::TrustGradle
        } else {
            BannerState::TrustMaven
        };
        self.as_mut().banner_changed();
    }

    /// "Load" on the trust banner: append the canonical root to the
    /// **global** settings' trusted roots (ADR-0057 §3 — never the
    /// project's own file), then sync.
    pub fn trust_and_load(mut self: Pin<&mut Self>) -> ffi::FfiResult {
        let Some(root) = current_project_root() else {
            return errors::failure(errors::CODE_NO_PROJECT, "no project is open");
        };
        let Ok(canonical) = root.canonicalize() else {
            return errors::failure(
                errors::CODE_REFUSED,
                "this project's root could not be resolved",
            );
        };
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return errors::failure(errors::CODE_REFUSED, "could not read global settings");
        };
        if !settings.build_tools.trusted_roots.contains(&canonical) {
            settings.build_tools.trusted_roots.push(canonical);
        }
        if app_config::save(&config_dir, &settings).is_err() {
            return errors::failure(errors::CODE_REFUSED, "could not save global settings");
        }
        *self.banner.borrow_mut() = BannerState::None;
        self.as_mut().banner_changed();
        self.as_mut().sync()
    }

    /// A watched build file changed on disk — `main_window.cpp`'s
    /// `watchedFileChanged` relay, via `build_tools_wiring.cpp`.
    pub fn file_changed(mut self: Pin<&mut Self>, path: &QString) {
        self.as_mut().handle_change(path, ChangeOrigin::Watcher);
    }

    /// The user saved a build file open in a tab — `editor_tabs.cpp`'s
    /// `documentSaved` relay.
    pub fn file_saved(mut self: Pin<&mut Self>, path: &QString) {
        let path_buf = PathBuf::from(path.to_string());
        self.save_tracker.borrow_mut().record_save(&path_buf);
        self.as_mut().handle_change(path, ChangeOrigin::Save);
    }

    fn handle_change(mut self: Pin<&mut Self>, path: &QString, origin: ChangeOrigin) {
        let Some(root) = current_project_root() else {
            return;
        };
        let path_buf = PathBuf::from(path.to_string());
        let Ok(relative) = path_buf.strip_prefix(&root) else {
            return;
        };
        let detected = detected_build_tools(&root);
        if detected.is_empty() {
            return;
        }
        let patterns: Vec<String> = detected
            .iter()
            .flat_map(|(_, c)| c.build_files.iter().cloned())
            .collect();
        if !jvm_build_core::sync::is_build_file(relative, &patterns) {
            return;
        }
        if origin == ChangeOrigin::Watcher
            && self.save_tracker.borrow().is_echo_of_recent_save(&path_buf)
        {
            return;
        }
        let settings = crate::bridge::convert::load_resolved_settings();
        if !jvm_build_core::sync::is_trusted(&root, &settings.build_tools.trusted_roots) {
            return;
        }
        // Two crates each carry their own `AutoReload` (`sync`'s reload
        // policy is Qt-free and settings-model-free by design, per
        // `docs/architecture/layering.md`'s `jvm-build-core` row), so the
        // settings-model value crosses through its own id string, the same
        // join `settings_model::build_tools::AutoReload::from_id` already
        // does the other way for a persisted string.
        let mode =
            jvm_build_core::sync::AutoReload::from_id(auto_reload_mode(&settings, &detected).id())
                .unwrap_or(jvm_build_core::sync::AutoReload::DEFAULT);
        match jvm_build_core::sync::decide(mode, origin) {
            jvm_build_core::sync::ReloadAction::Auto => {
                self.as_mut().sync();
            }
            jvm_build_core::sync::ReloadAction::Banner => {
                *self.banner.borrow_mut() = BannerState::ReloadNeeded;
                self.as_mut().banner_changed();
            }
        }
    }

    pub fn sync(mut self: Pin<&mut Self>) -> ffi::FfiResult {
        if self.syncing.get() {
            return errors::failure(errors::CODE_REFUSED, "a sync is already running");
        }
        let Some(root) = current_project_root() else {
            return errors::failure(errors::CODE_NO_PROJECT, "no project is open");
        };
        let detected = detected_build_tools(&root);
        if detected.is_empty() {
            return errors::failure(errors::CODE_REFUSED, "no Gradle or Maven project is open");
        }
        let settings = crate::bridge::convert::load_resolved_settings();
        if !jvm_build_core::sync::is_trusted(&root, &settings.build_tools.trusted_roots) {
            return errors::failure(
                errors::CODE_REFUSED,
                "this project has not been trusted yet",
            );
        }

        *self.sync_error.borrow_mut() = None;
        let cancel = Arc::new(AtomicBool::new(false));
        *self.cancel.borrow_mut() = Some(Arc::clone(&cancel));
        self.syncing.set(true);
        self.as_mut().sync_state_changed();

        let config_dir = app_core::resolve_config_dir();
        let offline = self.offline.get();
        let gradle_offline = offline || settings.build_tools.gradle.offline.unwrap_or(false);
        let maven_offline = offline || settings.build_tools.maven.offline.unwrap_or(false);
        let qt_thread = self.as_mut().qt_thread();
        let root_for_thread = root.clone();

        std::thread::spawn(move || {
            let mut models = Vec::new();
            let mut error: Option<String> = None;
            for (plugin, contribution) in detected {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                match tool_for(&contribution) {
                    Some(Tool::Gradle) => {
                        let script = contribution.init_script.as_ref().and_then(|relative| {
                            plugin
                                .asset_dir(&config_dir)
                                .ok()
                                .map(|dir| dir.join(relative))
                        });
                        let Some(script) = script else {
                            error = Some(format!("{}: no init script asset", contribution.name));
                            continue;
                        };
                        let opts = jvm_build_core::gradle::sync::SyncOptions {
                            offline: gradle_offline,
                            timeout: None,
                        };
                        match jvm_build_core::gradle::sync::sync(&root_for_thread, &script, &opts) {
                            Ok(model) => models.push(model),
                            Err(err) => error = Some(gradle_error_message(&err)),
                        }
                    }
                    Some(Tool::Maven) => {
                        let opts = jvm_build_core::maven::sync::SyncOptions {
                            offline: maven_offline,
                            timeout: None,
                        };
                        match jvm_build_core::maven::sync::sync(&root_for_thread, &opts) {
                            Ok(model) => models.push(model),
                            Err(err) => error = Some(maven_error_message(&err)),
                        }
                    }
                    None => {}
                }
            }
            let cancelled = cancel.load(Ordering::Relaxed);
            let root_for_queue = root_for_thread.clone();
            let _ = qt_thread.queue(move |mut service: Pin<&mut ffi::BuildToolsService>| {
                service.syncing.set(false);
                *service.cancel.borrow_mut() = None;
                if cancelled {
                    service.as_mut().sync_state_changed();
                    return;
                }
                *service.models.borrow_mut() = models.clone();
                *crate::bridge::registry::shared_build_models().borrow_mut() = models;
                if let Some(message) = &error {
                    publish_sync_error(&service.store, &root_for_queue, message);
                } else {
                    service.store.borrow_mut().clear_source(SOURCE_KEY);
                }
                *service.sync_error.borrow_mut() = error;
                service.as_mut().model_changed();
                service.as_mut().sync_state_changed();
            });
        });
        ffi::FfiResult::default()
    }

    pub fn cancel_sync(&self) {
        if let Some(flag) = self.cancel.borrow().as_ref() {
            flag.store(true, Ordering::Relaxed);
        }
    }

    fn find_task<'a>(
        &self,
        models: &'a [BuildModel],
        node_id: &str,
    ) -> Option<(&'a BuildModel, jvm_build_core::model::Task)> {
        let no_checked = std::collections::HashSet::new();
        for model in models {
            for node in jvm_build_core::view::rows(model, &no_checked) {
                if node.id == node_id && !node.task_path.is_empty() {
                    let task = model
                        .tasks
                        .iter()
                        .find(|t| t.path == node.task_path)
                        .cloned()
                        .unwrap_or(jvm_build_core::model::Task {
                            path: node.task_path.clone(),
                            name: node.label.clone(),
                            group: String::new(),
                            description: String::new(),
                        });
                    return Some((model, task));
                }
            }
        }
        None
    }

    /// The run configuration a double-click on task/goal row `node_id`
    /// launches (B3).
    pub fn task_config(&self, node_id: &QString) -> ffi::FfiRunConfig {
        self.task_config_for(node_id, "")
    }

    /// Same, with the "Execute…" line edit's text split into extra argv
    /// (B3) — the splitting rule is `jvm_build_core::run::split_args`,
    /// tested there, never re-implemented in `cpp/`.
    pub fn task_config_with_args(
        &self,
        node_id: &QString,
        extra_args: &QString,
    ) -> ffi::FfiRunConfig {
        self.task_config_for(node_id, &extra_args.to_string())
    }

    fn task_config_for(&self, node_id: &QString, extra_args_text: &str) -> ffi::FfiRunConfig {
        let models = self.models.borrow();
        let Some((model, task)) = self.find_task(&models, &node_id.to_string()) else {
            return ffi::FfiRunConfig::default();
        };
        let mut profiles: Vec<String> = self.checked_profiles.borrow().iter().cloned().collect();
        profiles.sort_unstable();
        let opts = jvm_build_core::run::RunOptions {
            offline: self.offline.get(),
            skip_tests: self.skip_tests.get(),
            profiles,
            extra_args: jvm_build_core::run::split_args(extra_args_text),
        };
        let config = jvm_build_core::run::task_config(model, &task, &opts);
        crate::bridge::run::to_ffi_run_config(&config)
    }
}

/// Rust side of `BuildToolsEditor` (B5). Global-only — see this module's
/// doc comment.
#[derive(Default)]
pub struct BuildToolsEditorRust {
    draft: RefCell<Option<settings_model::build_tools::BuildToolsDraft>>,
    saved: RefCell<Option<settings_model::build_tools::BuildToolsDraft>>,
    /// The layer this draft came from and will be written back to
    /// (ADR-0022) — `AnalysisEditor::begin_edit`'s own shape.
    scope: RefCell<settings_model::Scope>,
}

impl ffi::BuildToolsEditor {
    /// Load the draft from `scope` — `"global"` or `"project"`. Project
    /// scope shows only the project's own override (defaulted where it
    /// says nothing), never the resolved/effective value — the same shape
    /// `AnalysisEditor::begin_edit`'s Project branch uses, and never
    /// `trusted_roots`, which has no project-scope existence at all
    /// (ADR-0057 §3).
    pub fn begin_edit(&self, scope: &QString) {
        let scope = crate::bridge::settings::scope_from_name(&scope.to_string());
        *self.scope.borrow_mut() = scope;
        let settings = match scope {
            settings_model::Scope::Project => {
                let project_override = crate::bridge::convert::load_project_settings().build_tools;
                app_config::Settings {
                    build_tools: app_config::BuildToolsSettings {
                        trusted_roots: Vec::new(),
                        gradle: project_override.clone().unwrap_or_default().gradle,
                        maven: project_override.unwrap_or_default().maven,
                    },
                    ..app_config::Settings::default()
                }
            }
            _ => app_config::load(&app_core::resolve_config_dir()).unwrap_or_default(),
        };
        let draft = settings_model::build_tools::BuildToolsDraft::new(&settings);
        *self.saved.borrow_mut() = Some(draft.clone());
        *self.draft.borrow_mut() = Some(draft);
    }

    pub fn fields(&self) -> ffi::FfiBuildToolsFields {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return ffi::FfiBuildToolsFields::default();
        };
        to_ffi_fields(draft)
    }

    pub fn problems(&self) -> Vec<ffi::FfiBuildToolsProblem> {
        let draft = self.draft.borrow();
        let Some(draft) = draft.as_ref() else {
            return Vec::new();
        };
        draft
            .validate()
            .into_iter()
            .map(|problem| ffi::FfiBuildToolsProblem {
                field: to_ffi_field(problem.field),
                sentence: QString::from(problem.sentence.as_str()),
            })
            .collect()
    }

    pub fn set_gradle_distribution(&self, id: &QString) {
        if let Some(dist) =
            settings_model::build_tools::GradleDistribution::from_id(&id.to_string())
        {
            if let Some(draft) = self.draft.borrow_mut().as_mut() {
                draft.gradle_distribution = dist;
            }
        }
    }

    pub fn set_gradle_home(&self, value: &QString) {
        set_path_field(&self.draft, value, |draft| &mut draft.gradle_home);
    }

    pub fn set_gradle_java_home(&self, value: &QString) {
        set_path_field(&self.draft, value, |draft| &mut draft.gradle_java_home);
    }

    pub fn set_gradle_offline(&self, value: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.gradle_offline = value;
        }
    }

    pub fn set_gradle_auto_reload(&self, id: &QString) {
        if let Some(mode) = settings_model::build_tools::AutoReload::from_id(&id.to_string()) {
            if let Some(draft) = self.draft.borrow_mut().as_mut() {
                draft.gradle_auto_reload = mode;
            }
        }
    }

    pub fn set_gradle_download_sources(&self, value: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.gradle_download_sources = value;
        }
    }

    pub fn set_gradle_jvm_args(&self, value: &QString) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.gradle_jvm_args = value
                .to_string()
                .split_whitespace()
                .map(str::to_string)
                .collect();
        }
    }

    pub fn set_maven_home(&self, value: &QString) {
        let text = value.to_string();
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.maven_home = (!text.trim().is_empty()).then_some(text);
        }
    }

    pub fn set_maven_user_settings_file(&self, value: &QString) {
        set_path_field(&self.draft, value, |draft| {
            &mut draft.maven_user_settings_file
        });
    }

    pub fn set_maven_local_repository(&self, value: &QString) {
        set_path_field(&self.draft, value, |draft| {
            &mut draft.maven_local_repository
        });
    }

    pub fn set_maven_offline(&self, value: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.maven_offline = value;
        }
    }

    pub fn set_maven_skip_tests(&self, value: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.maven_skip_tests = value;
        }
    }

    pub fn set_maven_threads(&self, value: &QString) {
        let text = value.to_string();
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.maven_threads = (!text.trim().is_empty()).then_some(text);
        }
    }

    pub fn set_maven_always_update_snapshots(&self, value: bool) {
        if let Some(draft) = self.draft.borrow_mut().as_mut() {
            draft.maven_always_update_snapshots = value;
        }
    }

    pub fn set_maven_auto_reload(&self, id: &QString) {
        if let Some(mode) = settings_model::build_tools::AutoReload::from_id(&id.to_string()) {
            if let Some(draft) = self.draft.borrow_mut().as_mut() {
                draft.maven_auto_reload = mode;
            }
        }
    }

    pub fn is_dirty(&self) -> bool {
        *self.draft.borrow() != *self.saved.borrow()
    }

    pub fn commit(&self) -> ffi::FfiResult {
        let Some(draft) = self.draft.borrow().clone() else {
            return ffi::FfiResult::default();
        };
        if !draft.validate().is_empty() {
            return errors::failure(errors::CODE_REFUSED, "fix the highlighted fields first");
        }
        if *self.scope.borrow() == settings_model::Scope::Project {
            let gradle = draft.to_gradle_settings();
            let maven = draft.to_maven_settings();
            let result = crate::bridge::settings::commit_to_project(move |project| {
                project.build_tools = Some(app_config::BuildToolsProjectSettings { gradle, maven });
            });
            if result.code == 0 {
                *self.saved.borrow_mut() = Some(draft);
            }
            return result;
        }
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return errors::failure(errors::CODE_REFUSED, "could not read global settings");
        };
        draft.apply_to(&mut settings);
        if app_config::save(&config_dir, &settings).is_err() {
            return errors::failure(errors::CODE_REFUSED, "could not save global settings");
        }
        *self.saved.borrow_mut() = Some(draft);
        ffi::FfiResult::default()
    }
}

/// Write `value` (empty text means "unset") into an `Option<PathBuf>` field
/// the closure picks out of the draft — one helper for the four path-typed
/// fields (`gradleHome`/`gradleJavaHome`/`mavenUserSettingsFile`/
/// `mavenLocalRepository`) rather than four copies of the same three lines.
fn set_path_field(
    draft: &RefCell<Option<settings_model::build_tools::BuildToolsDraft>>,
    value: &QString,
    field: impl FnOnce(&mut settings_model::build_tools::BuildToolsDraft) -> &mut Option<PathBuf>,
) {
    let text = value.to_string();
    if let Some(draft) = draft.borrow_mut().as_mut() {
        *field(draft) = (!text.trim().is_empty()).then(|| PathBuf::from(text));
    }
}

fn path_text(path: &Option<PathBuf>) -> QString {
    QString::from(
        path.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
            .as_str(),
    )
}

fn to_ffi_fields(draft: &settings_model::build_tools::BuildToolsDraft) -> ffi::FfiBuildToolsFields {
    ffi::FfiBuildToolsFields {
        gradle_distribution: QString::from(draft.gradle_distribution.id()),
        gradle_home: path_text(&draft.gradle_home),
        gradle_java_home: path_text(&draft.gradle_java_home),
        gradle_offline: draft.gradle_offline,
        gradle_auto_reload: QString::from(draft.gradle_auto_reload.id()),
        gradle_download_sources: draft.gradle_download_sources,
        gradle_jvm_args: QString::from(draft.gradle_jvm_args.join(" ").as_str()),
        maven_home: QString::from(draft.maven_home.clone().unwrap_or_default().as_str()),
        maven_user_settings_file: path_text(&draft.maven_user_settings_file),
        maven_local_repository: path_text(&draft.maven_local_repository),
        maven_offline: draft.maven_offline,
        maven_skip_tests: draft.maven_skip_tests,
        maven_threads: QString::from(draft.maven_threads.clone().unwrap_or_default().as_str()),
        maven_always_update_snapshots: draft.maven_always_update_snapshots,
        maven_auto_reload: QString::from(draft.maven_auto_reload.id()),
    }
}

fn to_ffi_field(field: settings_model::build_tools::BuildToolsField) -> ffi::FfiBuildToolsField {
    use settings_model::build_tools::BuildToolsField;
    match field {
        BuildToolsField::GradleHome => ffi::FfiBuildToolsField::GradleHome,
        BuildToolsField::GradleAutoReload => ffi::FfiBuildToolsField::GradleAutoReload,
        BuildToolsField::MavenHome => ffi::FfiBuildToolsField::MavenHome,
        BuildToolsField::MavenAutoReload => ffi::FfiBuildToolsField::MavenAutoReload,
        BuildToolsField::MavenThreads => ffi::FfiBuildToolsField::MavenThreads,
    }
}
