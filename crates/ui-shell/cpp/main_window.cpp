#include "main_window.h"

#include "ai_chat_panel.h"
#include "ai_menu.h"
#include "markdown_preview_panel.h"
#include "appearance_page.h"
#include "changes_panel.h"
#include "build_menu.h"
#include "debug_menu.h"
#include "debug_panel.h"
#include "build_panel.h"
#include "class_view_panel.h"
#include "code_editor.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "editing_actions.h"
#include "editor_tabs.h"
#include "file_history_panel.h"
#include "find_bar.h"
#include "find_usages_panel.h"
#include "hierarchy_panel.h"
#include "hex_viewer.h"
#include "icon_cache.h"
#include "ide_main_window.h"
#include "keymap_page.h"
#include "mcp_page.h"
#include "navigate_menu.h"
#include "panel_shadow.h"
#include "search_everywhere_dialog.h"
#include "problems_panel.h"
#include "icon_decoration_proxy.h"
#include "project_tree_dock.h"
#include "recent_projects_menu.h"
#include "refactor_controller.h"
#include "run_console_panel.h"
#include "run_menu.h"
#include "run_toolbar.h"
#include "search_results_panel.h"
#include "settings_dialog.h"
#include "splash_screen.h"
#include "status_bar.h"
#include "syntax_highlighter.h"
#include "terminal_sessions_panel.h"
#include "theme.h"
#include "rounded_corners.h"
#include "ui_tokens.h"
#include "vcs_menu.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QApplication>
#include <QByteArray>
#include <QSet>
#include <QTimer>
#include <QToolTip>
#include <QFileDialog>
#include <QFont>
#include <QHash>
#include <algorithm>
#include <cstdint>
#include <functional>
#include <memory>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QPlainTextEdit>
#include <QProxyStyle>
#include <QStyle>
#include <QToolBar>
#include <QStringList>
#include <QLayout>
#include <QSplitter>
#include <QStatusBar>
#include <QTabWidget>
#include <QtGui/QTextDocument>
#include <QTreeView>
#include <QWidget>

namespace ui_shell {

namespace {

// Swaps Qt's platform close glyph for ui_shell::tabCloseIcon() (theme.cpp),
// so every plain QTabWidget's close button — editor tab groups, terminal
// session tabs — matches ADS's own dock/tab close buttons, themed the same
// way (see applyTheme()'s ads::CIconProvider registration for that half of
// the swap). Everything else falls through to the base style unchanged.
class TabCloseIconStyle : public QProxyStyle
{
public:
    using QProxyStyle::QProxyStyle;

