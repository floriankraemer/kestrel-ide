#pragma once

#include "code_editor.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QFont>
#include <QHash>
#include <QJsonObject>
#include <QList>
#include <QObject>
#include <QPair>
#include <QPoint>
#include <QString>
#include <QStringList>
#include <functional>

class QLabel;
class QMenu;
class QPlainTextEdit;
class QSplitter;
class QTabWidget;
class QTextDocument;
class QWidget;

namespace ui_shell {

class CodeEditor;
class FindBar;
class HexViewer;
class IntentionBulb;

// F3-12a/F3-16: joins a window's VcsService to the project-open lifecycle
// (mirroring how LanguageService::openProject is wired) and to EditorTabs's
// gutter. Free function, not inline in buildMainWindow, so that wiring
// doesn't grow the already-large main_window.cpp past its file-size
// ceiling.
class EditorTabs;
void wireVcsService(VcsService *vcsService, ProjectTreeModel *treeModel, EditorTabs *editorTabs);
// R1-7: gives EditorTabs the RunService its gutter Run icon asks and acts
// through (editor_tabs_run.cpp).
void wireRunService(RunService *runService, EditorTabs *editorTabs);
// D2-5/D3: gives EditorTabs the DebugService whose breakpoints its gutter
// shows and toggles (editor_tabs_debug.cpp).
void wireDebugService(DebugService *debugService, EditorTabs *editorTabs);

// app_core::TabKind's stable code for a binary tab (ADR-0020).
constexpr int kTabKindBinary = 1;
// app_core::TabKind's stable code for a read-only diff tab (F3-14).
constexpr int kTabKindDiff = 2;

// Humble view for the editor area (ADR-0002): owns the QTabWidget <->
// DocumentManager wiring, decides nothing. Tabs are identified by the
// session's stable TabId (ADR-0003); the TabId <-> page-index mapping lives
// here and only here, as a dynamic property on each page widget — an id
// never shifts when other tabs close, so there is no index lockstep to
// maintain and no parallel title list to keep in sync.
//
// The area is a QSplitter tree of tab groups (JetBrains-style splits): one
// group to start, more created by the tab context menu's Split Vertical /
// Split Horizontal, which *move* the clicked tab into the new group so a
// TabId still maps to exactly one editor widget. ADS still sees the whole
// tree as the single "Editor" dock widget, so D4's dock save/restore is
// untouched by splitting.
//
// One class, three translation units: the whole of it does not fit under the
// file-size gate's 1200-line ceiling for a .cpp, and defining members of a
// class across several sources is ordinary C++. editor_tabs_panes.cpp holds
// the QSplitter tree of tab groups and its save/restore; editor_tabs_lsp.cpp
// holds the language-server leg (the positions the protocol speaks, hover,
// completion, diagnostics, and the per-editor wiring that carries them);
// editor_tabs.cpp holds the rest — the tab surface itself.
class EditorTabs : public QObject
{
public:
    EditorTabs(DocumentManager *docManager, LanguageService *languageService, QSplitter *root,
                QWidget *window);

    // Class View follows whatever tab is current; EditorTabs has no
    // Q_OBJECT (no moc target) so it hands out a callback rather than a
    // signal, matching how ClassViewPanel already receives its "Find
    // Usages" hook.
    void setActiveTabChangedCallback(std::function<void()> callback);

    // Debounced (300ms) like `didChange`'s own timer in editor_tabs_lsp.cpp
    // — a document being typed is not a document worth re-rendering on
    // every keystroke. Fires only for the *current* tab's own edits, with
    // its id, so the Preview dock knows which tab's content just changed
    // without asking `currentTabId()` and hoping nothing switched in
    // between.
    void setPreviewChangedCallback(std::function<void(quint64 tabId)> callback);

    // N7: a Ctrl+Click inside any editor. Same callback shape as above.
    std::function<void(int)> declarationRequested_;
    std::function<void()> navigationChanged_;
    // RF12: the index leg of hover, wired by the window because SearchModel
    // lives there.
    std::function<void()> hoverFallback_;
    std::function<void()> hoverCanceled_;
    std::function<void(QMenu *)> contextMenu_;
    int hoverPosition_ = 0;

    // Opens `path`, or focuses its tab if already open (US-3). The session
    // decides what opens and as what kind of page (ADR-0020) — a binary file
    // opens a hex tab rather than failing; this only shows whatever error is
    // left over.
    void openFile(const QString &path);

    // Task H: open `path` (reusing openFile's own dialog/focus behavior
    // above) and move the caret to a Find-in-Files match. `line` is
    // 1-based; `column` is a byte offset within that line from
    // `index_core::SearchMatch`, converted to a UTF-16 column on the way in.
    //
    // N5: this and `jumpWithinCurrentTab` are the two functions every jump
    // in the app funnels through, so recording the pre-jump position here
    // is what gives Find in Files, Go to Symbol, Class View, Go to Line
    // and Go to Declaration their Back/Forward history at once.
    void openFileAtLine(const QString &path, int line, int column);

