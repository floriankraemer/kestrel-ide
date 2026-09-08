#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <QWidget>

class QLabel;
class QPlainTextEdit;
class QVBoxLayout;

namespace ui_shell {

// One commit's full detail (F3-12e): header (id, author, committer,
// parents), the full message body, and one `DiffView` per changed file —
// the body of one tab in `CommitDetailPanel`.
//
// Humble view: what a commit says, what it touched and what changed in
// each file are all `vcs-core`'s (via `VcsService::requestCommitDetail`/
// `changedCommitFiles`/`requestCommitFileDiff`); this only lays the
// answers out as they arrive, each file's `DiffView` populated
// independently of the others.
class CommitDetailView : public QWidget
{
    // Q_OBJECT rather than the Q_OBJECT-free shape most of this dock's
    // siblings use: `CommitDetailPanel::closeTab` needs `qobject_cast` to
    // recover which commit a closed tab's widget belonged to, which needs
    // a real metaobject.
    Q_OBJECT

public:
    CommitDetailView(VcsService *vcsService, const QString &commitId, QWidget *parent);

    const QString &commitId() const { return commitId_; }

private:
    void onCommitDetailReady(const QString &id);
    void onCommitFileDiffReady(const QString &id, const QString &path);

    VcsService *vcsService_;
    QString commitId_;
    QLabel *headerLabel_ = nullptr;
    QPlainTextEdit *bodyEdit_ = nullptr;
    QVBoxLayout *diffsLayout_ = nullptr;
    QHash<QString, QWidget *> diffContainersByPath_;
};

} // namespace ui_shell