    QIcon standardIcon(StandardPixmap standardIcon, const QStyleOption *option,
                        const QWidget *widget) const override
    {
        if (standardIcon == QStyle::SP_TabCloseButton) {
            return tabCloseIcon();
        }
        return QProxyStyle::standardIcon(standardIcon, option, widget);
    }
};

// Sidebar tree + tabbed editor area, PHPStorm-style (US-5): each panel is
// its own ADS CDockWidget (D3) — float/redock each independently, room left
// for future dock widgets (search, run console, MCP activity log) without
// restructuring this function again. The editor stays one dock widget (not
// one per open file, per the plan's migration scope): the splits inside it
// are a QSplitter tree of tab groups owned by EditorTabs (D5), invisible to
// ADS, and G2's drag-reorder stays internal to each group's QTabWidget.
// Return value of buildCentralWidget(): the tab-strip adapter (needed by
// menu wiring) plus the dock manager (needed by IdeMainWindow for D4's
// close-time saveState()) — one caller, so a tiny struct beats an
// out-param.
struct CentralWidgets
{
    EditorTabs *editorTabs;
    ads::CDockManager *dockManager;
    // Every side/bottom dock's identity, placement and show/hide now lives
    // in one registry (F0-7) rather than a scattered `toggleView`/`raise()`
    // pair at each call site — see dock_layout.h.
    DockRegistry *docks;
    // Only reason it escapes: the project tree carries its own interface
    // font scale, so buildMainWindow has to be able to hand it to
    // applyUiFontScales().
    QTreeView *projectTree;
    SearchResultsPanel *searchResultsPanel;
    ClassViewPanel *classViewPanel;
    TerminalSessionsPanel *terminalPanel;
    FindUsagesPanel *findUsagesPanel;
    HierarchyPanel *hierarchyPanel;
    SearchEverywhereDialog *searchEverywhereDialog;
    ProblemsPanel *problemsPanel;
    AiChatPanel *aiChatPanel;
    ChangesPanel *changesPanel;
    FileHistoryPanel *fileHistoryPanel;
    RunConsolePanel *runConsolePanel;
    BuildPanel *buildPanel;
    DebugPanel *debugPanel;
    MarkdownPreviewPanel *previewPanel;
};

CentralWidgets buildCentralWidget(QMainWindow *window, ProjectTreeModel *treeModel,
                                   DocumentManager *docManager, AppSettings *appSettings,
                                   SearchModel *searchModel, TerminalSupervisor *terminalSupervisor,
                                   LanguageService *languageService, AiChat *aiChat,
                                   VcsService *vcsService, RunService *runService,
                                   BuildService *buildService, DebugService *debugService,
                                   PreviewProvider *previewProvider)
{
    // Constructing with `window` (a QMainWindow) as parent makes the dock
    // manager install itself as the central widget automatically (ADS's own
    // CDockManager::CDockManager) — no explicit QMainWindow::setCentralWidget().
    // Full tab titles: ADS's default is to elide every title in a strip
    // that overflows ("Cl...", "AI...", "Ch...") and offer a menu instead;
    // with eliding off the strip scrolls, and the titles stay readable.
    ads::CDockManager::setConfigFlag(ads::CDockManager::DisableTabTextEliding, true);
    // A tab strip carries the close button only: the undock and tabs-menu
    // buttons the default configuration adds hold 50px of every strip for
    // two affordances the mockup does not show, and a dock is still
    // undocked by dragging its tab.
    ads::CDockManager::setConfigFlag(ads::CDockManager::DockAreaHasUndockButton, false);
    ads::CDockManager::setConfigFlag(ads::CDockManager::DockAreaHasTabsMenuButton, false);
    auto *dockManager = new ads::CDockManager(window);
    // --panel-gap (blend spec), the margin *around* the docked panels. It has
    // to go on the layout: ADS gives the dock manager a layout of its own in
    // its constructor, and a layout's own margins win over the widget's
    // contentsMargins, so setting them on the widget alone did nothing.
    // The gap *between* two panels is the splitter handle, widened to match
    // in dockStyleSheet().
    if (QLayout *managerLayout = dockManager->layout()) {
        managerLayout->setContentsMargins(tokens::kPanelGap, tokens::kPanelGap, tokens::kPanelGap,
                                          tokens::kPanelGap);
    }
    // The rounded panel card — see rounded_corners.h for why it is neither
    // a QSS radius nor a mask. The 1px margin keeps the area's children off
    // the border line it paints.
    QObject::connect(dockManager, &ads::CDockManager::dockAreaCreated, window,
                     [](ads::CDockAreaWidget *dockArea) {
                         if (QLayout *areaLayout = dockArea->layout()) {
                             areaLayout->setContentsMargins(1, 1, 1, 1);
                         }
                         roundCorners(dockArea, tokens::kRadiusPanel);
                     });
    addPanelShadows(dockManager, tokens::kRadiusPanel);
    auto *docks = new DockRegistry(dockManager);

    // The Run/Stop/Rerun cluster and the configuration picker, on a
    // full-width strip under the menu bar — the mockup's `.toolbar` — rather
    // than inside the Run Console dock, where they were only visible once
    // that dock was opened. The Run Console panel still forwards the global
    // `run.*` shortcuts to it.
    auto *runToolbar = new RunToolbar(runService, buildService, debugService, window);
    QToolBar *toolBar = window->addToolBar(QObject::tr("Run"));
    toolBar->setObjectName(QStringLiteral("runToolBar"));
    toolBar->setMovable(false);
    toolBar->setFloatable(false);
    toolBar->addWidget(runToolbar);
    // The main window's own right-click menu only lists toolbars to hide,
    // and hiding the only one is not a feature.
    window->setContextMenuPolicy(Qt::PreventContextMenu);

    // The editor area is a QSplitter tree of tab groups (see EditorTabs) so
    // a tab can be split off into a second pane; ADS still sees the whole
    // tree as this one dock widget, leaving D4's dock save/restore alone.
    auto *editorRoot = new QSplitter(Qt::Horizontal);
    auto *editorDock = new ads::CDockWidget(dockManager, QObject::tr("Editor"));
    editorDock->setWidget(editorRoot);
    // The editor is ADS's *central* dock widget, not an ordinary center-area
    // one: a central widget absorbs the leftover space, so the side and
    // bottom panels keep their size hints instead of splitting the window
    // into equal shares and squeezing the editor down to nothing.
    auto *editorArea = dockManager->setCentralWidget(editorDock);
    // dockStyleSheet() paints the editor column on `surface` and every side
    // panel on `surface2`, the way the mockup's `.editor-col` and `.sidebar`
    // differ; ADS exposes no selector for "the central area", so this
    // property is the hook.
    editorArea->setProperty("centralArea", true);
    editorArea->style()->unpolish(editorArea);
    editorArea->style()->polish(editorArea);

    const ProjectTreeDock projectTreeDock = createProjectTreeDock(dockManager, editorArea, treeModel, docks);
    QTreeView *treeView = projectTreeDock.view;
    QAction *projectTreeLocateAction = projectTreeDock.locateAction;

    auto *editorTabs = new EditorTabs(docManager, languageService, editorRoot, window);

    // Task H: bottom dock panel, matching where JetBrains/VS-style IDEs
    // dock their Find in Files results. Reuses the one EditorTabs instance
    // above (its openFileAtLine) to open a match rather than a second,
    // parallel "open file" path.
    auto openAt = [editorTabs](const QString &path, int line, int column) {
        editorTabs->openFileAtLine(path, line, column);
    };
    auto *searchResultsPanel = new SearchResultsPanel(searchModel, openAt, dockManager);
    auto *searchResultsDock = new ads::CDockWidget(dockManager, QObject::tr("Search Results"));
    searchResultsDock->setWidget(searchResultsPanel);
    // First bottom panel: it creates the bottom dock area; every panel after
    // it is added *into* that area (CenterDockWidgetArea) so they become tabs
    // rather than each stacking one more split between editor and status bar.
    auto *bottomArea = docks->registerDock(QStringLiteral("searchResults"), searchResultsDock,
                                           ads::BottomDockWidgetArea, editorArea);

    // Task J: bottom dock panel, tabbed alongside Find in Files — same
    // "list of locations" shape, just fed by a symbol name instead of typed
    // free text. Built before ClassViewPanel so its "Find Usages" callback
    // (below) can capture this panel and its dock widget.
    auto *findUsagesPanel = new FindUsagesPanel(searchModel, editorTabs, dockManager);
    auto *findUsagesDock = new ads::CDockWidget(dockManager, QObject::tr("Find Usages"));
    findUsagesDock->setWidget(findUsagesPanel);
    docks->registerDock(QStringLiteral("findUsages"), findUsagesDock, ads::CenterDockWidgetArea,
                        bottomArea);

    // C11-followup: tabbed alongside Find Usages — same "asked about a
    // symbol, results stream into a dock" shape, just a lazy tree instead
    // of a flat list (a hierarchy walk can go arbitrarily deep, a location
    // list cannot).
    auto *hierarchyPanel = new HierarchyPanel(languageService, editorTabs, dockManager);
    auto *hierarchyDock = new ads::CDockWidget(dockManager, QObject::tr("Hierarchy"));
    hierarchyDock->setWidget(hierarchyPanel);
    docks->registerDock(QStringLiteral("hierarchy"), hierarchyDock, ads::CenterDockWidgetArea,
                        bottomArea);

    // Task D: right-side dock panel, matching where JetBrains-style IDEs
    // dock their Class/Structure View. Reuses the one EditorTabs instance
    // above (its jumpToByteOffset) rather than a second navigation path.
    // Task J extends it with a "Find Usages" context-menu action that
    // raises the Find Usages dock and runs the query there.
    auto *classViewPanel = new ClassViewPanel(
      docManager, searchModel, editorTabs,
      [docks, findUsagesPanel](const QString &name) {
          docks->show(QStringLiteral("findUsages"));
          findUsagesPanel->findUsages(name);
      },
      dockManager);
    auto *classViewDock = new ads::CDockWidget(dockManager, QObject::tr("Class View"));
    classViewDock->setWidget(classViewPanel);
    auto *rightArea = docks->registerDock(QStringLiteral("classView"), classViewDock,
                                          ads::RightDockWidgetArea, editorArea);

    // AC16/AC17: the AI Chat dock, tabbed into the right-hand area
    // (CenterDockWidgetArea) exactly as Find Usages and Problems tab into
    // the bottom one — it sits beside the code it is talking about rather
    // than squeezing a third split into the window. Its callbacks (the
    // current buffer text, and applying a code block) are set in
    // buildMainWindow, which is where the editor lives.
    auto *aiChatPanel = new AiChatPanel(aiChat, searchModel, dockManager);
    auto *aiChatDock = new ads::CDockWidget(dockManager, QObject::tr("AI Chat"));
    aiChatDock->setWidget(aiChatPanel);
    docks->registerDock(QStringLiteral("aiChat"), aiChatDock, ads::CenterDockWidgetArea, rightArea);

    // ADR-0033: the Preview dock, tabbed beside AI Chat and Class View —
    // it sits next to the document it is showing rather than squeezing a
    // third split into the window, the same reasoning the AI Chat comment
    // above gives. Starts hidden; `view.preview` and opening a previewable
    // file both reveal it (see below).
    auto *previewPanel = new MarkdownPreviewPanel(previewProvider, dockManager);
    auto *previewDock = new ads::CDockWidget(dockManager, QObject::tr("Preview"));
    previewDock->setWidget(previewPanel);
    docks->registerDock(QStringLiteral("preview"), previewDock, ads::CenterDockWidgetArea, rightArea);
    docks->hide(QStringLiteral("preview"));

    // F3-17/18: both start hidden; vcs_menu.cpp reveals each in turn.
    auto *changesPanel = new ChangesPanel(
      vcsService, [editorTabs](const QString &path) { editorTabs->showDiffForPath(path); },
      dockManager);
    auto *changesDock = new ads::CDockWidget(dockManager, QObject::tr("Changes"));
    changesDock->setWidget(changesPanel);
    docks->registerDock(QStringLiteral("changes"), changesDock, ads::CenterDockWidgetArea, rightArea);
    docks->hide(QStringLiteral("changes"));
    auto *fileHistoryPanel = new FileHistoryPanel(
      vcsService,
      [editorTabs](const QString &path, const QString &leftRevision, const QString &leftLabel,
                    const QString &rightRevision, const QString &rightLabel) {
          editorTabs->openCompareRevisions(path, leftRevision, leftLabel, rightRevision,
                                             rightLabel);
      },
      dockManager);
    auto *fileHistoryDock = new ads::CDockWidget(dockManager, QObject::tr("File History"));
    fileHistoryDock->setWidget(fileHistoryPanel);
    docks->registerDock(QStringLiteral("fileHistory"), fileHistoryDock, ads::CenterDockWidgetArea,
                        bottomArea);
    docks->hide(QStringLiteral("fileHistory"));

    // Search Everywhere: a transient popup parented to the top-level window
    // (not the dock manager) since it's a floating overlay, not a dock
    // widget. It hands a query off to the Search Results dock on Ctrl+Enter,
    // which is why it is built after that panel. The action map it triggers
    // commands through is filled later in buildMainWindow, so it takes a
    // pointer to the map rather than a copy.
    auto *searchEverywhereDialog =
      new SearchEverywhereDialog(searchModel, openAt, searchResultsPanel, window);

    // Task F3, multi-session since F4-14: bottom dock panel, tabbed alongside
    // Find in Files. Each tab's TerminalWidget starts its own PTY once shown.
    // Task L2: the Problems panel, tabbed into the same bottom area as Find
    // in Files and Find Usages — the same "list of locations" shape, fed by
    // the language servers instead of a query.
    auto *problemsPanel = new ProblemsPanel(languageService, buildService, openAt, dockManager);
    auto *problemsDock = new ads::CDockWidget(dockManager, QObject::tr("Problems"));
    problemsDock->setWidget(problemsPanel);
    docks->registerDock(QStringLiteral("problems"), problemsDock, ads::CenterDockWidgetArea,
                        bottomArea);
    // Hidden until there is something to show (or the View menu asks): it
    // opens itself once per session, the first time a diagnostic arrives.
    // A plain toggleView rather than docks->show() — this auto-open must not
    // raise the tab over whatever the user is already looking at.
    docks->hide(QStringLiteral("problems"));
    problemsPanel->setFirstDiagnosticCallback([docks]() {
        docks->dock(QStringLiteral("problems"))->toggleView(true);
    });
    // The squiggles and the panel read the same store, so one signal drives
    // both.
    QObject::connect(languageService, &LanguageService::diagnosticsChanged, editorTabs,
                      [editorTabs]() { editorTabs->applyDiagnostics(); });

    auto *terminalPanel =
      new TerminalSessionsPanel(terminalSupervisor, appSettings, openAt, dockManager);
    auto *terminalDock = new ads::CDockWidget(dockManager, QObject::tr("Terminal"));
    terminalDock->setWidget(terminalPanel);
    docks->registerDock(QStringLiteral("terminal"), terminalDock, ads::CenterDockWidgetArea,
                        bottomArea);
    auto *runConsolePanel = buildRunConsoleDock(dockManager, docks, bottomArea, runToolbar, openAt);
    auto *buildPanel = buildBuildDock(dockManager, docks, bottomArea, buildService);
    auto *debugPanel = buildDebugDock(dockManager, docks, bottomArea, debugService);

    // Class View tracks whatever tab is current: refresh on open, on
    // switch, and whenever a tab becomes clean. `tabModifiedChanged`
    // firing with `modified == false` doubles as "just saved" — there is
    // no separate "save completed" signal, and this one already fires
    // exactly when EditorTabs::saveTab succeeds (it forwards
    // QTextDocument::modificationChanged, which setModified(false) there
    // triggers). It also fires on initial load and on undo-to-clean, both
    // harmless extra refreshes of the same content.
    QObject::connect(docManager, &DocumentManager::tabOpened, classViewPanel,
                      [classViewPanel, editorTabs](quint64, const QString &) {
                          classViewPanel->refresh(editorTabs->currentTabId());
                      });
    DockRegistry *previewDocks = docks;
    editorTabs->setPreviewProvider(previewProvider);
    previewPanel->setOpenFileHandler([editorTabs](const QString &path, int line) {
        if (line >= 0) {
            editorTabs->openFileAtLine(path, line, 0);
        } else {
            editorTabs->openFile(path);
        }
    });
    previewPanel->setStatusHandler([window](const QString &message) {
        window->statusBar()->showMessage(message, 4000);
    });

    editorTabs->setActiveTabChangedCallback(
      [classViewPanel, editorTabs, problemsPanel, fileHistoryPanel, previewPanel,
       previewDocks, previewProvider, projectTreeLocateAction]() {
          classViewPanel->refresh(editorTabs->currentTabId());
          // The current file's group sorts to the top of the Problems panel.
          problemsPanel->setCurrentFile(editorTabs->currentPath());
          // F3-18: keep File History pinned to the current tab, but only
          // while its dock is actually shown — otherwise every tab switch
          // walks the whole ancestry of a file nobody is looking at.
          // Opening the dock (View menu) refreshes it itself.
          if (!previewDocks->isClosed(QStringLiteral("fileHistory"))) {
              fileHistoryPanel->setCurrentFile(editorTabs->currentPath());
          }
          editorTabs->setAnnotateEnabled(editorTabs->annotateEnabled());
          // Locate-in-tree only makes sense while a tab is open.
          projectTreeLocateAction->setEnabled(!editorTabs->currentPath().isEmpty());

          // ADR-0033: follow the active tab, and reveal the dock the first
          // time a previewable file is opened — `toggleView(true)` rather
          // than `previewDocks->show()`, so switching to a markdown tab
          // never steals focus from whatever the user is already looking
          // at, the same restraint the Problems panel's first-diagnostic
          // auto-open already uses.
          const quint64 tabId = editorTabs->currentTabId();
          const QString path = editorTabs->currentPath();
          // While a tab renders itself in place (view mode,
          // editor_tabs_preview.cpp), the dock stands down: `PreviewProvider`
          // keys one render per tab id, so two panels asking for the same tab
          // would rasterise its diagrams at whichever panel's width won the
          // race, pay for two Mermaid layouts per keystroke, and emit two
          // `preview_ready` markers for one revision. Tab id 0 is the "no tab"
          // sentinel the panel's own filter already rejects everything for.
          const bool renderedInTab = editorTabs->previewModeActive(tabId);
          if (renderedInTab) {
              previewPanel->setCurrentTab(0, QString(), QString());
          } else {
              previewPanel->setCurrentTab(tabId, path, editorTabs->currentContent());
          }
          // Auto-opens once per session, the first time a previewable file
          // becomes the active tab — `toggleView(true)`, not `show()`, so
          // it never raises the dock over whatever the user is already
          // looking at. Exactly the Problems panel's own "opens itself
          // once, on the first diagnostic" restraint
          // (`setFirstDiagnosticCallback` above), applied to "the first
          // previewable file" instead.
          static bool everShown = false;
          if (!everShown && !renderedInTab && !path.isEmpty() && previewProvider->hasPreview(path)) {
              everShown = true;
              previewDocks->dock(QStringLiteral("preview"))->toggleView(true);
          }
      });

    // ADR-0033's own debounce (editor_tabs_lsp.cpp's `previewTimer`, 300ms,
    // separate from the LSP one): the *current* tab's content changed.
    editorTabs->setPreviewChangedCallback([editorTabs, previewPanel](quint64 tabId) {
        // One cadence for both surfaces: whichever of the two is showing this
        // tab gets the settled content, and the other was already stood down.
        if (editorTabs->previewModeActive(tabId)) {
            return;
        }
        previewPanel->setCurrentTab(tabId, editorTabs->currentPath(), editorTabs->currentContent());
    });
    QObject::connect(docManager, &DocumentManager::tabModifiedChanged, classViewPanel,
                      [classViewPanel, editorTabs](quint64 tabId, bool modified) {
                          if (!modified && tabId == editorTabs->currentTabId()) {
                              classViewPanel->refresh(tabId);
                          }
                      });

    // Open the project's text index off the same project-open lifecycle
    // event the tree/watcher already use (no second, parallel hook). Opening
    // reuses whatever is already on disk and re-reads only what changed, so
    // a warm start costs a walk rather than a full index build.
    QObject::connect(treeModel,
                      &ProjectTreeModel::projectOpened,
                      searchModel,
                      [searchModel](const QString &rootPath) { searchModel->openIndex(rootPath); });

    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, treeModel,
                      [](const QString &rootPath) {
                          e2eMark(QStringLiteral("{\"ev\":\"project_opened\",\"root\":%1}")
                                    .arg(e2eJson(rootPath)));
                      });

