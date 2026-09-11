use std::path::{Path, PathBuf};
use std::process::Command;

/// Every `.cpp` in `third_party/qt-advanced-docking-system/src`
/// (`ads_SRCS` in its own `CMakeLists.txt`), minus the platform-specific
/// `linux/FloatingWidgetTitleBar.cpp` handled separately below.
const ADS_SOURCES: &[&str] = &[
    "ads_globals.cpp",
    "DockAreaTabBar.cpp",
    "DockAreaTitleBar.cpp",
    "DockAreaWidget.cpp",
    "DockContainerWidget.cpp",
    "DockManager.cpp",
    "DockOverlay.cpp",
    "DockSplitter.cpp",
    "DockWidget.cpp",
    "DockWidgetTab.cpp",
    "DockingStateReader.cpp",
    "DockFocusController.cpp",
    "ElidingLabel.cpp",
    "FloatingDockContainer.cpp",
    "FloatingDragPreview.cpp",
    "IconProvider.cpp",
    "DockComponentsFactory.cpp",
    "AutoHideSideBar.cpp",
    "AutoHideTab.cpp",
    "AutoHideDockContainer.cpp",
    "PushButton.cpp",
    "ResizeHandle.cpp",
];

/// `ads_HEADERS` from the same `CMakeLists.txt`, all moc'd manually (see
/// `moc_ads_header`) regardless of whether each one actually declares a
/// `Q_OBJECT` class — moc is a no-op (empty output) on the two that don't
/// (IconProvider.h, DockComponentsFactory.h), so filtering them out isn't
/// worth the upkeep of tracking which ones matter.
const ADS_HEADERS: &[&str] = &[
    "ads_globals.h",
    "DockAreaTabBar.h",
    "DockAreaTitleBar.h",
    "DockAreaTitleBar_p.h",
    "DockAreaWidget.h",
    "DockContainerWidget.h",
    "DockManager.h",
    "DockOverlay.h",
    "DockSplitter.h",
    "DockWidget.h",
    "DockWidgetTab.h",
    "DockingStateReader.h",
    "DockFocusController.h",
    "ElidingLabel.h",
    "FloatingDockContainer.h",
    "FloatingDragPreview.h",
    "IconProvider.h",
    "DockComponentsFactory.h",
    "AutoHideSideBar.h",
    "AutoHideTab.h",
    "AutoHideDockContainer.h",
    "PushButton.h",
    "ResizeHandle.h",
];

fn qmake_query(qmake: &str, var: &str) -> Option<String> {
    let output = Command::new(qmake).args(["-query", var]).output().ok()?;
    if !output.status.success() {
        return None;
    }
    Some(String::from_utf8(output.stdout).ok()?.trim().to_string())
}

/// Directories to search for a Qt-internal tool (`moc`, `rcc`), host-first.
/// Both are content/syntax tools with no target-arch code, so cross builds
/// (the Windows MXE stage) need the *host*-runnable copy, not the one next
/// to target libraries — same `QT_HOST_LIBEXECS`-before-`QT_INSTALL_LIBEXECS`
/// order qt-build-utils' own (private) `try_qmake_find_tool` uses, just
/// reimplemented here since that resolution isn't exposed publicly.
fn qt_tool_search_dirs(qmake: &str) -> Vec<PathBuf> {
    [
        "QT_HOST_LIBEXECS/get",
        "QT_HOST_LIBEXECS",
        "QT_HOST_BINS/get",
        "QT_HOST_BINS",
        "QT_INSTALL_LIBEXECS/get",
        "QT_INSTALL_LIBEXECS",
        "QT_INSTALL_BINS/get",
        "QT_INSTALL_BINS",
    ]
    .iter()
    .filter_map(|var| qmake_query(qmake, var))
    .map(PathBuf::from)
    .collect()
}