    // A jump that stays inside the tab already open, recorded the same way.
    // `column` is a byte offset, as in openFileAtLine.
    void jumpWithinCurrentTab(int line, int column);

    // N5: Navigate > Back / Forward. The session owns the stack and the
    // rules; this only carries the answer to the editor.
    void jumpBack() { applyHistoryLocation(docManager_->jumpBack()); }
    void jumpForward() { applyHistoryLocation(docManager_->jumpForward()); }

    bool canJumpBack() const { return docManager_->canJumpBack(); }
    bool canJumpForward() const { return docManager_->canJumpForward(); }

    // Lets the window re-enable its Back/Forward actions after a jump,
    // without EditorTabs needing to be a Q_OBJECT.
    void setNavigationChangedCallback(std::function<void()> callback);

    // N7/N2: the caret's position as a UTF-8 byte offset — what the index
    // speaks. Also the shared conversion for a Ctrl+Click, whose
    // document position arrives from CodeEditor.
    quint64 byteOffsetAt(int documentPosition) const;

    // L3/L4: the same document position as the line/character pair the
    // language server speaks.
    QPair<quint32, quint32> lspPositionAt(int documentPosition) const;

    // The caret, as a document position — what the refactoring gestures ask
    // about when there is no explicit selection.
    int caretPosition() const;

    // RF10: the editor's own revision counter, which is what
    // `lsp_core::EditGate` compares an arriving refactoring against. Zero
    // when no tab is open, which no live buffer ever reports.
    int documentRevision() const;

    // The selection, or the caret twice when there is none, as the protocol
    // line/character pairs a code-action request is made about.
    QPair<QPair<quint32, quint32>, QPair<quint32, quint32>> selectionRange() const;

    // Whether any open tab has unsaved changes. The name-based rename
    // refuses to run in that case, because the index it reads is on disk —
    // `index_core::plan_index_rename` owns that rule, this only answers the
    // question it asks.
    bool hasUnsavedChanges() const;

    // RF10: splice a refactoring's edits into the buffers that are open.
    //
    // One edit block per file, so one Ctrl+Z undoes the whole refactoring in
    // that file, and the edits are applied in the order Rust handed them
    // over — already sorted last-first, so each range still addresses the
    // text it was computed against. Nothing is decided here: which edits are
    // buffer edits at all was decided by `lsp_core::plan_edit`.
    void applyBufferEdits(const ::rust::Vec<FfiTextEdit> &edits);

    // F1-15: the same splice into one known editor, for the edits that are
    // about the buffer the user is typing in and therefore name no file.
    // Static because it touches nothing but the editor it is handed —
    // `FindBar` splices its replacements through it too (F0-18).
    static void applyEditsTo(QPlainTextEdit *editor, const ::rust::Vec<FfiTextEdit> &edits);

    // RF12: where the pointer last dwelled, so the index leg of hover can
    // be started from outside this class when the server declines.
    int hoverPosition() const { return hoverPosition_; }

    // Called when no server answered a hover, and when there was no server
    // to ask. Set by the window, which owns the SearchModel.
    void setHoverFallbackCallback(std::function<void()> callback);

    void setHoverCanceledCallback(std::function<void()> callback);

    // What the window wants added to an editor's right-click menu. Set once;
    // every editor opened afterwards picks it up, and so does every one
    // already open.
    void setContextMenuCallback(std::function<void(QMenu *)> callback);

    void hoverFallback();

    void hoverCanceled();

    // Save every tab with unsaved changes. The name-based rename needs this
    // because the index it reads is on disk; false if any save failed, in
    // which case the caller must not go ahead.
    bool saveAllModified();

    // Every file open in a tab. Used by the name-based rename to splice the
    // ones the user can see instead of rewriting them on disk.
    QStringList openPaths() const;

    // The editor showing `path`, or nullptr when it is not open.
    CodeEditor *editorForPath(const QString &path) const;

    // F1-15/F1-16: run one editing operation over the current editor's
    // carets and splice what comes back. `op` asks `editorOps_` for a
    // transaction; everything about what the operation means is decided
    // there, and this only applies the answer and repaints the carets.
    void runEditorOp(const std::function<::rust::Vec<FfiTextEdit>(quint64, const QString &)> &op);

    // The caret surface, for the operations that move carets without
    // editing (Ctrl+D, expand/shrink) and for the settings dialog's
    // commit.
    EditorOps *editorOps() const { return editorOps_; }

    // Re-read the carets Rust holds for this editor and show them: the
    // primary becomes the widget's own cursor, the rest are painted.
    void refreshCarets(CodeEditor *editor);

