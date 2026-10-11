#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QKeySequence>
#include <QString>

class QAction;
class QMenu;
class QWidget;

namespace ui_shell {

// Settings > Keymap: the table of rebindable actions and the controls that
// change them. Split out of main_window.cpp because — unlike the Appearance
// and Editor pages — it needs nothing from EditorTabs, only the KeymapEditor
// QObject holding the dialog's draft keymap.
//
// Humble view (ADR-0002): every rule (default fallback, which actions a
// shortcut clashes with, what stealing does) is a KeymapEditor call into
// app-config. This file only renders rows and asks the user to confirm.
QWidget *buildKeymapPage(QWidget *parent, KeymapEditor *editor);

// Adds a menu action whose shortcut comes from the persisted keymap rather
// than a literal QKeySequence, and records it under its stable action id so
// Settings > Keymap can re-apply a rebinding without rebuilding the menus.
//
// `id` must be one of app_config::ACTIONS' ids — that catalog, not the view,
// is where an action's default shortcut lives.
QAction *registerAction(QMenu *menu, const QString &id, const QString &text,
                        AppSettings *appSettings, QHash<QString, QAction *> &actions);

// Re-reads every registered action's shortcut from settings — run after the
// Keymap page commits, so a rebinding takes effect without a restart. An
// action left unbound gets an empty QKeySequence, which Qt renders as no
// accelerator at all.
void applyKeymap(const QHash<QString, QAction *> &actions, AppSettings *appSettings);

// How far below its owner a scoped shortcut reaches.
enum class ShortcutReach { Owner, OwnerAndChildren };

// ADR-0074: gives `action` the keymap shortcut of `id`, live only while focus
// is in `owner` (and its children, for `OwnerAndChildren`), and adds it to
// `owner`. The one way the view scopes a keymap action; there is no
// window-wide reach to pass.
//
// `id` must be a scoped action in app_config::keymap (`SCOPED_ACTIONS`), and
// every scoped action must be registered here or through `scopedShortcutKey`:
// `every_scoped_action_is_registered_widget_scoped` checks the calls, so
// `id` must be spelled as a `QStringLiteral` at the call.
QAction *scopeShortcut(QAction *action, QWidget *owner, const QString &id,
                       AppSettings *appSettings, ShortcutReach reach);

// ADR-0074: the keymap shortcut of the scoped action `id` that a key filter
// on its owning widget handles, for an action with no QAction. Same rules
// for `id` as `scopeShortcut`.
QKeySequence scopedShortcutKey(const QString &id, AppSettings *appSettings);

} // namespace ui_shell
