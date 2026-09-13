#include "editing_actions.h"

#include "editor_tabs.h"
#include "keymap_page.h"

#include <QAction>
#include <QMenu>
#include <QSignalBlocker>
#include <QWidget>

namespace ui_shell {

namespace {

// `EditorOps::lineOp`'s constants, in the order the bridge declares them.
constexpr quint8 kLineOpDuplicate = 0;
constexpr quint8 kLineOpMoveUp = 1;
constexpr quint8 kLineOpMoveDown = 2;
constexpr quint8 kLineOpDelete = 3;
constexpr quint8 kLineOpJoin = 4;

// One menu entry plus the operation it runs. `run` gets the tab id and the
// live buffer text and returns the edits to splice.
QAction *addEditingAction(QMenu *menu, QWidget *window, AppSettings *appSettings,
                          QHash<QString, QAction *> &actions, EditorTabs *editorTabs,
                          const QString &id, const QString &text,
                          std::function<::rust::Vec<FfiTextEdit>(quint64, const QString &)> run)
{
    QAction *action = registerAction(menu, id, text, appSettings, actions);
    QObject::connect(action, &QAction::triggered, window, [editorTabs, run]() {
        editorTabs->runEditorOp(run);
    });
    return action;
}

} // namespace

void buildEditingActions(QMenu *editMenu, QWidget *window, AppSettings *appSettings,
                         QHash<QString, QAction *> &actions, EditorTabs *editorTabs)
{
    EditorOps *ops = editorTabs->editorOps();

    editMenu->addSeparator();

    // The caret operations change no text, so they are not runEditorOp's
    // shape: they move carets and the widget repaints them.
    QAction *nextOccurrence = registerAction(editMenu,
                                             QStringLiteral("edit.selectNextOccurrence"),
                                             QObject::tr("Select Next Occurrence"), appSettings,
                                             actions);
    QObject::connect(nextOccurrence, &QAction::triggered, window, [editorTabs, ops]() {
        editorTabs->withCurrentEditor([ops](quint64 tabId, const QString &text) {
            ops->selectNextOccurrence(tabId, text);
        });
    });

    QAction *caretAbove = registerAction(editMenu, QStringLiteral("edit.addCaretAbove"),
                                         QObject::tr("Add Caret Above"), appSettings, actions);
    QObject::connect(caretAbove, &QAction::triggered, window, [editorTabs, ops]() {
        editorTabs->withCurrentEditor([ops](quint64 tabId, const QString &text) {
            ops->addCaretVertically(tabId, text, false);
        });
    });

    QAction *caretBelow = registerAction(editMenu, QStringLiteral("edit.addCaretBelow"),
                                         QObject::tr("Add Caret Below"), appSettings, actions);
    QObject::connect(caretBelow, &QAction::triggered, window, [editorTabs, ops]() {
        editorTabs->withCurrentEditor([ops](quint64 tabId, const QString &text) {
            ops->addCaretVertically(tabId, text, true);
        });
    });

    editMenu->addSeparator();

    addEditingAction(editMenu, window, appSettings, actions, editorTabs,
                     QStringLiteral("edit.toggleLineComment"),
                     QObject::tr("Comment with Line Comment"),
                     [ops](quint64 tabId, const QString &text) {
                         return ops->toggleComment(tabId, text, false);
                     });
    addEditingAction(editMenu, window, appSettings, actions, editorTabs,
                     QStringLiteral("edit.toggleBlockComment"),
                     QObject::tr("Comment with Block Comment"),
                     [ops](quint64 tabId, const QString &text) {
                         return ops->toggleComment(tabId, text, true);
                     });

    editMenu->addSeparator();

    addEditingAction(editMenu, window, appSettings, actions, editorTabs,
                     QStringLiteral("edit.duplicateLine"),
                     QObject::tr("Duplicate Line or Selection"),
                     [ops](quint64 tabId, const QString &text) {
                         return ops->lineOp(tabId, text, kLineOpDuplicate);
                     });
    addEditingAction(editMenu, window, appSettings, actions, editorTabs,
                     QStringLiteral("edit.moveLineUp"), QObject::tr("Move Line Up"),
                     [ops](quint64 tabId, const QString &text) {
                         return ops->lineOp(tabId, text, kLineOpMoveUp);
                     });
    addEditingAction(editMenu, window, appSettings, actions, editorTabs,
                     QStringLiteral("edit.moveLineDown"), QObject::tr("Move Line Down"),
                     [ops](quint64 tabId, const QString &text) {
                         return ops->lineOp(tabId, text, kLineOpMoveDown);
                     });
    addEditingAction(editMenu, window, appSettings, actions, editorTabs,
                     QStringLiteral("edit.deleteLine"), QObject::tr("Delete Line"),
                     [ops](quint64 tabId, const QString &text) {
                         return ops->lineOp(tabId, text, kLineOpDelete);
                     });
    addEditingAction(editMenu, window, appSettings, actions, editorTabs,
                     QStringLiteral("edit.joinLines"), QObject::tr("Join Lines"),
                     [ops](quint64 tabId, const QString &text) {
                         return ops->lineOp(tabId, text, kLineOpJoin);
                     });

    editMenu->addSeparator();

    QAction *expand = registerAction(editMenu, QStringLiteral("edit.expandSelection"),
                                     QObject::tr("Extend Selection"), appSettings, actions);
    QObject::connect(expand, &QAction::triggered, window, [editorTabs, ops]() {
        editorTabs->withCurrentEditor([ops](quint64 tabId, const QString &text) {
            ops->expandSelection(tabId, text);
        });
    });

    QAction *shrink = registerAction(editMenu, QStringLiteral("edit.shrinkSelection"),
                                     QObject::tr("Shrink Selection"), appSettings, actions);
    QObject::connect(shrink, &QAction::triggered, window, [editorTabs, ops]() {
        editorTabs->withCurrentEditor(
          [ops](quint64 tabId, const QString &) { ops->shrinkSelection(tabId); });
    });

    QAction *matching = registerAction(editMenu, QStringLiteral("edit.matchingBracket"),
                                       QObject::tr("Go to Matching Bracket"), appSettings,
                                       actions);
    QObject::connect(matching, &QAction::triggered, window,
                     [editorTabs]() { editorTabs->jumpToMatchingBracket(); });
}

void wireSoftWrapToggle(QMenu *viewMenu, AppSettings *appSettings,
                        QHash<QString, QAction *> &actions, EditorTabs *editorTabs)
{
    QAction *action = registerAction(viewMenu, QStringLiteral("view.toggleSoftWrap"),
                                     QObject::tr("Soft Wrap"), appSettings, actions);
    action->setCheckable(true);
    action->setChecked(editorTabs->softWrapEnabled());
    QObject::connect(action, &QAction::triggered, viewMenu, [editorTabs, action]() {
        const bool enabled = editorTabs->toggleSoftWrap();
        const QSignalBlocker blocker(action);
        action->setChecked(enabled);
    });
}

} // namespace ui_shell