/// Qt's private headers (needed only by `ads_globals.cpp`'s
/// `<qpa/qplatformnativeinterface.h>` include, guarded to Unix-not-macOS in
/// the same way the xcb integration below is) aren't on the public
/// `Qt6Gui` include path, and `CxxQtBuilder` has no method to add a raw
/// include path outside its own crate — so this is discovered the same way
/// qmake's own private-header consumers do: ask qmake for
/// `QT_INSTALL_HEADERS` and look for the version-numbered `QtGui/<ver>/QtGui`
/// subdirectory underneath it.
fn qt_private_gui_include_dir(qmake: &str) -> Option<PathBuf> {
    let base = PathBuf::from(qmake_query(qmake, "QT_INSTALL_HEADERS")?);
    let gui_dir = base.join("QtGui");
    for entry in std::fs::read_dir(&gui_dir).ok()?.flatten() {
        let candidate = entry.path().join("QtGui");
        if candidate.join("qpa/qplatformnativeinterface.h").is_file() {
            return Some(candidate);
        }
    }
    None
}

/// `moc` and `rcc` rewrite their output unconditionally, and every file this
/// script hands to `cpp_file()` becomes a `rerun-if-changed` input — so a
/// freshly stamped generated file makes the build script dirty on its own
/// next run, forever. That cost a full `ui-shell` recompile and `app` relink
/// on *every* build, including no-op ones. Running the tool into a temp file
/// and keeping the old output when the bytes match breaks the cycle.
fn replace_if_changed(temp: &Path, output: &Path) {
    let generated = std::fs::read(temp).expect("generated file is readable");
    if std::fs::read(output).ok().as_deref() == Some(generated.as_slice()) {
        let _ = std::fs::remove_file(temp);
        return;
    }
    std::fs::rename(temp, output).expect("generated file is movable into place");
}

/// Compiles `ads.qrc` via `rcc` directly rather than `CxxQtBuilder::qrc()`.
/// `qrc()` derives the generated resource-initializer function's name from
/// the qrc file's own filename (`ads.qrc` -> `qInitResources_ads_qrc`, dots
/// replaced with underscores — see `QtToolRcc::compile` in `qt-build-utils`),
/// but `DockManager.cpp` calls `Q_INIT_RESOURCE(ads)` itself, which expands
/// to `qInitResources_ads()` — a name only `rcc --name ads` produces.
/// Mismatching either way is an undefined-symbol link error, not a subtle
/// bug, so this was caught immediately rather than shipped silently broken.
fn compile_ads_qrc(ads_dir: &Path, tool_dirs: &[PathBuf]) -> PathBuf {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let output = out_dir.join("ads_resources.cpp");
    let temp = out_dir.join("ads_resources.cpp.new");
    let qrc_file = ads_dir.join("ads.qrc");

    let candidates: Vec<PathBuf> = tool_dirs
        .iter()
        .map(|dir| dir.join("rcc"))
        .chain(["rcc6", "rcc"].map(PathBuf::from))
        .collect();
    for rcc in &candidates {
        let status = Command::new(rcc)
            .arg(&qrc_file)
            .arg("-o")
            .arg(&temp)
            .args(["--name", "ads"])
            .status();
        if matches!(status, Ok(s) if s.success()) {
            replace_if_changed(&temp, &output);
            return output;
        }
    }
    panic!(
        "could not run rcc (tried: {candidates:?}) to compile {}",
        qrc_file.display()
    );
}