    // The caret-only operations (Ctrl+D, add caret above/below,
    // expand/shrink): run `op` against the current editor's tab and live
    // text, then repaint whatever carets it left behind. Does nothing when
    // no editor is current.
    void withCurrentEditor(const std::function<void(quint64, const QString &)> &op);

    // Ctrl+]: move the caret to the partner of the bracket it is on, or
    // leave it where it is when it is not on one. Which bracket answers
    // which is `edit_ops::brackets`' answer.
    void jumpToMatchingBracket();

    // F2-10: Alt+Return / code.showIntentions. Asks again right now,
    // bypassing the caret-settle debounce that drives the bulb — the user
    // asked explicitly — and opens the grouped popup as soon as the answer
    // lands, whether or not the bulb ends up shown for it.
    void showIntentionsNow();

    // F2-11: code.toggleInlayHints. Applies to every open editor
    // immediately (S2's live-apply convention — see setEditorFont) and to
    // every editor opened afterward.
    void setInlayHintsEnabled(bool enabled);
    bool inlayHintsEnabled() const { return inlayHintsEnabled_; }

    // code.collapseAll / code.expandAll: the current tab only, unlike
    // setInlayHintsEnabled above — folding is per-editor view state, not a
    // setting that should retroactively apply to every open tab.
    void collapseAllFolds();
    void expandAllFolds();

    // F2-11: Ctrl+P. Asks again with `explicit_request = true`, which is
    // what lets it work with the caret sitting still on an argument the
    // trigger character for was typed several keystrokes ago.
    void requestSignatureHelpNow();

    // F2-9's `organizeImports`, over whatever tab is current.
    // `documentRevision()`/`currentPath()` are what every other refactor
    // gesture already reads from here.
    void organizeImports();

    // A protocol position as a document position. The inverse of
    // `lspPosition`, and a re-expression for the same reason: both count
    // UTF-16 code units within a block.
    static int positionAt(const QTextDocument *document, quint32 line, quint32 character);

    // The word under the caret, used by the caret-driven Find Usages and
    // the type-hierarchy jumps. Empty when no tab is open or the caret is
    // not on a word.
    QString wordUnderCursor() const;

    // What the user has selected, as plain text. QTextCursor reports a
    // paragraph separator (U+2029) where the document has a newline, which
    // no consumer of this text — least of all a model prompt — expects.
    QString selectedText() const;

    QString currentPath() const { return docManager_->tabPath(currentTabId()); }

    QString currentContent() const;

    // N2: ask the session to resolve whatever the caret sits on. The
    // answer arrives asynchronously on SearchModel's declaration signals.
    void requestDeclarationAtCaret();

    void setDeclarationRequestedCallback(std::function<void(int)> callback);

    QPlainTextEdit *currentEditor() const;

    // N5: hand the caret's current file and line to the session's jump
    // history. Whether that actually pushes an entry (or collapses into
    // the previous one) is the session's rule, not this widget's.
    void recordCurrentPosition();

    void applyHistoryLocation(const FfiLocation &location);

    void navigationChanged();

    // Task L2: repaint every open editor's squiggles from whatever the
    // language servers have published. Called on the service's
    // diagnosticsChanged signal — the store is the single source, so no
    // per-editor bookkeeping of "which diagnostics are mine" exists here.
    void applyDiagnostics();

    // The current editor's find bar, or nothing when no tab is open.
    void withFindBar(const std::function<void(FindBar *)> &action);

    // Task D: the TabId of whichever tab is current in the active group, or
    // 0 (the "no tab" sentinel, matching FfiOpenResult's convention) when
    // none is open. Public wrapper over the private tabIdAt/activeGroup_
    // pair below, for ClassViewPanel to know which tab its outline belongs
    // to.
    quint64 currentTabId() const;

    // Task D: move the caret to a byte offset within the *current* tab's
    // text and focus it — used by ClassViewPanel's jump-to-symbol, which
    // (unlike Find in Files' openFileAtLine) never needs to open a
    // different file, since Class View always describes the active tab.
    // `byteOffset` is a UTF-8 byte offset into the tab's content (matching
    // `syntax_core::SymbolNode`); converted to a line + in-line byte column
    // here, then to a UTF-16 column by moveCursorToByteColumn.
    void jumpToByteOffset(quint64 byteOffset);

    // Edit > Find/Replace/Find Next/Find Previous. Each just forwards to
    // the current editor's own bar — the bar is what talks to
    // `DocumentManager`, and finding no bar (no tab open) is a no-op.
    void showFindBar();
    void showReplaceBar();
    void findNext();
    void findPrevious();

    // View > Go to Line... The spin box is bounded by the document, so an
    // out-of-range line can't be entered in the first place; the caret is
    // moved through the same helper every other jump uses, so folds and
    // centring behave identically.
    void goToLine();

