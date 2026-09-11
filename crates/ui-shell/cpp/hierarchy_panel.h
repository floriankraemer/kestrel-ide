#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

class QComboBox;
class QLabel;
class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {

class EditorTabs;

// C11-followup: call/type hierarchy dock. The Rust+FFI pipeline
// (`LanguageService::requestCallHierarchy`/`requestIncomingCalls`/
// `requestOutgoingCalls`/`requestTypeHierarchy`/`requestSupertypes`/
// `requestSubtypes`) is wired and reachable end to end (stub_server_session.rs's
// C11 tests, lsp_core::hierarchy's unit tests); this is the first `cpp/`
// consumer, the same shape StructurePanel's outline tree and
// FindUsagesPanel's location list already established for their own data.
//
// `modeCombo_` picks one of four edges (Incoming/Outgoing Calls,
// Supertypes/Subtypes) at a time; only one request can be "in flight" per
// the FFI's own design (`requestIncomingCalls`/etc. index into whichever
// `prepareCallHierarchy`/`prepareTypeHierarchy` answer landed last), so the
// tree is lazily populated one node at a time: expanding a node re-prepares
// the hierarchy at that node's own position (resetting the server's "current
// item" to it) and then asks for its edges, the same two-step round trip
// `showCallHierarchyAt`/`showTypeHierarchyAt` do for the root. `expandTarget_`
// is the one node currently waiting on that round trip — like
// `EditorTabs::inlayHintsEditor_`'s single-latch pattern, not guarded against
// a second expand landing first, matching StructurePanel/FindUsagesPanel's
// own lack of stale-answer protection for this class of panel.
class HierarchyPanel : public QWidget
{
public:
    HierarchyPanel(LanguageService *languageService, EditorTabs *editorTabs, QWidget *parent);

    // N9: Navigate > Show Call Hierarchy / Show Type Hierarchy, called with
    // the caret's own file and LSP position (0-based line/character,
    // `EditorTabs::lspPositionAt`'s own convention).
    void showCallHierarchyAt(const QString &path, quint32 line, quint32 character);
    void showTypeHierarchyAt(const QString &path, quint32 line, quint32 character);

private:
    enum class Mode { IncomingCalls, OutgoingCalls, Supertypes, Subtypes };

    bool isCallMode() const { return mode_ == Mode::IncomingCalls || mode_ == Mode::OutgoingCalls; }

    // Re-issues the root request (call or type hierarchy, whichever
    // `isCallMode()` says) at the position the dock was last opened with —
    // used both by the initial show and by a mode-combo switch, which must
    // restart from the root rather than reinterpret whatever the previous
    // mode's tree already held.
    void restartFromRoot();

    // `node`'s own stored position (see addNode's doc comment) re-prepared,
    // setting `expandTarget_` to `node` so the next `callHierarchyReady`/
    // `typeHierarchyReady` knows to request `node`'s edges instead of
    // rebuilding the root.
    void expandNode(QTreeWidgetItem *node);

    void onModeChanged(int index);
    void onItemExpanded(QTreeWidgetItem *item);
    void onItemDoubleClicked(QTreeWidgetItem *item);

    void onIncomingCallsReady();
    void onOutgoingCallsReady();
    void onSupertypesReady();
    void onSubtypesReady();

    // Shared by callHierarchyReady/typeHierarchyReady: build one top-level
    // node for `items[0]` if this landed for the root (`expandTarget_`
    // still null coming in), or hand off to `requestEdgesForTarget` if it
    // landed for a deeper `expandNode` call (`expandTarget_` already set
    // from before the re-prepare request).
    void onHierarchyPrepared();

    // Fires whichever of the four edge requests `mode_` currently means,
    // for item 0 of the just-landed prepare answer — always item 0,
    // because both the root and every `expandNode` re-prepare narrow the
    // server's answer to exactly the one node being expanded.
    void requestEdgesForTarget();

    // One child under `parent` (or a new top-level item when `parent` is
    // null) for `item`, carrying its jump target (path, 1-based line,
    // 0-based UTF-16 column — `FfiHierarchyItem`'s own convention, the same
    // one `DeclarationNavigator::jumpTo` already jumps with) and a
    // placeholder "Loading..." grandchild so the expand arrow shows before
    // its own edges are known. `suffix` is call hierarchy's caller/callee
    // count, shown in the label when non-empty; type hierarchy has none.
    QTreeWidgetItem *addNode(QTreeWidgetItem *parent, const FfiHierarchyItem &item,
                             const QString &suffix);

    // Removes `node`'s placeholder child and returns true, or returns
    // false if it has already been cleared (a node is only ever populated
    // once).
    bool takePlaceholder(QTreeWidgetItem *node);

    LanguageService *languageService_;
    EditorTabs *editorTabs_;
    QComboBox *modeCombo_ = nullptr;
    QLabel *statusLabel_ = nullptr;
    QTreeWidget *tree_ = nullptr;
    Mode mode_ = Mode::IncomingCalls;
    QString rootPath_;
    quint32 rootLine_ = 0;
    quint32 rootCharacter_ = 0;
    // The node whose edges the next *CallsReady/*typesReady signal fills
    // in; null while waiting on a *HierarchyReady for the root itself.
    QTreeWidgetItem *expandTarget_ = nullptr;
};

} // namespace ui_shell