/// Compiles the hand-authored `.ts` translation sources (translations/
/// README.md) into a `.qm` per locale via `lrelease`, then bundles all of
/// them into one `rcc`-embedded resource, `--name translations` so
/// `main_window.cpp`'s `Q_INIT_RESOURCE(translations)` call matches the
/// generated `qInitResources_translations()` symbol — same reasoning as
/// `compile_ads_qrc` above. There is no `ide_en.ts`/`.qm`: English is the
/// `tr()` source text itself, so an untranslated id (or `ui_locale == "en"`)
/// falls back to it without a translator installed at all.
fn compile_translations_qrc(translations_dir: &Path, tool_dirs: &[PathBuf]) -> PathBuf {
    const LOCALES: [&str; 3] = ["de", "es", "fr"];
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));

    let lrelease_candidates: Vec<PathBuf> = tool_dirs
        .iter()
        .map(|dir| dir.join("lrelease"))
        .chain(["lrelease6", "lrelease"].map(PathBuf::from))
        .collect();

    let mut qm_files = Vec::new();
    for locale in LOCALES {
        let ts_file = translations_dir.join(format!("ide_{locale}.ts"));
        println!("cargo:rerun-if-changed={}", ts_file.display());
        let output = out_dir.join(format!("ide_{locale}.qm"));
        let temp = output.with_extension("qm.new");

        let mut compiled = false;
        for lrelease in &lrelease_candidates {
            let status = Command::new(lrelease)
                .arg(&ts_file)
                .arg("-qm")
                .arg(&temp)
                .status();
            if matches!(status, Ok(s) if s.success()) {
                replace_if_changed(&temp, &output);
                compiled = true;
                break;
            }
        }
        if !compiled {
            panic!(
                "could not run lrelease (tried: {lrelease_candidates:?}) on {}",
                ts_file.display()
            );
        }
        qm_files.push(output);
    }

    let qrc_path = out_dir.join("translations.qrc");
    let mut qrc = String::from("<RCC>\n  <qresource prefix=\"/i18n\">\n");
    for qm in &qm_files {
        qrc.push_str(&format!(
            "    <file alias=\"{}\">{}</file>\n",
            qm.file_name().unwrap().to_string_lossy(),
            qm.display()
        ));
    }
    qrc.push_str("  </qresource>\n</RCC>\n");
    let qrc_temp = out_dir.join("translations.qrc.new");
    std::fs::write(&qrc_temp, &qrc).expect("translations.qrc is writable");
    replace_if_changed(&qrc_temp, &qrc_path);

    let rcc_candidates: Vec<PathBuf> = tool_dirs
        .iter()
        .map(|dir| dir.join("rcc"))
        .chain(["rcc6", "rcc"].map(PathBuf::from))
        .collect();
    let output = out_dir.join("translations_resources.cpp");
    let temp = output.with_extension("cpp.new");
    for rcc in &rcc_candidates {
        let status = Command::new(rcc)
            .arg(&qrc_path)
            .arg("-o")
            .arg(&temp)
            .args(["--name", "translations"])
            .status();
        if matches!(status, Ok(s) if s.success()) {
            replace_if_changed(&temp, &output);
            return output;
        }
    }
    panic!(
        "could not run rcc (tried: {rcc_candidates:?}) to compile {}",
        qrc_path.display()
    );
}

/// Runs `moc` on an ADS header directly rather than through
/// `CxxQtBuilder::cpp_file()`'s automatic moc: `MocArguments` has no way to
/// pass a `-D` define, but ADS's `FloatingDockContainer.h` branches its own
/// base class on `Q_OS_WIN`/`Q_OS_UNIX`, and moc's minimal preprocessor
/// doesn't get the cross target's predefined macros for free the way the
/// real `x86_64-w64-mingw32-g++` compiling the rest of the sources does —
/// moc is always the *host*'s own moc binary (Linux here, even when
/// targeting Windows), so left unguided it silently mis-selects the Linux
/// branch (`QDockWidget`) instead of Windows's (`QWidget`), which then fails
/// to link, not compile — verified this actually flips the branch with a
/// throwaway test header before relying on it.
fn moc_ads_header(
    header: &str,
    ads_dir: &Path,
    tool_dirs: &[PathBuf],
    includes: &[PathBuf],
    is_windows: bool,
) -> PathBuf {
    let out_dir = PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR not set"));
    let header_path = ads_dir.join(header);
    let output = out_dir.join(format!("moc_{}.cpp", header.replace(['/', '.'], "_")));
    let temp = output.with_extension("cpp.new");

    let candidates: Vec<PathBuf> = tool_dirs
        .iter()
        .map(|dir| dir.join("moc"))
        .chain(["moc6", "moc"].map(PathBuf::from))
        .collect();
    for moc in &candidates {
        let mut cmd = Command::new(moc);
        cmd.arg("-I").arg(ads_dir);
        for include in includes {
            cmd.arg("-I").arg(include);
        }
        if is_windows {
            cmd.arg("-DQ_OS_WIN");
        }
        cmd.arg(&header_path).arg("-o").arg(&temp);
        if matches!(cmd.status(), Ok(s) if s.success()) {
            replace_if_changed(&temp, &output);
            return output;
        }
    }
    panic!(
        "could not run moc (tried: {candidates:?}) on {}",
        header_path.display()
    );
}