    // `view.togglePreviewMode`: flip the current tab between its source and
    // a rendered preview of it, for whichever file types a plugin previews
    // (`PreviewProvider::hasPreview`). Nothing happens for a tab whose file
    // has no preview provider.
    //
    // The tab's page widget stays the CodeEditor either way — view mode is a
    // preview panel parented *to* the editor and shown over it, the same
    // shape FindBar already uses. Reparenting the editor out of the tab, the
    // way the editable diff window does, would hide it from `forEachEditor`,
    // `saveAllModified` and `hasUnsavedChanges`, all of which skip a page
    // that is not a QPlainTextEdit — a tab in view mode would then be
    // invisible to Save All and to the quit-time unsaved-changes prompt.
    void togglePreviewMode();

    // Whether `tabId` is currently showing its preview rather than its
    // source. Read by the window to decide whether the Preview *dock* should
    // stand down for this tab (two panels rendering one tab would rasterise
    // its diagrams at whichever panel's width won the race).
    bool previewModeActive(quint64 tabId) const;

    // Re-feed the preview shown over `editor`, if it is in view mode. Called
    // from the same 300ms content debounce the dock uses, so both cadences
    // are one cadence.
    void refreshPreviewMode(CodeEditor *editor);

    // Ctrl+S / File > Save.
    void saveCurrentTab();

    // File > Save As... (L2): the session repoints the tab at the chosen
    // path and writes there; the tree's own watcher picks up the new file
    // for free (no explicit tree-refresh call needed here).
    void saveCurrentTabAs();

    // L3: registers the status bar's line:col and language labels, and
    // fills them in immediately for whatever tab is already current.
    void attachStatusBar(QLabel *positionLabel, QLabel *languageLabel);

    // Exit / window-close (L1): runs the same unsaved-changes prompt as
    // closing tabs one at a time, stopping at the first Cancel so the
    // caller can abort the close.
    bool confirmCloseAllTabs();

    // S2 live-apply: updates every open tab immediately and remembers the
    // choice so tabs opened afterward pick it up too. No persistence here —
    // the settings dialog decides via AppSettings whether to keep (OK) or
    // revert (Cancel) this.
    void setEditorFont(const QFont &font);

    // `backgroundHex`/`foregroundHex` empty means "use the theme's default
    // palette role" (A3): starting from qApp's own palette and overriding
    // only the roles with a value keeps that default live even after a
    // theme switch, rather than freezing whatever color was current when
    // the override was set.
    void setEditorColors(const QString &backgroundHex, const QString &foregroundHex,
                          const QString &currentLineHex);

    // Show-whitespace-characters task, same S2 live-apply convention as
    // setEditorFont/setEditorColors above.
    void setWhitespaceOptions(const WhitespaceOptions &options);

    // L6: the language-server settings were committed and stale servers
    // were stopped, so every open document has to be announced again — to a
    // replacement server for the languages that changed, and to nobody at
    // all for the ones that did not (reopenDocument drops those).
    void reannounceDocuments();

    // A language was turned off or back on, so which language each open
    // file resolves to may have changed — and that is bound when the
    // highlighter is built, not on every repaint. Asking each one to
    // re-resolve is cheaper than tearing tabs down and rebuilding them.
    void reloadHighlighterLanguages();

    // Token colors are resolved by syntax_core::theme from the active
    // theme (and the user's syntax colours) and then cached per
    // highlighter, and a QSyntaxHighlighter only re-runs when its document
    // changes — so a live theme switch has to drop that cache and ask every
    // open editor to re-highlight itself.
    // The icon theme changed under tabs that already hold their art: a tab
    // keeps the QIcon it opened with, unlike the tree and the result lists,
    // which rebuild their rows and pick the new art up on their own.
    void refreshTabIcons();

    void refreshHighlighting();

    // Rename/delete via the tree changed a tab's title (US-2b) — re-render
    // the label, preserving the unsaved-changes indicator.
    void onTabTitleChanged(quint64 tabId, const QString &title);

    // M5: an MCP client's edit_buffer call changed the tab's content —
    // reflect it in the widget immediately, no prompt (unlike a disk
    // change, this came through the same session the widget already
    // trusts). editor->document()->setModified(true) mirrors what
    // onTabOpened's own modificationChanged forwarding would have done had
    // a human typed the same edit.
    void onBufferEditedExternally(quint64 tabId, const QString &content);

    // US-3's external-change prompt: the tab `tabId` (backed by `path`) was
    // modified outside the editor (filesystem watcher). "Reload" re-reads
    // the file from disk, discarding in-editor edits; "Keep" leaves the
    // editor content untouched but marks the tab dirty, since it's now
    // known to differ from what's on disk.
    void handleExternalChange(quint64 tabId, const QString &path);

