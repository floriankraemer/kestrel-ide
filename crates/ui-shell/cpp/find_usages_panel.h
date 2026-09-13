#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

class QLabel;
class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {

class EditorTabs;
class SearchPreviewPane;

// Find Usages results dock (Task J): reuses FindInFilesPanel's dockable
// "list of locations, double-click to jump" shape rather than inventing a
// new one — find-usages results are the same kind of thing (a list of
// file:line locations), just fed by `SearchModel::findUsages`/`usagesAt`
// instead of `search`, and triggered from Structure's context menu or
// Navigate instead of typed free text, so there's no query box here.
//
// R8: grouped by file (a `QTreeWidget`, matching `SearchResultsPanel`'s own
// file-grouping rather than a new `QAbstractItemModel`) with the same
// `SearchPreviewPane` Find in Files uses.
class FindUsagesPanel : public QWidget
{
public:
    FindUsagesPanel(SearchModel *searchModel, EditorTabs *editorTabs, QWidget *parent);

    // Called from StructurePanel's "Find Usages" context-menu action, by
    // exact name only — the row clicked carries no live caret position for
    // an LSP `references` request to use.
    void findUsages(const QString &name);

    // R8: Navigate > Find Usages, from the caret. Tries a running language
    // server's `textDocument/references` first and falls back to the
    // index — see `usagesAt`'s doc comment in `ffi.rs`.
    void findUsagesAt(const QString &name, const QString &path, quint32 line, quint32 character);

    // N3: Navigate > Go to Implementation / Go to Interface. Both are
    // lists of file:line locations, which is exactly what this dock
    // already renders, so they stream in on the same signals rather than
    // getting a near-identical panel of their own.
    void findImplementations(const QString &name);

    void findSupertypes(const QString &name);

private:
    void beginQuery(const QString &status);
    void addUsage(const FfiSymbolMatch &row);
    void openSelected(QTreeWidgetItem *item, int column);
    void previewSelection();
    QTreeWidgetItem *fileGroup(const QString &path);

    SearchModel *searchModel_;
    EditorTabs *editorTabs_;
    QLabel *statusLabel_ = nullptr;
    QTreeWidget *results_ = nullptr;
    SearchPreviewPane *preview_ = nullptr;
};

} // namespace ui_shell