/// ADS's `ads_globals.h` defaults `ADS_EXPORT` to `Q_DECL_IMPORT` unless
/// `ADS_STATIC` is defined (CMake's static-build path sets it via
/// `target_compile_definitions`); left unset here, that's a dllimport
/// attribute on every ADS class with no matching dllexport anywhere, since
/// everything is linked directly into one binary rather than a shared
/// library. `CxxQtBuilder`/`CppFile` have no method to add a raw `-D` define
/// to the underlying `cc::Build`, so this leans on `cc`'s own documented
/// `CXXFLAGS` env-var fallback (`Build::envflags`) instead — set before
/// `.build()` runs, so it's already visible when `cc` reads it.
fn set_ads_static_define() {
    let existing = std::env::var("CXXFLAGS").unwrap_or_default();
    let flags = format!("{existing} -DADS_STATIC").trim().to_string();
    std::env::set_var("CXXFLAGS", flags);
}

/// What the About dialog shows: the commit this binary was built from and
/// when that commit was authored.
///
/// Read here rather than at runtime because the shipped binary has no
/// repository of its own to ask — a runtime `git` call in an installed build
/// would report whatever project the user happens to have open, which is a
/// different fact wearing the same name.
///
/// Every failure degrades to `unknown`: a source tarball has no `.git`, a
/// minimal build image may have no `git`, and neither is a reason to fail a
/// build over a line in a dialog.
fn emit_build_metadata() {
    const UNKNOWN: &str = "unknown";

    let git = |args: &[&str]| -> String {
        Command::new("git")
            // Builds run in the container as root against a bind-mounted,
            // host-owned tree, which is exactly what git's dubious-ownership
            // guard refuses. Without this every containerised build — which
            // is every build (CLAUDE.md) — reports `unknown`.
            .args(["-c", "safe.directory=*"])
            .args(args)
            .output()
            .ok()
            .filter(|out| out.status.success())
            .and_then(|out| String::from_utf8(out.stdout).ok())
            .map(|text| text.trim().to_string())
            .filter(|text| !text.is_empty())
            .unwrap_or_else(|| UNKNOWN.to_string())
    };

    println!(
        "cargo:rustc-env=KESTREL_GIT_HASH={}",
        git(&["rev-parse", "--short", "HEAD"])
    );
    println!(
        "cargo:rustc-env=KESTREL_GIT_DATE={}",
        git(&["log", "-1", "--date=format:%Y-%m-%d", "--format=%cd"])
    );

    // No separate build timestamp: `git` is the only clock formatter here
    // without adding a `time`/`chrono` build dependency, and the commit date
    // is the date the About dialog is actually claiming — the version this
    // build is based on.

    // This script has opted out of cargo's rerun-on-any-change default (see
    // main()), so without these the hash is frozen at whatever it was the
    // first time the crate compiled. `.git/HEAD` alone is not enough: it only
    // changes when the *branch* does, and committing on the same branch moves
    // the ref file it points at instead.
    let git_dir = Path::new("../../.git");
    let head = git_dir.join("HEAD");
    if head.exists() {
        println!("cargo:rerun-if-changed={}", head.display());
        if let Some(reference) = std::fs::read_to_string(&head)
            .ok()
            .and_then(|text| text.trim().strip_prefix("ref: ").map(str::to_string))
        {
            // Only when it is still a loose ref; a packed one lives in
            // `packed-refs`, which the next line covers.
            let ref_path = git_dir.join(&reference);
            if ref_path.exists() {
                println!("cargo:rerun-if-changed={}", ref_path.display());
            }
        }
        let packed = git_dir.join("packed-refs");
        if packed.exists() {
            println!("cargo:rerun-if-changed={}", packed.display());
        }
    }
}