    // Same project-open lifecycle event for the language servers: the root is
    // what `initialize` reports, and re-opening a project must not leave the
    // previous one's servers running.
    QObject::connect(treeModel,
                      &ProjectTreeModel::projectOpened,
                      languageService,
                      [languageService](const QString &rootPath) {
                          languageService->openProject(rootPath);
                      });

    // Initial bottom-panel height: without it the area is sized from the
    // terminal's tiny size hint (~60px), which is unusable for every panel
    // tabbed there. Overridden by restoreState() below once a layout has
    // been saved.
    dockManager->setSplitterSizes(bottomArea, {520, 200});
    // Likewise the right-hand column, which shares a splitter with the
    // editor only (the tree is one level up): sized from its panels' hints
    // it comes up narrower than the Changes panel's button row.
    dockManager->setSplitterSizes(rightArea, {680, 360});

    // D4: restored after both dock widgets exist for this layout to apply
    // to (ADS matches saved widgets by their title/object name). Empty
    // means nothing was ever saved — first launch, or window_state predates
    // D4 — so the layout built above (tree left of editor) stands as-is.
    const QString savedState = appSettings->windowState();
    if (!savedState.isEmpty()) {
        dockManager->restoreState(QByteArray::fromBase64(savedState.toLatin1()));
    }

