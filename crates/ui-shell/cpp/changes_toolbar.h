#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QToolButton;

namespace ui_shell {

// The Changes dock's toolbar (G6): branch chip, Refresh/Fetch/Pull/Push,
// Stage all/Unstage all — the F3-17 dock's own precedent for what a toolbar
// widget in this app looks like (`DiffToolbar`).
//
// Humble view per CLAUDE.md: every enabled state, every ahead/behind count
// and whether `--set-upstream` applies comes straight out of
// `VcsService::branchStatus()`/`changedFiles()` — this widget only reads
// them and paints. Refresh/Fetch/Pull/Push/Stage all/Unstage all *only*
// emit signals; `ChangesPanel` (the owner) wires those to the actual bridge
// calls, the same split `DiffToolbar`'s previous/next signals use. The
// branch chip is the one exception: it opens the existing branch menu
// (`showBranchMenu`, promoted from `vcs_menu.cpp` for this) directly, the
// same action `buildBranchWidget`'s status-bar chip already offers — reused
// rather than copied, which is why this widget alone keeps a `VcsService`
// pointer instead of only signals.
class ChangesToolbar : public QWidget
{
    Q_OBJECT

public:
    explicit ChangesToolbar(VcsService *vcsService, QWidget *parent = nullptr);

signals:
    void refreshRequested();
    void fetchRequested();
    void pullRequested();
    void pushRequested();
    void stageAllRequested();
    void unstageAllRequested();

private:
    void refresh();

    VcsService *vcsService_;
    QToolButton *branchChip_ = nullptr;
    QToolButton *refreshButton_ = nullptr;
    QToolButton *fetchButton_ = nullptr;
    QToolButton *pullButton_ = nullptr;
    QToolButton *pushButton_ = nullptr;
    QToolButton *stageAllButton_ = nullptr;
    QToolButton *unstageAllButton_ = nullptr;
};

} // namespace ui_shell
