//! Process-wide handles the adapters share.
//!
//! Every singleton here exists for the same reason: cxx-qt constructs the
//! Rust side of a QObject through `Default` when C++ does `new Foo(parent)`,
//! so there is no constructor-injection point to hand a shared instance in
//! through. What must be shared therefore lives in a `thread_local!` (for
//! Qt-thread-only state) or a `OnceLock` (for state background threads
//! reach), and each `Default` impl takes a handle from here.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex, OnceLock};

use app_core::AppSession;

thread_local! {
    /// The single `AppSession` both QObject adapters share. cxx-qt
    /// constructs the Rust structs via `Default` when C++ does
    /// `new ProjectTreeModel(window)` — there is no constructor-injection
    /// path — so the shared instance lives in a thread-local both `Default`
    /// impls clone. Sound because all QObjects (and every slot/signal here)
    /// live on the single Qt UI thread; the watcher thread never touches the
    /// session directly, it queues closures onto the Qt thread via
    /// `CxxQtThread` first.
    static APP_SESSION: Rc<RefCell<AppSession>> = Rc::new(RefCell::new(AppSession::new()));
}

pub(crate) fn shared_session() -> Rc<RefCell<AppSession>> {
    APP_SESSION.with(Rc::clone)
}

/// The one icon theme in this process, plus the appearance the current
/// colour theme asks for.
///
/// Shared for the same reason `AppSession` is — cxx-qt has no injection
/// point — and shared rather than duplicated for a second reason: the
/// renderer memoises rasterised icons, and a second copy would rasterise
/// the whole tree again for the tab strip.
pub(crate) struct SharedIcons {
    /// `&mut` only for the renderer's cache, so the interior mutability
    /// stops at this cell rather than reaching into `app-core`.
    pub(crate) service: RefCell<app_core::icons::IconService>,
    /// Which art the colour theme in force asks for. A `Cell` because the
    /// Appearance page switches themes live and the icons follow within the
    /// same repaint — a light theme wearing the dark icon set is exactly the
    /// mismatch the pack ships light variants to avoid.
    pub(crate) appearance: Cell<app_core::icons::Appearance>,
}

thread_local! {
    /// Booting the plugin host: one scan, and everything that comes out of
    /// it. The icon theme and the wasm tier are two answers to the same
    /// question — which plugins are loaded — so they are started together
    /// and from the user's one `disabled_plugins` list.
    static ICONS: Rc<SharedIcons> = Rc::new({
        let config_dir = app_core::resolve_config_dir();
        let settings = app_config::load(&config_dir).unwrap_or_default();
        // Scans and swaps the process-wide registry, which is what the tier
        // below is then started over — so this call has to come first.
        let service = app_core::icons::IconService::load(
            &config_dir,
            &settings.disabled_plugins,
            &settings.icon_theme,
        );
        start_plugin_tier();
        SharedIcons {
            service: RefCell::new(service),
            appearance: Cell::new(app_core::icons::appearance_for_theme(settings.theme_name())),
        }
    });
}

/// Start (or restart) the wasm tier over the registry as it stands.
///
/// The host services are chosen here because an implementation that routes
/// a plugin's `notify` to the editor is a Qt object, and this crate is the
/// only one allowed to hold one. Until that surface exists, the host's own
/// stderr default is the honest answer — a plugin's diagnostics reach the
/// log rather than being dropped.
pub(crate) fn start_plugin_tier() {
    plugin_host::start_tier(
        std::sync::Arc::new(plugin_host::StderrServices::default()),
        plugin_host::WasmLimits::default(),
    );
}

pub(crate) fn shared_icons() -> Rc<SharedIcons> {
    ICONS.with(Rc::clone)
}

/// The one preview renderer in this process — `Arc<Mutex<_>>`, not the
/// `Rc<RefCell<_>>` every other shared handle here uses, because
/// `PreviewProvider`'s slot (M6) renders off the Qt thread: a request
/// spawns a `std::thread`, and that thread needs to reach the same
/// `app_core::preview::PreviewService` a second request from the Qt thread
/// might reach at the same moment, for the same reason `icon_pixels`
/// shares one `IconService` rather than rasterising twice — the diagram
/// cache only helps if there is one of it.
///
/// Built from whatever `plugin_host::registry()`/`plugin_host::tier()`
/// already hold rather than scanning again: [`shared_icons`] is what
/// performs the one process-wide scan and starts the tier, so this calls
/// it first to guarantee that has happened, exactly as `apply_icon_theme`
/// and `apply_color_theme` rely on the same ordering implicitly today.
pub(crate) fn shared_preview() -> Arc<Mutex<app_core::preview::PreviewService>> {
    static PREVIEW: OnceLock<Arc<Mutex<app_core::preview::PreviewService>>> = OnceLock::new();
    Arc::clone(PREVIEW.get_or_init(|| {
        let _ = shared_icons();
        Arc::new(Mutex::new(
            app_core::preview::PreviewService::from_registry(
                plugin_host::registry(),
                plugin_host::tier(),
            ),
        ))
    }))
}