    // Filesystem-watcher plumbing: ProjectTreeModel's watcher-driven signal
    // already carries the changed path and already runs on the Qt thread
    // (queued there via CxxQtThread), so relaying it to DocumentManager is a
    // plain same-thread signal/slot connection — no further cross-thread
    // hop. The session decides whether the change warrants a prompt.
    QObject::connect(treeModel,
                      &ProjectTreeModel::filesChangedExternally,
                      docManager,
                      [docManager](const QString &path) { docManager->checkExternalChange(path); });

    // C5: relay the same watcher events, plus their LSP `FileChangeType`,
    // to the language service. Debouncing and per-server filtering are
    // `LanguageService::watchedFileChanged`'s job (Rust); this is a plain
    // signal relay, same as the connection above.
    QObject::connect(treeModel,
                      &ProjectTreeModel::watchedFileChanged,
                      languageService,
                      [languageService](const QString &path, qint32 kind) {
                          languageService->watchedFileChanged(path, kind);
                      });

    // Keep the search index in step with the disk. Paths are coalesced over a
    // short window because a single save can produce several watcher events,
    // and re-indexing a file is far more expensive than remembering its name.
    auto *dirtyPaths = new QSet<QString>();
    auto *reindexTimer = new QTimer(window);
    reindexTimer->setSingleShot(true);
    reindexTimer->setInterval(300);
    QObject::connect(reindexTimer, &QTimer::timeout, searchModel, [searchModel, dirtyPaths]() {
        // The whole window goes over as one call: whether a path is
        // re-indexed or dropped is decided in Rust from whether it still
        // exists, and the batch shares a single commit.
        searchModel->syncIndexedFiles(QStringList(dirtyPaths->values()));
        dirtyPaths->clear();
    });
    QObject::connect(treeModel,
                      &ProjectTreeModel::filesChangedExternally,
                      searchModel,
                      [dirtyPaths, reindexTimer](const QString &path) {
                          dirtyPaths->insert(path);
                          reindexTimer->start();
                      });

    // Every file the user opens feeds Search Everywhere's Recent tier.
    QObject::connect(docManager,
                      &DocumentManager::tabOpened,
                      searchModel,
                      [searchModel, docManager](quint64 tabId, const QString &) {
                          const QString path = docManager->tabPath(tabId);
                          if (!path.isEmpty()) {
                              searchModel->noteRecentFile(path);
                          }
                      });

    QObject::connect(docManager,
                      &DocumentManager::externalChangeDetected,
                      editorTabs,
                      [editorTabs](quint64 tabId, const QString &path) {
                          editorTabs->handleExternalChange(tabId, path);
                      });

    // MCP's edit_buffer tool (M5) changed a tab's content (M3's listener
    // thread relayed it here via CxxQtThread::queue already) — reflect it
    // in the widget.
    QObject::connect(docManager,
                      &DocumentManager::bufferEditedExternally,
                      editorTabs,
                      [editorTabs](quint64 tabId, const QString &content) {
                          editorTabs->onBufferEditedExternally(tabId, content);
                      });

    // A tree-driven rename/delete retitled an open tab (US-2b). A
    // refactoring's own resource operations (F2-3) retitle one too, wired in
    // RefactorController's constructor since it already owns both ends.
    QObject::connect(treeModel,
                      &ProjectTreeModel::tabTitleChanged,
                      editorTabs,
                      [editorTabs](quint64 tabId, const QString &title) {
                          editorTabs->onTabTitleChanged(tabId, title);
                      });

    wireProjectTree(treeView,
                    projectTreeDock.locateAction,
                    treeModel,
                    ProjectTreeActions{window,
                                       aiChat,
                                       aiChatPanel,
                                       docks,
                                       [editorTabs](const QString &path) {
                                           editorTabs->openFile(path);
                                       },
                                       [editorTabs]() { return editorTabs->currentPath(); },
                                       [editorTabs](const QString &left, const QString &right) {
                                           editorTabs->openCompareFiles(left, right);
                                       }});

