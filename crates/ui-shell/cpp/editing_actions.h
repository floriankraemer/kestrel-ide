#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>

class QAction;
class QMenu;
class QWidget;

namespace ui_shell {

class EditorTabs;

// The Edit menu's editing operations (F1-16): multi-caret, comment toggling,
// the line operations, expand/shrink selection and the bracket jump.
//
// Its own translation unit rather than more of `buildMainWindow`: fourteen
// entries is a fifth of that function again, and `main_window.cpp` sits
// twenty-five lines under the size ceiling.
//
// Every entry does the same thing — hand `EditorTabs::runEditorOp` a closure
// that asks `EditorOps` for a transaction. What any of these operations
// *means* is decided in `editor-core` and `edit-ops`; nothing here branches
// on the language, the selection or the text.
void buildEditingActions(QMenu *editMenu, QWidget *window, AppSettings *appSettings,
                         QHash<QString, QAction *> &actions, EditorTabs *editorTabs);

// R1: View > Soft Wrap. A checkable toggle rather than a plain
// `registerAction`/`triggered` pair like `wirePreviewModeAction` — its
// checked state has to track the persisted setting, which a hand-edited
// settings file or the Editing settings page can also change, not just this
// action.
void wireSoftWrapToggle(QMenu *viewMenu, AppSettings *appSettings,
                        QHash<QString, QAction *> &actions, EditorTabs *editorTabs);

} // namespace ui_shell
