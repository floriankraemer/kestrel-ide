#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

#include <functional>

class QComboBox;
class QLineEdit;
class QListWidget;
class QPlainTextEdit;
class QPoint;
class QToolButton;
class QTreeWidget;
class QTreeWidgetItem;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;

// The Debug dock (D3): threads and frames, variables, watches and evaluate,
// and the debugger console for the running session, with the stepping
// controls above them.
//
// Humble view: every action is one `DebugService` call, every row comes from
// one of its cached answers, and each button's enabled state comes from the
// session's state or the adapter's declared capabilities — never from this
// widget deciding what a debugger can do (ADR-0041).
class DebugPanel : public QWidget
{
public:
    // Opens `path:line` in the editor — the same `EditorTabs::openFileAtLine`
    // every other dock's "jump to source" goes through (R5: double-click on
    // a frame).
    using OpenAt = std::function<void(const QString &, int, int)>;

    DebugPanel(DebugService *debugService, OpenAt openAt, QWidget *parent);

    // The targets of the global `debug.*` shortcuts, so they act regardless
    // of which widget has focus — the same arrangement the run and build
    // shortcuts have.
    void resume();
    void pause();
    void stepOver();
    void stepInto();
    void stepOut();
    void stopSession();
    bool hasSession() const { return sessionId_ != 0; }
    // Which session the views are showing — what the Debug menu asks before
    // listing an adapter's exception filters (D4-3/D4-5).
    quint64 currentSession() const { return sessionId_; }

private:
    void onStarted(quint64 sessionId, const QString &configId);
    void onStopped(quint64 sessionId, const QString &reason, const QString &path, quint32 line);
    void onResumed(quint64 sessionId);
    void onTerminated(quint64 sessionId, int exitCode);
    void onFailed(quint64 sessionId, const FfiResult &error);
    void onOutput(quint64 sessionId, const QString &category, const QString &text);
    void onVariablesChanged(quint64 sessionId, qint64 reference);
    void onWatchChildrenChanged(quint64 sessionId, qint64 reference);
    void onEvaluatedToTree(quint64 sessionId, const FfiVariable &row);
    void onWatchesChanged();
    void refreshSessions();
    void refreshThreads();
    void refreshFrames();
    void expandItem(QTreeWidgetItem *item);
    void expandWatchItem(QTreeWidgetItem *item, QTreeWidget *tree);
    void applyVariableFilter(const QString &text);
    // R5: Copy Value, Copy Path and — on the Watches tree only — Remove
    // Watch. Shared by the Variables, Watches and Evaluate trees, since a
    // row is a row regardless of which one it came from.
    void showTreeContextMenu(QTreeWidget *tree, const QPoint &globalPos);
    void setRunning(bool running);

    DebugService *debugService_;
    OpenAt openAt_;
    quint64 sessionId_ = 0;
    // Set while a tree is being rebuilt from `DebugService`'s cache, so
    // `itemChanged` (fired by our own `setText`) is not mistaken for the
    // user finishing an inline edit.
    bool populating_ = false;

    QComboBox *sessionPicker_ = nullptr;
    QComboBox *threadPicker_ = nullptr;
    QToolButton *resumeButton_ = nullptr;
    QToolButton *pauseButton_ = nullptr;
    QToolButton *stopButton_ = nullptr;
    QToolButton *stepOverButton_ = nullptr;
    QToolButton *stepIntoButton_ = nullptr;
    QToolButton *stepOutButton_ = nullptr;
    QListWidget *frames_ = nullptr;
    QLineEdit *variableFilter_ = nullptr;
    QTreeWidget *variables_ = nullptr;
    QTreeWidget *watches_ = nullptr;
    QLineEdit *watchInput_ = nullptr;
    QTreeWidget *evaluateTree_ = nullptr;
    QPlainTextEdit *console_ = nullptr;
    QLineEdit *evaluateInput_ = nullptr;
};

// Builds the panel, wraps it in a dock widget and registers it under id
// `"debug"` — the same one-call pattern the run console and build docks use.
DebugPanel *buildDebugDock(ads::CDockManager *dockManager, DockRegistry *docks,
                            ads::CDockAreaWidget *relativeTo, DebugService *debugService,
                            DebugPanel::OpenAt openAt);

} // namespace ui_shell