    // The split layout as JSON, for AppSettings to persist on close: the
    // splitter tree (orientation + sizes) with each group's file paths and
    // its current file. Paths, not TabIds — ids are per-run and mean
    // nothing to the next launch.
    QString saveLayout() const;

    // Rebuilds the splitter tree written by saveLayout() and reopens each
    // group's files into it. Called once at startup with nothing open yet;
    // an empty/unparseable/file-less layout leaves the single default group
    // as built by the constructor. Files that no longer open (deleted,
    // now-unreadable) are skipped — a stale entry must not cost the user
    // the rest of the layout.
    void restoreLayout(const QString &json);

    // F3-16: told once a project is known to be (or not be) a repository.
    // `nullptr` (never called) is the ordinary state for a project with no
    // Git — every gutter/popup path below is a no-op without it, the same
    // "no server for this language" shape LanguageService's absence has.
    void setVcsService(VcsService *vcsService);
    // ADR-0033: the provider the in-tab preview asks for a render. Null in
    // a window built without one, in which case view mode is a no-op —
    // the same "no service, no feature" shape as the three below.
    void setPreviewProvider(PreviewProvider *previewProvider);
    void setRunService(RunService *runService);
    void setDebugService(DebugService *debugService);

    // D2-5: push this file's breakpoints into its gutter, and turn a gutter
    // click into `DebugService::toggleBreakpoint`.
    void refreshBreakpointsFor(CodeEditor *editor);
    void refreshBreakpoints();
    void toggleBreakpointAt(CodeEditor *editor, int blockNumber);
    // D2-3: follow this editor's edits so its breakpoints move with them.
    void watchLineCountFor(CodeEditor *editor);
    // D3: show (or clear, with an empty path) the suspended line.
    // D3-7: re-read the stopped frame's inline values into every editor
    // showing the file they belong to, and clear them everywhere else.
    void refreshInlineValues();
    void showExecutionPoint(const QString &path, int line);

    // R1-7: ask `RunService::canRunFile` whether an editor's file has a run
    // target, and show or hide the gutter's Run icon accordingly.
    void refreshRunMarker(CodeEditor *editor);
    void refreshRunMarkers();
    // The gutter Run icon (or `run.runContext`) fired: launch this editor's
    // file through `RunService::runContext`.
    void requestRunFor(CodeEditor *editor);

    // `VcsService::hunksChanged(path)`: push the hunks it now has for
    // `path` into that file's gutter, if it is open.
    void applyVcsHunks(const QString &path);

    // F3-18: vcs.annotate. Off by default; toggling asks for blame on the
    // active tab's file and applies it once `blameReady` answers, the same
    // "widget never computes it" split every other gutter overlay follows.
    void setAnnotateEnabled(bool enabled);
    bool annotateEnabled() const { return annotateEnabled_; }

    // `VcsService::blameReady(path, lines)`: push blame text for `path`
    // into that file's gutter, if it is still open and annotation is on.
    void applyVcsBlame(const QString &path, const ::rust::Vec<FfiBlameLine> &lines);

    // F3-19: vcs.showDiff — the working-tree-vs-HEAD diff for the current
    // file, in the same dialog shape the gutter's "Show Diff" popup uses.
    // A no-op with nothing cached yet (no VcsService, or the file has no
    // hunks requested for it).
    void showDiffAgainstHead();

    // F3-14: the Changes dock's double-click entry point — opens `path`
    // (focusing it if already open) and immediately shows its editable
    // diff window, the same one `vcs.showDiff` opens for the active tab.
    void showDiffForPath(const QString &path);

    // F3-14: Project Tree's "Compare with…" — a read-only `TabKind::Diff`
    // tab over two arbitrary files' current on-disk contents, no Git
    // involved. Shows an error dialog and opens nothing if either file
    // can't be read.
    void openCompareFiles(const QString &leftPath, const QString &rightPath);

    // F3-14: File History's "Compare with Working Tree" / "Compare Selected
    // Revisions" — a read-only `TabKind::Diff` tab over `path` at two
    // revisions. An empty revision string means the live working text (the
    // open buffer if `path` is open, the file on disk otherwise), which is
    // what "Compare with Working Tree" needs and a revision id never is.
    void openCompareRevisions(const QString &path,
                                const QString &leftRevision,
                                const QString &leftLabel,
                                const QString &rightRevision,
                                const QString &rightLabel);

    // F3-19: vcs.rollbackHunk — reverts whichever cached hunk contains the
    // caret's line, the keyboard equivalent of the gutter popup's Revert.
    void rollbackHunkAtCaret();

