#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QCoreApplication>
#include <QString>
#include <QWidget>

class QComboBox;
class QToolButton;

namespace ui_shell {

// F4-12: the main window's toolbar strip under the menu bar — Run/Stop/Rerun
// for whichever configuration is selected, and the configuration picker at
// the strip's right end (the mockup's `.tbtn-text` pill).
//
// Humble view: what configurations exist is `RunService::configurations()`;
// this widget only lists them and turns a button click into `run`/`stop`/
// `rerun`. It tracks "is the selected configuration's console still
// running" itself, from `consoleStarted`/`consoleFinished`, so it can
// enable/disable Run vs. Stop/Rerun without RunConsolePanel reaching back
// into it.
//
// Buttons are icon-only 26x26 glyphs per the blend spec (Run/Stop/Rerun/
// Build, drawn at runtime — see run_toolbar.cpp's anonymous namespace).
// Build arrived with `BuildService` (B1-7) and Debug with `DebugService`
// (D3-8), so the cluster is now the whole one the mockup shows.
// ADR-0069: turn the PHP debug listener on or off for the Run menu action
// and the toolbar button alike; a failure (typically the adapter not being
// installed) is shown with its install hint.
void setPhpListening(DebugService *debugService, QWidget *parent, bool enabled);

class RunToolbar : public QWidget
{
    // Not a `Q_OBJECT` (no moc target), so `tr()` would resolve in `QWidget`'s
    // context; this keeps it in this class's own, where the .ts entries live.
    Q_DECLARE_TR_FUNCTIONS(RunToolbar)

public:
    RunToolbar(RunService *runService, BuildService *buildService, DebugService *debugService,
               QWidget *parent);

    // Run/Stop/Rerun the currently selected configuration — the targets of
    // the global `run.run`/`run.stop`/`run.rerun` shortcuts, so they act on
    // whatever this toolbar has selected regardless of focus.
    void runSelected();
    void stopSelected();
    // R2-4: stop without the grace period `stopSelected` allows.
    void killSelected();
    void rerunSelected();
    // D3-8: start a debug session for whatever configuration is selected.
    void debugSelected();

    // Gives the configuration combo box keyboard focus — `run.
    // selectConfiguration`'s target.
    void focusConfigSelector();

    RunService *runService() const { return runService_; }

private:
    void refreshConfigurations();
    void refreshButtons();
    QString selectedConfigId() const;

    RunService *runService_;
    BuildService *buildService_;
    DebugService *debugService_;
    QComboBox *configCombo_ = nullptr;
    QToolButton *runButton_ = nullptr;
    QToolButton *stopButton_ = nullptr;
    QToolButton *rerunButton_ = nullptr;
    QToolButton *buildButton_ = nullptr;
    QToolButton *debugButton_ = nullptr;
    QToolButton *listenButton_ = nullptr;

    // ponytail: one running console tracked per configuration id, the
    // latest `run()` call for it. Running the same configuration a second
    // time before the first finishes still opens its own tab in
    // RunConsolePanel (each console keeps its own id there) but this
    // toolbar's Stop/Rerun only reach the newest one — upgrade to a list
    // per configuration if concurrent same-configuration runs need their
    // own toolbar control.
    QHash<QString, quint64> runningConsoleIdByConfig_;
};

} // namespace ui_shell
