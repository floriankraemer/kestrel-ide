#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

class QLabel;
class QListWidget;
class QListWidgetItem;

namespace ui_shell {

class EditorTabs;

// Find Usages results dock (Task J): reuses FindInFilesPanel's dockable
// "list of locations, double-click to jump" shape rather than inventing a
// new one — find-usages results are the same kind of thing (a list of
// file:line locations), just fed by `SearchModel::findUsages` instead of
// `search`, and triggered from Structure's context menu instead of typed
// free text, so there's no query box here.
class FindUsagesPanel : public QWidget
{
public:
    FindUsagesPanel(SearchModel *searchModel, EditorTabs *editorTabs, QWidget *parent);

    // Called from StructurePanel's "Find Usages" context-menu action and
    // from Navigate > Find Usages (via main_window's wiring) with the
    // symbol's exact name.
    void findUsages(const QString &name);

    // N3: Navigate > Go to Implementation / Go to Interface. Both are
    // lists of file:line locations, which is exactly what this dock
    // already renders, so they stream in on the same signals rather than
    // getting a near-identical panel of their own.
    void findImplementations(const QString &name);

    void findSupertypes(const QString &name);

private:
    // `index_core::TextIndex::find_usages` already returns results sorted
    // by (path, line) — see `SearchModel::find_usages` — so consecutive
    // rows here already read as grouped by file with no extra tree
    // structure needed.
    void beginQuery(const QString &status);

    void addUsage(const FfiSymbolMatch &row);

    void openSelected(QListWidgetItem *item);

    SearchModel *searchModel_;
    EditorTabs *editorTabs_;
    QLabel *statusLabel_ = nullptr;
    QListWidget *resultsList_ = nullptr;
};

} // namespace ui_shell