    // F3-19: vcs.nextChange/vcs.previousChange (F7/Shift+F7 outside a diff
    // dialog) — moves the caret to the next/previous cached hunk in the
    // current file, wrapping at either end.
    void jumpToChange(bool forward);

private:
    // Where a tab lives now: which group's tab strip, and at which index in
    // it. `group == nullptr` means "no such open tab".
    struct TabLoc
    {
        QTabWidget *group = nullptr;
        int index = -1;
    };

    // The one TabId <-> (group, index) mapping (ADR-0003): the id rides on
    // the page widget itself, so closes, reorders and splits can never
    // desynchronize it.
    quint64 tabIdAt(QTabWidget *group, int index) const;

    TabLoc locate(quint64 tabId) const;

    QPlainTextEdit *editorForTab(quint64 tabId) const;

    void forEachEditor(const std::function<void(QPlainTextEdit *)> &apply) const;

    // The same walk for hex tabs. They are not QPlainTextEdits, so every
    // forEachEditor loop skips them — appearance changes that should reach
    // every page (the editor font, the editor colours) need this too.
    void forEachHexViewer(const std::function<void(HexViewer *)> &apply) const;

    void focusTab(quint64 tabId);

    // One tab group: everything a group needs to behave like the single tab
    // strip used to, plus the context menu and the "clicking me activates
    // me" wiring.
    QTabWidget *makeGroup();

    void setActiveGroup(QTabWidget *group, int index);

    // Right-click on a tab: Close / Close Others (this group only) / the
    // two splits. Splitting *moves* the clicked tab, so it needs a second
    // tab to leave behind — with one tab the split would just relabel the
    // same group.
    void showTabContextMenu(QTabWidget *group, const QPoint &pos);

    // Ids, not indices: each close shifts the ones after it.
    void closeOtherTabs(QTabWidget *group, int keptIndex);

    // Moves the tab into a brand-new group beside (Qt::Horizontal) or below
    // (Qt::Vertical) its current one. Pure widget surgery: no AppSession
    // call, no TabId change, so a file still has exactly one editor widget.
    void splitTab(QTabWidget *group, int index, Qt::Orientation orientation);

    // Drag-and-drop of a tab onto another group's tab strip: the same pure
    // widget surgery splitTab does, only into a group that already exists.
    // A negative `index` appends. Dropping a tab back on its own strip is a
    // no-op — QTabBar's built-in reorder already owns that gesture.
    void moveTabToGroup(quint64 tabId, QTabWidget *target, int index);

    static QList<int> evenSizes(QSplitter *splitter);

    // A group that just lost its last tab disappears, unless it's the only
    // one left (an empty editor area still needs somewhere to open into).
    void collapseGroup(QTabWidget *group);

    // A nested splitter left with a single child adds a level of nothing —
    // hoist the child into the grandparent so a later split reads the
    // orientation it can actually see.
    void pruneSplitters(QSplitter *splitter);

    // Label rendering: the session's display title verbatim, plus the
    // view's own unsaved-changes dot — and the icon the tab's filename
    // resolves to, which is why a rename or a Save As repaints it here.
    void renderTabText(QTabWidget *group, int index, const QString &title, bool modified);

    // Writes the tab's content to disk. Shows an error dialog and leaves the
    // dirty state set on failure (US-4: no silent data loss). Returns
    // whether the save succeeded.
    bool saveTab(QTabWidget *group, int index);

    // The save logic `saveTab(group, index)` runs, factored out so the
    // editable diff window (F3-14) — whose editor briefly isn't any group's
    // page widget — can save through the same path Ctrl+S normally does,
    // rather than a second copy of the tidy/write/refresh sequence.
    bool saveEditor(quint64 tabId, CodeEditor *codeEditor, QPlainTextEdit *editor);

    // Save/Discard/Cancel prompt for a tab with unsaved changes (US-3/US-4).
    // Returns true if the tab is now safe to close. Dirtiness is read from
    // the session — Rust owns that flag (ADR-0003).
    bool confirmCloseTab(QTabWidget *group, int index);

    void requestCloseTab(QTabWidget *group, int index);

    // L3: line:col + language for whatever tab is current, or blank when
    // no tab is open. The "UTF-8" label is static (set once in
    // buildMainWindow) since only UTF-8 is supported today — nothing here
    // needs to touch it.
    void updateStatusBar();

    // Shared with setEditorColors, and with onTabOpened's initial apply.
    void applyEditorAppearance(QPlainTextEdit *editor);

    // Takes a QWidget, not a QPlainTextEdit: the editor colours apply to
    // every page that paints on the editor background, hex tabs included.
    void applyEditorPalette(QWidget *editor);

    // Builds the page for a binary tab: a read-only hex view (ADR-0020).
    // None of the editor wiring below applies — there is no document, so no
    // highlighter, no find bar, no dirty tracking and no LSP.
    // The marker stream (e2e_mark.h). `index` is the tab's position in its
    // own group and `tab_id` the session's stable id: a test asserting the
    // two agree with MCP's view is what catches an index/id mix-up at the
    // model edge.
    void markTab(const char *event, quint64 tabId, QTabWidget *group, int index,
                  const QString &title);

