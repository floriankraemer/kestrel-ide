#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QRect>
#include <QString>
#include <QStringList>
#include <QWidget>

class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {

// A commit list shared by the File History panel and the repo-wide Commit
// Log panel: one row per commit (locale-formatted date, short id, author,
// summary), click-to-expand showing the full message body (fetched lazily,
// on first expand, via `VcsService::requestCommitDetail`/`commitDetail`),
// and double-click to open the commit-detail dock.
//
// A `QTreeWidget` rather than a `QListWidget` with hand-tracked expansion
// state: each commit is a top-level item with one non-selectable child
// holding the wrapped body text, so expand/collapse, keyboard nav and the
// arrow indicator all come from Qt's own tree behaviour instead of being
// re-implemented here.
//
// Humble view: which commits exist, in what order, and what one said in
// full are all `vcs-core`'s (via `VcsService`); this only lays the answer
// out and forwards activation.
class HistoryListView : public QWidget
{
    Q_OBJECT

public:
    explicit HistoryListView(VcsService *vcsService, QWidget *parent);

    // Replaces every row. Existing expansion state is not preserved — a
    // fresh answer is a fresh list, the same as the old `QListWidget` panel.
    void setEntries(const ::rust::Vec<FfiLogEntry> &entries);

    int commitCount() const;
    QString commitIdAt(int row) const;
    // `row`'s rect in global (screen) coordinates, for an E2E flow to click
    // or right-click a specific commit without recomputing row geometry
    // itself — the same convention `FileHistoryPanel`'s old marking loop
    // used. Empty (default `QRect`) if the row is not currently visible.
    QRect globalRectForRow(int row) const;

    QStringList selectedCommitIds() const;
    // The first selected row's commit id, or empty with nothing selected.
    QString firstSelectedCommitId() const;

signals:
    // Double-click (or Enter) on a commit row.
    void commitActivated(const QString &commitId);
    // Right-click, with every currently selected commit id — the context
    // menu itself (which actions make sense for one vs. two selections)
    // stays the caller's, the same split `FileHistoryPanel`'s menu already
    // draws.
    void contextMenuRequestedFor(const QPoint &globalPos, const QStringList &selectedIds);

private:
    void onItemExpanded(QTreeWidgetItem *item);
    void onItemActivated(QTreeWidgetItem *item, int column);
    void showContextMenu(const QPoint &pos);
    void populateBody(QTreeWidgetItem *placeholder, const QString &commitId);

    VcsService *vcsService_;
    QTreeWidget *tree_ = nullptr;
};

} // namespace ui_shell