    return CentralWidgets{editorTabs,       dockManager,      docks,
                           treeView,         searchResultsPanel, classViewPanel,
                           terminalPanel,    findUsagesPanel,  hierarchyPanel,
                           searchEverywhereDialog,
                           problemsPanel,    aiChatPanel,      changesPanel,
                           fileHistoryPanel, runConsolePanel,  buildPanel,
                           debugPanel,       previewPanel};
}

// Menu structure per US-5 acceptance criteria. "Open Folder..." and the
// Edit/Save actions are wired to the tabbed editor area; the rest remain
// non-functional stubs for later tasks.
// `progress` is called once per startup stage (1-based, see
// SplashScreen::StageCount) so the splash can show what is taking time. The
// stages are the blocking steps below, in the order they already ran.
//
// `whenReady` is called exactly once, with the finished window, once startup
// has genuinely finished — which is no longer necessarily before this
// function returns (ADR-0037): reopening the last project walks its
// directory tree on a worker thread, so "restore the editor layout and show
// the window" waits for that walk's outcome (`projectOpened` or
// `projectOpenFailed`) instead of running synchronously inline the way it
// used to when `reopenLastProject()` blocked.
void buildMainWindow(AppSettings *appSettings,
                      const std::function<void(int, const QString &)> &progress,
                      const std::function<void(QMainWindow *)> &whenReady)
{
    progress(1, QObject::tr("Loading settings..."));

    auto *window = new IdeMainWindow();
    window->setWindowTitle(QStringLiteral("IDE"));

    // Created by run_app() before the splash so the persisted theme is known
    // early enough to paint it; adopted by the window here as before.
    appSettings->setParent(window);
    // Holds the Settings > Keymap page's draft between beginEdit() and
    // commit(); parented to the window so it outlives each dialog.
    auto *keymapEditor = new KeymapEditor(window);
    // The same arrangement for the three language-platform pages (T4, G3,
    // L6): each holds its page's state between the dialog's beginEdit() and
    // its commit/revert, and is parented to the window so it outlives the
    // dialog.
    auto *syntaxColorEditor = new SyntaxColorEditor(window);
    auto *languageCatalog = new LanguageCatalog(window);
    auto *languageServerEditor = new LanguageServerEditor(window);
    auto *editingEditor = new EditingEditor(window);
    // P7's Plugins page, the same arrangement again: it holds the rows of
    // the last scan between the dialog's refresh() calls.
    auto *pluginCatalog = new PluginCatalog(window);

    const FfiWindowGeometry savedGeometry = appSettings->windowGeometry();
    if (savedGeometry.width > 0 && savedGeometry.height > 0) {
        window->setGeometry(savedGeometry.x, savedGeometry.y,
                             static_cast<int>(savedGeometry.width),
                             static_cast<int>(savedGeometry.height));
    } else {
        window->resize(1280, 800);
    }

    progress(2, QObject::tr("Starting services..."));

    auto *treeModel = new ProjectTreeModel(window);
    auto *docManager = new DocumentManager(window);
    auto *searchModel = new SearchModel(window);
    // Task F3, multi-session since F4-14: one supervisor for every terminal
    // session the "Terminal" dock's tabs open (`RunService`'s N-consoles
    // shape, applied here). No shell spawns until a tab calls `newSession()`.
    auto *terminalSupervisor = new TerminalSupervisor(window);
    // Task L2: one language-server adapter per window, alongside the other
    // per-window QObjects. It launches nothing until a project is opened and
    // a file of a configured language is opened in it.
    auto *languageService = new LanguageService(window);
    // F3-12/F3-16: one Git adapter per window, discovering nothing until a
    // project is opened, same as LanguageService.
    auto *vcsService = new VcsService(window);
    auto *runService = new RunService(window);
    // B1-6: one build adapter per window, like the others; it runs nothing
    // until asked and knows no project until one is open.
    auto *buildService = new BuildService(window);
    // D3-1: one debug adapter per window. It owns the breakpoints, which
    // exist with no session at all, so it is built before any project opens
    // and told to load them when one does.
    auto *debugService = new DebugService(window);
    auto *runConfigEditor = new RunConfigEditor(window);
    // ADR-0021: one AI chat session per window, alongside the other
    // per-window QObjects, plus the Settings > AI Providers draft — the same
    // arrangement KeymapEditor and LanguageServerEditor use, parented to the
    // window so it outlives each dialog. Both read the persisted AI settings
    // on the way up, so both are built after appSettings.
    auto *aiChat = new AiChat(window);
    auto *aiProviderEditor = new AiProviderEditor(window);
    // ADR-0033: one Preview provider per window, alongside the other
    // per-window QObjects.
    auto *previewProvider = new PreviewProvider(window);
    // One MCP server per process, brought up right after the shared
    // DocumentManager exists — the listener thread it spawns dispatches
    // every EditorCommand back onto this same QObject's Qt thread. Whether
    // it listens at all, and on which port, is the Rust side's decision
    // from settings; this call only says "make it match".
    auto mcpStatus = wireMcpStatus(docManager, window);
    docManager->applyMcpSettings();
    progress(3, QObject::tr("Building workspace..."));
    const CentralWidgets central =
      buildCentralWidget(window, treeModel, docManager, appSettings, searchModel,
                          terminalSupervisor, languageService, aiChat, vcsService, runService,
                          buildService, debugService, previewProvider);
    EditorTabs *editorTabs = central.editorTabs;
    wireVcsService(vcsService, treeModel, editorTabs); // F3-12a/F3-16
    wireRunService(runService, editorTabs);             // R1-7
    wireDebugService(debugService, editorTabs);         // D2-5
    // Breakpoints live under the project's `.ide/local/`, so they can only
    // be read once a project is open — the same lifecycle hook run
    // configuration detection uses.
    QObject::connect(treeModel, &ProjectTreeModel::projectOpened, debugService,
                      [debugService](const QString &) { debugService->loadBreakpoints(); });

    // Every path that shows the AI chat goes through here — see
    // DockRegistry::show (dock_layout.h) for why "re-add if homeless" runs
    // on every call rather than only at startup.
    const auto showAiChat = [central]() { central.docks->show(QStringLiteral("aiChat")); };

    window->setEditorTabs(editorTabs);
    window->setAppSettings(appSettings);
    window->setDockManager(central.dockManager);
    window->setDocumentManager(docManager);

    // S2: applied before reopenLastProject() (below) opens any tabs, so
    // every tab — including ones opened at startup — starts with the
    // persisted font/colors rather than the QPlainTextEdit default.
    const FfiEditorFont savedFont = appSettings->editorFont();
    editorTabs->setEditorFont(QFont(savedFont.family, static_cast<int>(savedFont.size)));
    const FfiEditorColors savedColors = appSettings->editorColors();
    editorTabs->setEditorColors(savedColors.background, savedColors.foreground,
                                 savedColors.current_line);
    const FfiWhitespaceOptions savedWhitespace = appSettings->whitespaceOptions();
    editorTabs->setWhitespaceOptions(WhitespaceOptions{
      savedWhitespace.enabled, savedWhitespace.leading, savedWhitespace.inner,
      savedWhitespace.trailing, savedWhitespace.eol_markers});

    wireAiChatToEditor(window, aiChat, central.aiChatPanel, editorTabs, searchModel);

    const UiFontTargets uiFontTargets =
      buildStatusBar(window, appSettings, languageService, searchModel, vcsService, editorTabs,
                     central.projectTree, central.docks, central.problemsPanel, treeModel);

    // Every menu action is registered under a stable id from
    // app_config::ACTIONS and takes its shortcut from the persisted keymap,
    // so Settings > Keymap can rebind any of them (nothing here hardcodes a
    // QKeySequence any more).
    // Boxed so the Preferences lambda (which runs long after this function
    // returns, and needs the *complete* registry including actions added
    // below it) shares one instance instead of capturing a dangling
    // reference — the same std::make_shared trick the settings dialog's
    // colour pickers use.
    progress(4, QObject::tr("Preparing menus..."));
    auto actions = std::make_shared<QHash<QString, QAction *>>();

    // The terminal's Copy/Paste are per-tab QActions (widget-scoped, so
    // Ctrl+C keeps reaching the shell); Settings > Keymap lists them from
    // `app_config::ACTIONS` regardless, but a single QAction* here would
    // dangle once its tab closed, so a rebind reaches every open tab via
    // `SettingsContext::terminalPanel`'s `reapplyKeymap()` instead. `newSession`
    // is one QAction for the whole panel's lifetime, so it can sit here.
    actions->insert(QStringLiteral("terminal.newSession"), central.terminalPanel->newSessionAction());
    actions->insert(QStringLiteral("terminal.selectShell"),
                    central.terminalPanel->selectShellAction());

    QMenu *fileMenu = window->menuBar()->addMenu(QObject::tr("&File"));
    QAction *openFolderAction = registerAction(fileMenu, QStringLiteral("file.openFolder"),
                                                QObject::tr("Open Folder..."), appSettings, *actions);
    QMenu *recentProjectsMenu = fileMenu->addMenu(QObject::tr("Recent Projects"));
    populateRecentProjectsMenu(recentProjectsMenu, appSettings, treeModel, window);
    fileMenu->addSeparator();
    QAction *saveAction = registerAction(fileMenu, QStringLiteral("file.save"),
                                          QObject::tr("Save"), appSettings, *actions);
    QAction *saveAsAction = registerAction(fileMenu, QStringLiteral("file.saveAs"),
                                            QObject::tr("Save As..."), appSettings, *actions);
    fileMenu->addSeparator();
    QAction *preferencesAction = registerAction(fileMenu, QStringLiteral("file.preferences"),
                                                 QObject::tr("Preferences..."), appSettings, *actions);
    QAction *projectSettingsAction =
      registerAction(fileMenu, QStringLiteral("file.projectSettings"),
                     QObject::tr("Project Settings..."), appSettings, *actions);
    fileMenu->addSeparator();
    QAction *exitAction = registerAction(fileMenu, QStringLiteral("file.exit"),
                                          QObject::tr("Exit"), appSettings, *actions);

    QObject::connect(openFolderAction, &QAction::triggered, window,
                      [treeModel, window, recentProjectsMenu, appSettings]() {
                          const QString dir = QFileDialog::getExistingDirectory(
                            window, QObject::tr("Open Folder"), QString(),
                            QFileDialog::ShowDirsOnly);
                          if (dir.isEmpty()) {
                              return;
                          }
                          openProjectAndRefreshRecents(treeModel, window, recentProjectsMenu,
                                                        appSettings, dir);
                      });

    QObject::connect(exitAction, &QAction::triggered, window, [window]() { window->close(); });

    QObject::connect(saveAction, &QAction::triggered, window, [editorTabs]() {
        editorTabs->saveCurrentTab();
    });

    QObject::connect(saveAsAction, &QAction::triggered, window, [editorTabs]() {
        editorTabs->saveCurrentTabAs();
    });

    // Built once, outside the lambda, and captured whole: fourteen separate
    // captures is what the parameter object exists to replace (see
    // SettingsContext). Every member is a pointer or a handle that outlives
    // the window, so a by-value capture holds nothing that can dangle.
    const SettingsContext settingsContext{
      appSettings,
      editorTabs,
      keymapEditor,
      actions,
      docManager,
      mcpStatus,
      syntaxColorEditor,
      languageCatalog,
      languageServerEditor,
      editingEditor,
      languageService,
      aiProviderEditor,
      aiChat,
      pluginCatalog,
      uiFontTargets,
      central.terminalPanel,
    };
    QObject::connect(preferencesAction, &QAction::triggered, window,
                      [window, settingsContext, appSettings]() {
                          appSettings->setSettingsScope(QStringLiteral("global"));
                          showSettingsDialog(window, settingsContext);
                      });
    // The same dialog, opened on the project's own layer (ADR-0022). Two
    // entry points rather than one because "configure this project" and
    // "configure my editor" are different intentions, and the scope selector
    // inside the dialog is how you get from one to the other afterwards.
    QObject::connect(projectSettingsAction, &QAction::triggered, window,
                      [window, settingsContext, appSettings]() {
                          appSettings->setSettingsScope(QStringLiteral("project"));
                          showSettingsDialog(window, settingsContext);
                      });

    QMenu *editMenu = window->menuBar()->addMenu(QObject::tr("&Edit"));
    QAction *undoAction = registerAction(editMenu, QStringLiteral("edit.undo"),
                                          QObject::tr("Undo"), appSettings, *actions);
    QAction *redoAction = registerAction(editMenu, QStringLiteral("edit.redo"),
                                          QObject::tr("Redo"), appSettings, *actions);
    editMenu->addSeparator();
    QAction *cutAction = registerAction(editMenu, QStringLiteral("edit.cut"),
                                         QObject::tr("Cut"), appSettings, *actions);
    QAction *copyAction = registerAction(editMenu, QStringLiteral("edit.copy"),
                                          QObject::tr("Copy"), appSettings, *actions);
    QAction *pasteAction = registerAction(editMenu, QStringLiteral("edit.paste"),
                                           QObject::tr("Paste"), appSettings, *actions);
    editMenu->addSeparator();
    QAction *findAction = registerAction(editMenu, QStringLiteral("edit.find"),
                                         QObject::tr("Find..."), appSettings, *actions);
    QObject::connect(findAction, &QAction::triggered, window,
                     [editorTabs]() { editorTabs->showFindBar(); });
    QAction *replaceAction = registerAction(editMenu, QStringLiteral("edit.replace"),
                                            QObject::tr("Replace..."), appSettings, *actions);
    QObject::connect(replaceAction, &QAction::triggered, window,
                     [editorTabs]() { editorTabs->showReplaceBar(); });
    QAction *findNextAction = registerAction(editMenu, QStringLiteral("edit.findNext"),
                                             QObject::tr("Find Next"), appSettings, *actions);
    QObject::connect(findNextAction, &QAction::triggered, window,
                     [editorTabs]() { editorTabs->findNext(); });
    QAction *findPreviousAction = registerAction(editMenu, QStringLiteral("edit.findPrevious"),
                                                 QObject::tr("Find Previous"), appSettings,
                                                 *actions);
    QObject::connect(findPreviousAction, &QAction::triggered, window,
                     [editorTabs]() { editorTabs->findPrevious(); });
    QAction *findInFilesAction = registerAction(editMenu, QStringLiteral("edit.findInFiles"),
                                                 QObject::tr("Find in Files..."), appSettings,
                                                 *actions);

    // F1-16: multi-caret, comment toggling, the line operations,
    // expand/shrink selection and the bracket jump. Its own translation
    // unit; every entry routes through EditorTabs to EditorOps.
    buildEditingActions(editMenu, window, appSettings, *actions, editorTabs);

    // RF12: the hover signature fallback. `lsp_core::hover_outcome` decides
    // whether the server answered; this only starts the index leg when it
    // says no, and shows whatever comes back the same way a server's hover
    // is shown.
    editorTabs->setHoverFallbackCallback([editorTabs, searchModel]() {
        const QString path = editorTabs->currentPath();
        if (path.isEmpty()) {
            return;
        }
        searchModel->hoverSignature(path,
                                     editorTabs->currentContent(),
                                     editorTabs->byteOffsetAt(editorTabs->hoverPosition()));
    });
    editorTabs->setHoverCanceledCallback(
      [searchModel]() { searchModel->cancelHoverSignature(); });
    QObject::connect(languageService, &LanguageService::hoverFallback, window,
                      [editorTabs]() { editorTabs->hoverFallback(); });
    QObject::connect(searchModel, &SearchModel::hoverSignatureReady, window,
                      [](const QString &html) { QToolTip::showText(QCursor::pos(), html); });

    // RF11: the Refactor menu. Every entry routes through the one
    // RefactorController, so there is a single place that turns a server's
    // answer into an edit.
    auto *refactorer = new RefactorController(languageService, searchModel, editorTabs, window);
    QMenu *refactorMenu = window->menuBar()->addMenu(QObject::tr("Re&factor"));
    QAction *renameAction = registerAction(refactorMenu, QStringLiteral("refactor.rename"),
                                            QObject::tr("Rename..."), appSettings, *actions);
    QObject::connect(renameAction, &QAction::triggered, window,
                      [refactorer]() { refactorer->renameSymbol(); });

    refactorMenu->addSeparator();
    QAction *extractMethodAction =
      registerAction(refactorMenu, QStringLiteral("refactor.extractMethod"),
                      QObject::tr("Extract Method..."), appSettings, *actions);
    QObject::connect(extractMethodAction, &QAction::triggered, window, [refactorer]() {
        refactorer->extract(
          QStringLiteral("refactor.extract"),
          QObject::tr("The language server offers no method extraction for this selection."));
    });

    QAction *extractClassAction =
      registerAction(refactorMenu, QStringLiteral("refactor.extractClass"),
                      QObject::tr("Extract Class..."), appSettings, *actions);
    QObject::connect(extractClassAction, &QAction::triggered, window, [refactorer]() {
        refactorer->extract(
          QStringLiteral("refactor.extract.class"),
          QObject::tr("The language server offers no class extraction for this selection."));
    });

    QAction *refactorThisAction =
      registerAction(refactorMenu, QStringLiteral("refactor.refactorThis"),
                      QObject::tr("Refactor This..."), appSettings, *actions);
    QObject::connect(refactorThisAction, &QAction::triggered, window, [refactorer]() {
        refactorer->extract(QString(),
                            QObject::tr("The language server offers no refactorings here."));
    });

    refactorer->buildCodeActions(refactorMenu, appSettings, *actions);

    // The same gestures on the editor's right-click menu. The actions are
    // looked up by id rather than captured, so this does not depend on which
    // menus have been built yet — and because they are the *same* QActions,
    // their shortcuts show here and a rebinding in Settings > Keymap reaches
    // both places at once.
    editorTabs->setContextMenuCallback([actions](QMenu *menu) {
        const auto append = [menu, actions](const QString &id) {
            if (QAction *action = actions->value(id)) {
                menu->addAction(action);
            }
        };
        menu->addSeparator();
        append(QStringLiteral("navigate.goToDeclaration"));
        append(QStringLiteral("navigate.findUsages"));
        append(QStringLiteral("navigate.showCallHierarchy"));
        append(QStringLiteral("navigate.showTypeHierarchy"));
        menu->addSeparator();
        append(QStringLiteral("refactor.rename"));
        append(QStringLiteral("refactor.extractMethod"));
        append(QStringLiteral("refactor.extractClass"));
        append(QStringLiteral("refactor.refactorThis"));
        menu->addSeparator();
        append(QStringLiteral("ai.addSelection"));
        append(QStringLiteral("ai.addSelectionNewChat"));
    });

    QMenu *viewMenu = window->menuBar()->addMenu(QObject::tr("&View"));
    wireProjectTreeViewAction(viewMenu, central.docks, appSettings, *actions);
    QAction *classViewAction = registerAction(viewMenu, QStringLiteral("view.classView"),
                                               QObject::tr("Class View"), appSettings, *actions);
    QObject::connect(classViewAction, &QAction::triggered, window,
                      [central]() { central.docks->show(QStringLiteral("classView")); });
    // The AI panel's show-action belongs here with every other dock's, not
    // only on the AI menu: a user looking for a hidden panel opens View.
    QAction *aiChatViewAction = registerAction(viewMenu, QStringLiteral("view.aiChat"),
                                               QObject::tr("AI Chat"), appSettings, *actions);
    QObject::connect(aiChatViewAction, &QAction::triggered, window, [central, showAiChat]() {
        showAiChat();
        central.aiChatPanel->focusComposer();
    });
    QAction *previewViewAction = registerAction(viewMenu, QStringLiteral("view.preview"),
                                                QObject::tr("Preview"), appSettings, *actions);
    QObject::connect(previewViewAction, &QAction::triggered, window,
                      [central]() { central.docks->show(QStringLiteral("preview")); });
    wirePreviewModeAction(viewMenu, central.editorTabs, appSettings, *actions);
    QAction *problemsAction = registerAction(viewMenu, QStringLiteral("view.problems"),
                                             QObject::tr("Problems"), appSettings, *actions);
    QObject::connect(problemsAction, &QAction::triggered, window, [central]() {
        central.docks->show(QStringLiteral("problems"));
        central.problemsPanel->focusTree();
    });
    QAction *terminalAction = registerAction(viewMenu, QStringLiteral("view.terminal"),
                                             QObject::tr("Terminal"), appSettings, *actions);
    QObject::connect(terminalAction, &QAction::triggered, window, [central]() {
        central.docks->show(QStringLiteral("terminal"));
        central.terminalPanel->focusCurrent();
    });
    // Every entry point opens the same popup, just preselected on a
    // different tab — one search surface, several doors into it.
    QAction *searchEverywhereAction =
      registerAction(viewMenu, QStringLiteral("view.searchEverywhere"),
                     QObject::tr("Search Everywhere..."), appSettings, *actions);
    QObject::connect(searchEverywhereAction, &QAction::triggered, window, [central]() {
        central.searchEverywhereDialog->popup(SearchEverywhereDialog::Tier::All);
    });
    QAction *goToFileAction = registerAction(viewMenu, QStringLiteral("view.goToFile"),
                                             QObject::tr("Go to File..."), appSettings, *actions);
    QObject::connect(goToFileAction, &QAction::triggered, window, [central]() {
        central.searchEverywhereDialog->popup(SearchEverywhereDialog::Tier::Files);
    });
    QAction *findActionAction = registerAction(viewMenu, QStringLiteral("view.findAction"),
                                               QObject::tr("Find Action..."), appSettings, *actions);
    QObject::connect(findActionAction, &QAction::triggered, window, [central]() {
        central.searchEverywhereDialog->popup(SearchEverywhereDialog::Tier::Actions);
    });
    QAction *goToSymbolAction = registerAction(viewMenu, QStringLiteral("view.goToSymbol"),
                                               QObject::tr("Go to Symbol..."), appSettings, *actions);
    QObject::connect(goToSymbolAction, &QAction::triggered, window, [central]() {
        central.searchEverywhereDialog->popup(SearchEverywhereDialog::Tier::Symbols);
    });
    QAction *goToLineAction = registerAction(viewMenu, QStringLiteral("view.goToLine"),
                                             QObject::tr("Go to Line..."), appSettings, *actions);
    QObject::connect(goToLineAction, &QAction::triggered, window, [editorTabs]() {
        editorTabs->goToLine();
    });

    // F3-19: the VCS menu (vcs_menu.cpp).
    buildVcsMenu(window, vcsService, appSettings, *actions, editorTabs, central.docks,
                 central.fileHistoryPanel, viewMenu);
    buildRunMenu(window, runService, runConfigEditor, appSettings, *actions, central.docks,
                 central.runConsolePanel, treeModel, editorTabs, central.buildPanel,
                 viewMenu);
    buildBuildMenu(window, central.buildPanel, appSettings, *actions, central.docks, viewMenu);
    buildDebugMenu(window, debugService, central.debugPanel, central.runConsolePanel, editorTabs,
                    appSettings, *actions, central.docks, viewMenu);

    buildNavigateMenu(window, languageService, searchModel, editorTabs, appSettings, *actions,
                       central.docks, central.findUsagesPanel, central.hierarchyPanel);

    buildAiMenu(window, aiChat, editorTabs, appSettings, *actions, central.docks,
                central.aiChatPanel);

    QObject::connect(undoAction, &QAction::triggered, window, [editorTabs]() {
        if (auto *editor = editorTabs->currentEditor()) {
            editor->undo();
        }
    });
    QObject::connect(redoAction, &QAction::triggered, window, [editorTabs]() {
        if (auto *editor = editorTabs->currentEditor()) {
            editor->redo();
        }
    });
    QObject::connect(cutAction, &QAction::triggered, window, [editorTabs]() {
        if (auto *editor = editorTabs->currentEditor()) {
            editor->cut();
        }
    });
    QObject::connect(copyAction, &QAction::triggered, window, [editorTabs]() {
        if (auto *editor = editorTabs->currentEditor()) {
            editor->copy();
        }
    });
    QObject::connect(pasteAction, &QAction::triggered, window, [editorTabs]() {
        if (auto *editor = editorTabs->currentEditor()) {
            editor->paste();
        }
    });
    QObject::connect(findInFilesAction, &QAction::triggered, window, [central]() {
        central.docks->show(QStringLiteral("searchResults"));
        central.searchResultsPanel->focusQuery();
    });

    // The popup triggers commands through this registry, which only exists
    // once every menu above has been built.
    central.searchEverywhereDialog->setActions(actions.get());
    // JetBrains' double-Shift gesture, on top of the rebindable shortcut.
    window->setSearchEverywhereTrigger([central]() {
        central.searchEverywhereDialog->popup(SearchEverywhereDialog::Tier::All);
    });

    // Reopens the persisted editor split layout, then hands the finished
    // window to `whenReady` — shared tail for both branches below, run once
    // the project (if any) has actually settled, so restored files show up
    // under a live tree (files are addressed by absolute path and reopen
    // even if they sit outside the reopened project).
    auto finishStartup = [progress, editorTabs, appSettings, whenReady, window]() {
        progress(6, QObject::tr("Restoring editors..."));
        editorTabs->restoreLayout(appSettings->editorLayout());
        whenReady(window);
    };

    // US-1: relaunching the app reopens the last project automatically.
    // Reuses the same worker-thread path as "Open Folder..." (ADR-0037), so
    // the tree is live-refreshing from the moment it's populated rather than
    // blocking startup on the walk. `reopenLastProject()` returns whether a
    // reopen was even kicked off — false means nothing was ever persisted,
    // so no `projectOpened`/`projectOpenFailed` is ever coming and startup
    // must proceed on its own rather than wait forever.
    progress(5, QObject::tr("Restoring project..."));
    if (treeModel->reopenLastProject()) {
        // Parented to `window` so it cannot outlive it; deletes itself the
        // moment either outcome signal fires, since only the first of the
        // two ever arrives for a given reopen.
        auto *waiter = new QObject(window);
        QObject::connect(treeModel, &ProjectTreeModel::projectOpened, waiter,
                         [waiter, finishStartup]() {
                             finishStartup();
                             waiter->deleteLater();
                         });
        QObject::connect(treeModel, &ProjectTreeModel::projectOpenFailed, waiter,
                         [waiter, finishStartup](const FfiResult &) {
                             finishStartup();
                             waiter->deleteLater();
                         });
    } else {
        finishStartup();
    }
}

} // namespace