    void markPaneCount();

    void addHexTab(QTabWidget *group, quint64 tabId, const QString &title);

    // Builds the page for a read-only diff tab (F3-14): a `DiffViewPage`
    // over the two texts `DocumentManager::openDiffTab` stored for `tabId`.
    // Like `addHexTab`, `currentEditor()` is `nullptr` for this page — there
    // is no live document, by design (see `TabKind::Diff`'s doc comment).
    void addDiffTab(QTabWidget *group, quint64 tabId, const QString &title);

    // F2-10: the caret-settle debounce fired, or Alt+Return asked directly
    // (`explicitRequest`). Remembers which editor and document position the
    // request is about, so the answer can be positioned and — for an
    // explicit request — turned into the popup without asking the caret
    // again (it may have moved on to something else by the time the answer
    // lands, though a moved caret already cancelled this request first).
    // F2-11 piggybacks document highlights on the same caret-settle tick —
    // both are "what does the caret's position mean" questions, asked
    // about the same editor and position, so one debounce answers both.
    void requestIntentionsFor(CodeEditor *editor, bool explicitRequest);

    // `LanguageService::intentionsReady`. Empty hides the bulb; otherwise
    // positions it at the request's caret line and, for an explicit
    // request, opens the popup immediately.
    void onIntentionsReady();

    // The grouped popup itself, shared by the bulb's click and Alt+Return.
    void showIntentionsMenu();

    // F2-11: `LanguageService::documentHighlightsReady` — paint whatever
    // came back onto the editor `requestIntentionsFor` asked about.
    void onDocumentHighlightsReady();

    // F2-11: signature help, driven straight from `cursorPositionChanged`
    // rather than the caret-settle debounce — `should_request`/
    // `should_dismiss` are cheap and the tip has to track typing live, not
    // after it pauses. `showing` is `signatureTipVisible_`; `explicitRequest`
    // is Ctrl+P's, which asks again even without a trigger character.
    void requestSignatureHelpFor(CodeEditor *editor, bool explicitRequest = false);

    void onSignatureHelpReady();

    // F2-11: inlay hints for whatever `editor` currently has visible.
    // Called on scroll and after a document change settles; a no-op when
    // the toggle is off.
    void requestInlayHintsFor(CodeEditor *editor);

    void onInlayHintsReady();

    // C9-followup: ask for `editor`'s document's semantic tokens. A no-op
    // when the file has no path (`LanguageService::requestSemanticTokens`
    // itself already no-ops without a legend/server, so this only needs to
    // guard the FFI call's own precondition). Called on document open and
    // on the same debounce as `documentChanged`, matching every other
    // per-edit LSP request.
    void requestSemanticTokensFor(CodeEditor *editor);

    // `LanguageService::semanticTokensReady(path)` — repaints whichever
    // open editor has `path`, if any (unlike inlay hints/signature help,
    // this signal carries its own path, so it is not limited to "whatever
    // was last requested").
    void onSemanticTokensReady(const QString &path);

    // C10-followup: ask for `editor`'s document's code lenses. A no-op
    // when the file has no path, same guard as every other per-edit LSP
    // request here. Called on document open and on the same debounce as
    // `documentChanged`.
    void requestCodeLensesFor(CodeEditor *editor);

    // `LanguageService::codeLensesReady(path)` — repaints whichever open
    // editor has `path`, if any, same path-keyed lookup as
    // `onSemanticTokensReady`.
    void onCodeLensesReady(const QString &path);

    // F3-16: ask VcsService for `editor`'s hunks against `HEAD`, against its
    // live text. A no-op without a VcsService or for a file with no path
    // (an unsaved buffer has nothing in `HEAD` to gutter against).
    void requestHunksFor(CodeEditor *editor);


    // The gutter's marker was clicked: build and show the popup for that
    // hunk (Revert / Show Diff / Stage File).
    void onChangeMarkerClicked(CodeEditor *editor, int hunkIndex, const QPoint &globalPos);

    // F3-14: the editable half of "vcs.showDiff". A `QPlainTextEdit`/
    // `CodeEditor` widget can only be in one place at a time, and the
    // working-tree-vs-HEAD diff deliberately keeps the tab's *real*
    // `CodeEditor` — not a copy — as the diff's right pane, so undo/save/LSP
    // stay on the one true `Document` (ADR-0003). That means the editor is
    // physically reparented out of its tab for as long as the diff window
    // is open: this method removes it from `group`, drops a placeholder in
    // its place (a `Show Diff Window` button, since the tab still exists
    // and can still be closed, renamed by a file rename, etc.), and shows a
    // floating, non-modal window built around it. Re-opening an
    // already-open diff for the same tab raises the existing window instead
    // of reparenting a second time. Closing the window restores the editor
    // to its original tab and index.
    void openEditableDiffWindow(quint64 tabId, CodeEditor *editor, const QString &path);

