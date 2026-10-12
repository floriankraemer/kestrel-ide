#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCoreApplication>
#include <QString>
#include <QWidget>

class QLabel;
class QPlainTextEdit;
class QToolButton;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;

// The Build dock (B1-7): what the project's build tool printed, with a
// header saying what is being run and how it ended.
//
// Humble view: which steps a build runs, and which of its lines are
// diagnostics, are `BuildService`/`build-core` answers (ADR-0040). This
// widget appends the text it is handed and turns Stop into a call. The
// problems themselves are not listed here — they go to the Problems dock,
// which is the one place a user looks for them.
class BuildPanel : public QWidget
{
    // Not a `Q_OBJECT` (no moc target), so `tr()` would resolve in a Qt base
    // class's context; this keeps it in this class's own, where the .ts entries live.
    Q_DECLARE_TR_FUNCTIONS(BuildPanel)

public:
    BuildPanel(BuildService *buildService, QWidget *parent);

    // A before-launch task (B2-2) runs outside `BuildService` — it is part
    // of a launch, not of a build — but it is the same kind of output and
    // belongs in the same dock. `RunService`'s before-launch signals are
    // routed here rather than into a second panel.
    void showExternalTask(const QString &label);
    void appendExternalOutput(const QString &text);
    void reportExternalFailure(const QString &message);

    // The targets of the global `build.*` shortcuts, so they act regardless
    // of which widget has focus — the same arrangement `RunConsolePanel`
    // has with the run shortcuts.
    void buildProject();
    void rebuildProject();
    void stopBuild();

private:
    void onBuildStarted(quint64 buildId, const QString &command);
    void onBuildOutput(quint64 buildId, const QString &text);
    void onBuildFinished(quint64 buildId, int exitCode);
    void report(const FfiResult &result);

    BuildService *buildService_;
    QLabel *header_ = nullptr;
    QPlainTextEdit *output_ = nullptr;
    QToolButton *stopButton_ = nullptr;
    quint64 currentBuild_ = 0;
};

// Builds the panel, wraps it in a dock widget and registers it with `docks`
// under id `"build"` — the same one-call pattern `buildRunConsoleDock` uses,
// for the same reason: `main_window.cpp` is at its 1200-line ceiling
// (ADR-0025).
BuildPanel *buildBuildDock(ads::CDockManager *dockManager, DockRegistry *docks,
                            ads::CDockAreaWidget *relativeTo, BuildService *buildService);

} // namespace ui_shell
