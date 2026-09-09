#pragma once

#include <QString>

class QWidget;

namespace ui_shell {

// The confirm dialog before discarding a file's uncommitted changes via
// `git checkout HEAD --` (`VcsService::revertFile`): destructive and not
// undoable from inside the IDE once it runs, so every caller asks with the
// same wording rather than each writing its own (G8) — today the project
// tree's Git submenu and the Changes dock's row context menu.
//
// `markName` is the E2E dialog identity ("revert_file_confirm",
// "discard_changes_confirm", ...): each caller's own flow asserts on the
// name that matches *why* it opened the dialog, so the mark stays
// caller-specific even though the wording underneath does not.
//
// Returns whether the user chose to discard (the "Revert" button,
// `QMessageBox::DestructiveRole`); Cancel is the default button.
bool confirmDiscardChanges(QWidget *parent, const QString &fileName, const char *markName);

} // namespace ui_shell