int run_app()
{
    int argc = 0;
    QApplication app(argc, nullptr);
    // Taskbar/alt-tab/window-decoration icon on Linux and macOS (Windows
    // gets its exe/taskbar icon from the embedded .ico via app/build.rs
    // instead — this QIcon is just its in-window icon there).
    app.setWindowIcon(QIcon(QStringLiteral(":/ui/icons/app_icon.png")));
    // Wraps whatever platform style Qt picked, intercepting only the tab
    // close-button icon (see TabCloseIconStyle above) — every QTabWidget in
    // the app picks it up with no per-call-site change.
    app.setStyle(new TabCloseIconStyle(app.style()));

    // Parentless for now: the splash needs the persisted theme before any
    // window exists, and buildMainWindow() adopts this object as soon as it
    // has one.
    auto *appSettings = new AppSettings(nullptr);
    // Applying the theme (T2) before anything is shown means neither the
    // splash nor the main window ever flashes an unstyled frame.
    // Inter before the theme: the sheet's metrics are polished against the
    // application font, and applyUiFontScale() scales whatever is installed.
    installInterfaceFont();
    applyTheme(appSettings->themeName());
    // The global half of the interface font scale, before the splash for the
    // same reason: no frame is ever painted at a size the user did not pick.
    // The menu bar's and project tree's own scales are applied in
    // buildMainWindow(), where those widgets exist.
    applyUiFontScale(static_cast<int>(appSettings->uiFontScales().ui));
    // Build the language registry from what the config directory holds and
    // which languages the user turned off, before the first file can be
    // opened — otherwise a disabled language would come back every restart.
    appSettings->reloadLanguages();

    SplashScreen splash(appSettings->themeName());
    splash.show();

    buildMainWindow(
      appSettings,
      [&splash](int step, const QString &text) { splash.setStage(step, text); },
      [&splash](QMainWindow *window) {
          window->show();
          applyNativeWindowChrome(window);
          // Closes the splash exactly when the main window is up and its
          // project has settled (opened, failed, or there was none to
          // reopen) — no timer, no gap. When a reopen was kicked off, this
          // callback fires from a `qt_thread.queue`d closure (ADR-0037),
          // which needs the event loop below to actually be pumping to be
          // delivered — but queuing it before `QApplication::exec()` starts
          // is still sound, since Qt holds a posted event queued rather
          // than dropping it, and delivers it the moment the loop begins.
          splash.finish(window);
          e2eMark("{\"ev\":\"main_window_shown\"}");
      });

    return QApplication::exec();
}

} // namespace ui_shell