    // Undoes `openEditableDiffWindow`: pulls the editor back out of the
    // (about to close) diff window and puts it back as `tabId`'s page,
    // wherever that tab now sits (a split/reorder may have moved it while
    // the diff window was open). No-op if `tabId` isn't currently diffing.
    void restoreEditorFromDiffWindow(quint64 tabId);

    // Public for the same reason onBufferEditedExternally is: an agent's
    // tool can open a tab (AiChat::toolOpenedTab), and that relay lives in
    // buildMainWindow beside MCP's.
public:
    void onTabOpened(quint64 tabId, const QString &title);

private:
    void onTabClosed(quint64 tabId);

    // Public alongside onTabOpened: an agent's tool can save a buffer
    // (AiChat::toolSavedBuffer), which is the same "no longer modified"
    // event DocumentManager reports.
public:
    void onTabModifiedChanged(quint64 tabId, bool modified);

private:

    QJsonObject serializeSplitter(const QSplitter *splitter) const;

    QJsonObject serializeGroup(QTabWidget *group) const;

    void applySplitter(QSplitter *splitter, const QJsonObject &object);

    void restoreGroup(QSplitter *splitter, const QJsonObject &object);

    DocumentManager *docManager_;
    LanguageService *languageService_;
    // F3-16: null for a project with no Git — set once, after construction,
    // the same retrofit shape setContextMenuCallback uses.
    VcsService *vcsService_ = nullptr;
    PreviewProvider *previewProvider_ = nullptr;
    RunService *runService_ = nullptr;
    DebugService *debugService_ = nullptr;
    // F3-18: vcs.annotate's state, applied to whichever editor is active.
    bool annotateEnabled_ = false;

    // F3-14: which tabs currently have their `CodeEditor` reparented into a
    // floating editable diff window (see `openEditableDiffWindow`), and what
    // to restore. Absent from this map is the overwhelmingly common case —
    // a tab not being diffed right now.
    struct DiffWindowState
    {
        QWidget *window = nullptr;      // Top-level, WA_DeleteOnClose.
        QWidget *placeholder = nullptr; // Sits in the tab meanwhile.
    };
    QHash<quint64, DiffWindowState> diffWindows_;
    // F1-13/F1-15: carets and the language-aware editing operations, for
    // every editor this class opens. Owned here rather than passed in
    // because nothing outside the editor surface has anything to ask it.
    EditorOps *editorOps_;
    QSplitter *root_;
    QWidget *window_;
    QList<QTabWidget *> groups_;
    QTabWidget *activeGroup_ = nullptr;
    QTabWidget *restoredActiveGroup_ = nullptr;
    // True while a split or a restore is moving pages between groups: the
    // currentChanged bookkeeping would otherwise treat those moves as the
    // user activating a group.
    bool suspendActivation_ = false;
    std::function<void()> activeTabChanged_;
    std::function<void(quint64)> previewChanged_;
    QFont editorFont_;
    QString editorBackground_;
    QString editorForeground_;
    QString editorCurrentLine_;
    QLabel *positionLabel_ = nullptr;
    QLabel *languageLabel_ = nullptr;
    // Set while this class is the one moving a caret, so the cursor-moved
    // handler does not push the widget's single caret back over the set
    // Rust just computed. Same arrangement FindBar uses while it is the one
    // editing the document.
    bool syncingCarets_ = false;

    // F2-10: one bulb, reparented to whichever editor's viewport it is
    // currently shown over — like the hover tooltip, only one is ever
    // relevant at a time. `intentionsEditor_`/`intentionsDocPos_` are the
    // editor and document position the *last request* was made against,
    // which is what its answer is positioned against; `intentionsPending_`
    // is set only by an explicit Alt+Return, so a background bulb refresh
    // never pops the menu the user didn't ask for.
    IntentionBulb *intentionBulb_ = nullptr;
    CodeEditor *intentionsEditor_ = nullptr;
    int intentionsDocPos_ = 0;
    bool intentionsPending_ = false;

    // F2-11: the editor a signature-help/inlay-hints request was last made
    // about — same reasoning as `intentionsEditor_`, one relevant answer at
    // a time. `signatureTipVisible_` is what `requestSignatureHelpFor`
    // hands `LanguageService` as `showing`.
    CodeEditor *signatureHelpEditor_ = nullptr;
    bool signatureTipVisible_ = false;
    CodeEditor *inlayHintsEditor_ = nullptr;
    bool inlayHintsEnabled_ = false;
    WhitespaceOptions whitespaceOptions_;
};

} // namespace ui_shell
