#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QPoint>
#include <QString>
#include <QWidget>
#include <functional>

class QComboBox;
class QToolButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {

class EditorTabs;

// Structure dock panel: a QTreeWidget with two data-source tiers, toggled
// by `modeCombo_` (Task I extends Task D's original per-file-only panel —
// "same widget/model, second data-source impl" per the plan doc, not a
// second panel). Humble view per CLAUDE.md's hard rule — outline/symbol
// extraction is entirely `syntax_core`/`index_core`'s job; this only builds
// tree items and forwards double-clicks to a caret jump. Same dock-panel
// shape FindInFilesPanel (Task H) established above.
//
// Per-file tier (Task D): populated from `DocumentManager::tabOutline()`
// for whichever tab is current, reconstructing nesting from each
// `FfiSymbolNode`'s `depth` (per that struct's own doc comment).
//
// Project tier (Task I): populated by streamed `SearchModel::projectSymbolFound`
// signals off `index_core::TextIndex::find_definitions("")` (an empty query
// matches every definition — see the bridge's doc comment), grouped by file
// then by container. Ephemeral view state, like folding's collapsed-state
// (plan doc, Task I) — the toggle's position isn't persisted, and switching
// to Project mode doesn't track tab-switch/save events the way the per-file
// tier does (`refresh()` becomes a no-op in project mode); switching back
// re-syncs it.
class StructurePanel : public QWidget
{
public:
    // Task J: `onFindUsagesRequested` is called with a symbol's exact name
    // when the user picks "Find Usages" from a leaf item's context menu —
    // the panel doesn't know or care what happens with that name (main_window
    // wires it to FindUsagesPanel), keeping this class's only job "show the
    // outline, forward intents".
    StructurePanel(DocumentManager *docManager, SearchModel *searchModel, EditorTabs *editorTabs,
                    std::function<void(const QString &)> onFindUsagesRequested, QWidget *parent);

    // Repopulate the tree from `tabId`'s current outline — called on tab
    // open, on tab switch, and whenever a tab becomes clean (a proxy for
    // "just saved"; see buildCentralWidget's wiring comment for why). A
    // no-op while the Project tier is active (see class doc comment).
    // `tabId == 0` (no tab open) just clears the tree.
    void refresh(quint64 tabId);

private:
    // Task I: (re)issue a project-wide query. Results stream back via
    // `addProjectSymbol` below; `fileItems_`/`containerItems_` are rebuilt
    // from scratch each time, keyed off this call's own tree.
    void refreshProject();

    // One project-wide symbol definition, grouped under a per-file top-level
    // item and (when it has one) a per-container item nested under that —
    // `containerItems_` is keyed by `path + container` since two files can
    // each have their own same-named class.
    void addProjectSymbol(const FfiSymbolMatch &row);

    // Task 4b: the category sub-node `parent`'s children are grouped under
    // for `category` — created lazily, in `FfiSymbolCategory`'s fixed
    // order among whichever sibling categories already exist under the
    // same `parent`, the first time that category is actually seen (both
    // tiers route every symbol/leaf item through this instead of parenting
    // it directly). `groups` is `categoryGroups_` for the project tier
    // (rows stream in over the panel's lifetime) or a call-local map for
    // the per-file tier (`refresh()` rebuilds the whole tree each time).
    // Group items carry no `UserRole` data, same invariant file/container
    // items already follow, so double-click/context-menu keep skipping
    // them for free.
    QTreeWidgetItem *categoryGroup(QHash<QTreeWidgetItem *, QHash<int, QTreeWidgetItem *>> &groups,
                                    QTreeWidgetItem *parent, FfiSymbolCategory category);

    void onItemDoubleClicked(QTreeWidgetItem *item);

    // Task J: "Find Usages" on a leaf symbol item (per-file or project
    // tier alike — both stash the bare name at UserRole+2).
    void onContextMenuRequested(const QPoint &pos);

    DocumentManager *docManager_;
    SearchModel *searchModel_;
    EditorTabs *editorTabs_;
    std::function<void(const QString &)> onFindUsagesRequested_;
    QTreeWidget *tree_ = nullptr;
    QComboBox *modeCombo_ = nullptr;
    QToolButton *sortButton_ = nullptr;
    bool projectMode_ = false;
    QHash<QString, QTreeWidgetItem *> fileItems_;
    QHash<QString, QTreeWidgetItem *> containerItems_;
    // Project tier only; see categoryGroup()'s doc comment.
    QHash<QTreeWidgetItem *, QHash<int, QTreeWidgetItem *>> categoryGroups_;
};

} // namespace ui_shell
