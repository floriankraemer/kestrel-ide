#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QPoint>
#include <QString>

class QWidget;

namespace ui_shell {

// R7's remote picker: the only configured remote if there is only one, else
// an explicit choice — replaces every hard-coded `origin` a push/pull/fetch
// call used to go through. Empty if the repository has no remotes
// configured, or the user cancels the picker. Shared between the branch
// popup's own per-branch Push action and the VCS menu's plain Push/Pull/
// Fetch actions, rather than two copies of the same picker.
QString pickRemote(QWidget *anchor, VcsService *vcsService);

// R7: shared outcome handling for `merge`/`rebase`/`cherryPick`/
// `revertCommit` — silence on success (the branch/status signals every
// other write already fires are enough), a conflict shown as what it is
// (pointing at the Changes dock's "Merge Conflicts" group and
// "Resolve...") rather than a bare error, since a full merge editor is out
// of scope (ADR-0031 amendment). Shared between the branch popup's own
// Merge/Rebase actions and the commit log's row context menu's Cherry-pick/
// Revert Commit.
void watchIntegrationResult(QWidget *anchor, VcsService *vcsService, const QString &verb);

// R7: the branch popup — Local and Remote sections with search, and a
// per-branch context menu (Checkout, Merge into Current, Rebase Current
// onto, Rename, Delete, Push, Compare with Current). Replaces the flat
// `QMenu` `vcs_menu.cpp` used to pop up in its place; the function stays
// declared here under its original name so every existing caller (the VCS
// menu's "Branches..." action, the status-bar branch widget, the Changes
// dock toolbar's branch chip) needs no change.
void showBranchMenu(VcsService *vcsService, QWidget *anchor, const QPoint &globalPos);

} // namespace ui_shell