/// Rebuild the preview service over the registry as it stands — the
/// Plugins page's enable/disable path, so a `previews` provider a plugin
/// toggle just added or removed is picked up the same way `apply_icon_theme`
/// picks up an icon-theme change. Called after [`start_plugin_tier`], for
/// the same reason `IconService::from_registry` is rebuilt after a reload:
/// the wasm tier a preview might dispatch to has to be the one just
/// (re)started over the new registry, not the one from before the toggle.
pub(crate) fn reload_shared_preview() {
    let rebuilt = app_core::preview::PreviewService::from_registry(
        plugin_host::registry(),
        plugin_host::tier(),
    );
    *shared_preview()
        .lock()
        .expect("preview service lock poisoned") = rebuilt;
}

/// The one project index in this process, shared by `SearchModel` (which
/// builds and updates it) and the MCP server (which only queries it).
pub(crate) fn index_slot() -> mcp_server::IndexHandle {
    static INDEX: std::sync::OnceLock<mcp_server::IndexHandle> = std::sync::OnceLock::new();
    std::sync::Arc::clone(INDEX.get_or_init(Default::default))
}

/// The handle `apply_mcp_settings` keeps on a running server so a later
/// call (or app shutdown) can take it down again.
pub(crate) struct McpControl {
    pub(crate) stop: tokio::sync::oneshot::Sender<()>,
    pub(crate) thread: std::thread::JoinHandle<()>,
}

/// There is one MCP server per process, and the QObject that owns its
/// lifecycle is constructed by cxx-qt via `Default` with no place to put
/// state — so the control handle lives here, next to the index slot, for
/// the same reason.
pub(crate) fn mcp_control() -> &'static std::sync::Mutex<Option<McpControl>> {
    static CONTROL: std::sync::OnceLock<std::sync::Mutex<Option<McpControl>>> =
        std::sync::OnceLock::new();
    CONTROL.get_or_init(|| std::sync::Mutex::new(None))
}

/// Stop the running MCP server, if any, and wait for its thread to finish
/// so a restart cannot race the old server for the same port. Returns once
/// the port is free and the discovery file is gone.
pub(crate) fn stop_mcp_server() {
    let Some(control) = mcp_control()
        .lock()
        .expect("MCP control lock poisoned")
        .take()
    else {
        return;
    };
    // A send failure means the thread already exited on its own — nothing
    // to signal, but still something to join.
    let _ = control.stop.send(());
    let _ = control.thread.join();
}

/// The one diagnostic store in this process (ADR-0046), shared by
/// `LanguageService` and `BuildService` (which publish into it, each under
/// its own `(source, uri)` key) and by `DiagnosticsService` and `AiChat`
/// (which read it back — the Problems dock, the editor's squiggles, and
/// `attachDiagnostics`).
///
/// Same reasoning as the `APP_SESSION` thread-local and `index_slot`: cxx-qt
/// builds QObjects through `Default` with no injection point, and a second
/// store would mean the editor underlining a different set of problems than
/// the Problems panel shows — the whole bug class ADR-0046 exists to close.
/// A newtype rather than a bare `Rc` so every QObject keeps its derived
/// `Default`.
pub(crate) struct SharedDiagnostics(Rc<RefCell<diagnostics_core::DiagnosticStore>>);

thread_local! {
    static DIAGNOSTICS: Rc<RefCell<diagnostics_core::DiagnosticStore>> = Rc::default();
}

impl Default for SharedDiagnostics {
    fn default() -> Self {
        SharedDiagnostics(DIAGNOSTICS.with(Rc::clone))
    }
}

impl std::ops::Deref for SharedDiagnostics {
    type Target = RefCell<diagnostics_core::DiagnosticStore>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