fn main() {
    let ads_dir = Path::new("../../third_party/qt-advanced-docking-system/src");
    set_ads_static_define();
    emit_build_metadata();
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    let is_windows = target_os == "windows";
    // ads_globals.h only pulls in xcb/QPA on Unix-not-macOS (matches
    // CMakeLists.txt's own `if (UNIX AND NOT APPLE)` guard); the Windows MXE
    // cross-build never compiles that path, so it needs neither the extra
    // link lib nor the private Qt include below.
    let needs_xcb = !is_windows && target_os != "macos";

    let qmake = std::env::var("QMAKE").unwrap_or_else(|_| "qmake6".to_string());
    let tool_dirs = qt_tool_search_dirs(&qmake);
    let qt_headers_dir = qmake_query(&qmake, "QT_INSTALL_HEADERS").map(PathBuf::from);
    let moc_includes: Vec<PathBuf> = qt_headers_dir
        .iter()
        .flat_map(|base| {
            [
                base.clone(),
                base.join("QtCore"),
                base.join("QtGui"),
                base.join("QtWidgets"),
            ]
        })
        .collect();

    // cxx-qt-build tracks the Rust bridge, not the hand-written C++, and any
    // `rerun-if-changed` at all switches cargo off its "rerun on any change in
    // the package" default — so every input this script reads is listed here.
    // Without it an edit to `cpp/` silently keeps the previously compiled
    // objects and the app runs stale view code.
    for entry in std::fs::read_dir("cpp").expect("cpp/ must exist") {
        let path = entry.expect("readable cpp/ entry").path();
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rerun-if-changed=src/bridge/ffi.rs");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", ads_dir.display());

    let mut builder = cxx_qt_build::CxxQtBuilder::new()
        .file("src/bridge/ffi.rs")
        .cpp_file("cpp/main_window.cpp")
        .cpp_file("cpp/e2e_mark.cpp")
        // F0-7: the dock show/hide registry. Free of Q_OBJECT (plain class,
        // no signals/slots), so only the source is listed.
        .cpp_file("cpp/dock_layout.cpp")
        // The Help menu and its About dialog. No Q_OBJECT (free functions and
        // a locally built QDialog), so only the source is listed.
        .cpp_file("cpp/help_menu.cpp")
        .cpp_file("cpp/layouts_menu.cpp")
        // The project tree's Git submenu. No Q_OBJECT (one free function),
        // so only the source is listed.
        .cpp_file("cpp/project_tree_git_menu.cpp")
        // The Discard Changes confirm dialog shared by the project tree's
        // Git submenu and the Changes dock (G8). No Q_OBJECT.
        .cpp_file("cpp/git_dialogs.cpp")
        .cpp_file("cpp/rounded_corners.cpp")
        .cpp_file("cpp/panel_shadow.cpp")
        // First hand-written (non-generated) QObject in this crate: header
        // passed to cpp_file() auto-enables moc (CppFile::from, cxx-qt-build
        // 0.9), so this is also the first place build.rs runs moc directly.
        .cpp_file("cpp/code_editor.h")
        .cpp_file("cpp/code_editor.cpp")
        .cpp_file("cpp/code_editor_gutter.cpp")
        // Editor minimap (issue #199): no Q_OBJECT, so only the source is
        // listed, same as vcs_gutter.cpp below.
        .cpp_file("cpp/minimap.cpp")
        // F3-16: the change-marker colour/kind and the hunk popup. Free
        // functions and plain structs, no Q_OBJECT, so only the source is
        // listed — same as signature_tip.cpp.
        .cpp_file("cpp/vcs_gutter.cpp")
        .cpp_file("cpp/find_bar.h")
        .cpp_file("cpp/find_bar.cpp")
        .cpp_file("cpp/intention_bulb.h")
        .cpp_file("cpp/intention_bulb.cpp")
        // Free functions, no Q_OBJECT, so only the source is listed.
        .cpp_file("cpp/signature_tip.cpp")
        // EditorTabs is one class defined across three translation units:
        // the tab surface, the pane tree, and the language-server leg.
        // It declares no Q_OBJECT (main_window.cpp holds none by design),
        // so only the sources are listed — its header runs no moc.
        .cpp_file("cpp/editor_tabs.cpp")
        .cpp_file("cpp/editor_tabs_panes.cpp")
        .cpp_file("cpp/editor_tabs_lsp.cpp")
        // F3-16: the gutter's change markers and the hunk popup — a fourth
        // leg of EditorTabs, same reasoning as the other three.
        .cpp_file("cpp/editor_tabs_vcs.cpp")
        .cpp_file("cpp/editor_tabs_run.cpp")
        .cpp_file("cpp/editor_tabs_debug.cpp")
        // The in-tab preview mode (view mode), a sixth leg for the same
        // reason as the five above.
        .cpp_file("cpp/editor_tabs_preview.cpp")
        .cpp_file("cpp/build_panel.h")
        .cpp_file("cpp/build_panel.cpp")
        .cpp_file("cpp/build_menu.cpp")
        .cpp_file("cpp/debug_panel.h")
        .cpp_file("cpp/debug_panel.cpp")
        .cpp_file("cpp/debug_menu.cpp")
        .cpp_file("cpp/build_panel.h")
        .cpp_file("cpp/build_panel.cpp")
        .cpp_file("cpp/build_menu.cpp")
        // The two navigation docks and the QMainWindow subclass, likewise
        // Q_OBJECT-free (they subclass QWidget/QMainWindow but declare no
        // signals or slots of their own), so only the sources are listed.
        .cpp_file("cpp/structure_panel.cpp")
        .cpp_file("cpp/find_usages_panel.cpp")
        .cpp_file("cpp/hierarchy_panel.cpp")
        .cpp_file("cpp/ide_main_window.cpp")
        // The refactoring and Go-to-Declaration controllers, likewise
        // Q_OBJECT-free: they subclass QObject for parent ownership and
        // for connect()'s pointer-to-member overload, neither of which
        // needs moc, so only the sources are listed.
        .cpp_file("cpp/refactor_controller.cpp")
        .cpp_file("cpp/declaration_navigator.cpp")
        .cpp_file("cpp/symbol_kind_label.cpp")
        .cpp_file("cpp/symbol_icon.cpp")
        // The Edit menu's editing operations (F1-16): free functions, no
        // Q_OBJECT, so only the source is listed.
        .cpp_file("cpp/editing_actions.cpp")
        .cpp_file("cpp/keymap_page.cpp")
        .cpp_file("cpp/syntax_colors_page.cpp")
        .cpp_file("cpp/languages_page.cpp")
        .cpp_file("cpp/plugins_page.cpp")
        .cpp_file("cpp/file_associations_page.cpp")
        .cpp_file("cpp/appearance_page.cpp")
        .cpp_file("cpp/language_page.cpp")
        .cpp_file("cpp/i18n_startup.cpp")
        .cpp_file("cpp/language_servers_page.cpp")
        // The PHP tooling plan's B9: the Analysis settings page and its
        // menu action, Q_OBJECT-free like the pages/menus above.
        .cpp_file("cpp/analysis_settings_page.cpp")
        .cpp_file("cpp/analysis_menu.cpp")
        // The settings dialog and the last two pages that were still built
        // inline inside it. Q_OBJECT-free like the pages above — the dialog
        // is a stack-allocated QDialog and the pages are plain QWidgets
        // wired with lambdas — so only the sources are listed.
        .cpp_file("cpp/settings_dialog.cpp")
        .cpp_file("cpp/editor_page.cpp")
        .cpp_file("cpp/editing_page.cpp")
        .cpp_file("cpp/mcp_page.cpp")
        .cpp_file("cpp/terminal_page.cpp")
        .cpp_file("cpp/tab_padding_page.cpp")
        .cpp_file("cpp/problems_panel.cpp")
        // The PHP tooling plan's D5: the Tests dock. Q_OBJECT-free like
        // `build_panel.cpp` (plain QWidget, lambdas and pointer-to-member
        // connects), so only the source is listed.
        .cpp_file("cpp/tests_panel.cpp")
        .cpp_file("cpp/tests_menu.cpp")
        .cpp_file("cpp/icon_cache.cpp")
        // Declares Q_OBJECT (it overrides QIdentityProxyModel::data), so its
        // header is listed too — that is what runs moc on it.
        .cpp_file("cpp/icon_decoration_proxy.h")
        .cpp_file("cpp/icon_decoration_proxy.cpp")
        .cpp_file("cpp/recent_projects_menu.cpp")
        .cpp_file("cpp/project_tree_dock.cpp")
        .cpp_file("cpp/search_results_panel.cpp")
        .cpp_file("cpp/refactor_preview_dialog.cpp")
        // Declares Q_OBJECT, so its header is listed too — that is what
        // runs moc on it.
        .cpp_file("cpp/diff_view.h")
        .cpp_file("cpp/diff_view.cpp")
        // The diff viewer's parts: the shared extra-selection builder, the
        // read-only pane with its gutter (no Q_OBJECT — functor connects
        // only), and the divider (Q_OBJECT, so its header is listed).
        .cpp_file("cpp/diff_selections.cpp")
        .cpp_file("cpp/diff_pane.cpp")
        .cpp_file("cpp/diff_divider.h")
        .cpp_file("cpp/diff_divider.cpp")
        // The chrome around `DiffView`: the toolbar and the page composing
        // it. Both declare Q_OBJECT, so their headers are listed too.
        .cpp_file("cpp/diff_toolbar.h")
        .cpp_file("cpp/diff_toolbar.cpp")
        .cpp_file("cpp/diff_view_page.h")
        .cpp_file("cpp/diff_view_page.cpp")
        .cpp_file("cpp/unified_diff_view.h")
        .cpp_file("cpp/unified_diff_view.cpp")
        .cpp_file("cpp/search_everywhere_dialog.cpp")
        .cpp_file("cpp/splash_screen.cpp")
        .cpp_file("cpp/theme.cpp")
        .cpp_file("cpp/syntax_highlighter.cpp")
        .cpp_file("cpp/terminal_widget.h")
        .cpp_file("cpp/terminal_widget.cpp")
        // F4-14b: the tabbed multi-session terminal dock over TerminalWidget.
        .cpp_file("cpp/terminal_sessions_panel.h")
        .cpp_file("cpp/terminal_sessions_panel.cpp")
        .cpp_file("cpp/hex_viewer.h")
        .cpp_file("cpp/hex_viewer.cpp")
        // Read-only image tab: raster via QImageReader, SVG
        // via the FFI's renderSvgImage (icon-theme's resvg pipeline).
        .cpp_file("cpp/image_viewer.h")
        .cpp_file("cpp/image_viewer.cpp")
        // The chat panel declares Q_OBJECT, so its header is listed too
        // (passing a header to cpp_file() is what runs moc on it); the
        // providers page is a free function like every other settings
        // page and needs none.
        .cpp_file("cpp/ai_chat_panel.h")
        .cpp_file("cpp/ai_chat_panel.cpp")
        .cpp_file("cpp/ai_providers_page.cpp")
        // The Preview dock (ADR-0033). Declares Q_OBJECT for
        // `anchorClicked`/`previewReady`'s slots, so its header is listed
        // too, same as the chat panel above.
        .cpp_file("cpp/markdown_preview_panel.h")
        .cpp_file("cpp/markdown_preview_panel.cpp")
        // F3-17/18/19: the Changes/File History docks and the VCS menu.
        // Q_OBJECT-free like the other panels/pages above, so only the
        // sources are listed.
        .cpp_file("cpp/changes_panel.cpp")
        // The Changes dock's toolbar (G6). Q_OBJECT (its Refresh/Fetch/
        // Pull/Push/Stage all/Unstage all signals), so its header is listed
        // too, same as `diff_toolbar.h` above.
        .cpp_file("cpp/changes_toolbar.h")
        .cpp_file("cpp/changes_toolbar.cpp")
        // Q_OBJECT (signals `commitActivated`/`contextMenuRequestedFor`),
        // so its header is listed too, same as the chat/preview panels
        // above.
        .cpp_file("cpp/history_list_view.h")
        .cpp_file("cpp/history_list_view.cpp")
        .cpp_file("cpp/file_history_panel.cpp")
        .cpp_file("cpp/commit_log_panel.cpp")
        .cpp_file("cpp/commit_detail_panel.cpp")
        // Q_OBJECT (needed for `qobject_cast` in
        // `CommitDetailPanel::closeTab`), so its header is listed too.
        .cpp_file("cpp/commit_detail_view.h")
        .cpp_file("cpp/commit_detail_view.cpp")
        .cpp_file("cpp/vcs_menu.cpp")
        // F4-11/F4-12: the Run Console dock, its toolbar, the run
        // configuration dialog and the Run menu. Q_OBJECT-free, same as the
        // VCS panels above.
        .cpp_file("cpp/run_toolbar.cpp")
        .cpp_file("cpp/run_console_panel.cpp")
        .cpp_file("cpp/run_config_dialog.cpp")
        .cpp_file("cpp/run_menu.cpp")
        // Split out of main_window.cpp to keep it under its 1200-line
        // ceiling (ADR-0025): the status bar's permanent widgets, the
        // Navigate menu, and the AI chat wiring/menu. Q_OBJECT-free, same
        // as the VCS/Run menus above.
        .cpp_file("cpp/status_bar.cpp")
        .cpp_file("cpp/navigate_menu.cpp")
        .cpp_file("cpp/ai_menu.cpp")
        .include_dir("cpp")
        .include_dir(ads_dir)
        .cpp_file(compile_ads_qrc(ads_dir, &tool_dirs))
        .cpp_file(compile_translations_qrc(
            Path::new("translations"),
            &tool_dirs,
        ))
        // The close-icon mask (F?): a plain qrc, named by us, so CxxQtBuilder's
        // own filename-derived init symbol works — unlike ads.qrc above, no
        // manual rcc step is needed here.
        .qrc("resources/ui_icons.qrc")
        .qt_module("Widgets")
        // Widgets code uses QTextDocument (QtGui) directly. On Linux this
        // resolves transitively via the shared Qt6Widgets.so's own NEEDED
        // entry, but MinGW/PE import-library linking requires every module
        // whose symbols are referenced to be listed explicitly.
        .qt_module("Gui");

    for header in ADS_HEADERS {
        let moc_output = moc_ads_header(header, ads_dir, &tool_dirs, &moc_includes, is_windows);
        builder = builder.cpp_file(moc_output);
    }
    for source in ADS_SOURCES {
        builder = builder.cpp_file(ads_dir.join(source));
    }
    if is_windows {
        // DwmSetWindowAttribute (native rounded-corner + shadow opt-in on
        // Windows 11, applyNativeWindowChrome() in main_window.cpp).
        println!("cargo:rustc-link-lib=dwmapi");
    }
    if needs_xcb {
        // Matches CMakeLists.txt's `if (UNIX AND NOT APPLE) ... linux/FloatingWidgetTitleBar` block.
        let linux_header = "linux/FloatingWidgetTitleBar.h";
        let moc_output =
            moc_ads_header(linux_header, ads_dir, &tool_dirs, &moc_includes, is_windows);
        builder = builder
            .cpp_file(moc_output)
            .cpp_file(ads_dir.join("linux/FloatingWidgetTitleBar.cpp"));
        println!("cargo:rustc-link-lib=xcb");
        if let Some(private_dir) = qt_private_gui_include_dir(&qmake) {
            builder = builder.include_dir(private_dir);
        }
    }

    builder.build();
}
