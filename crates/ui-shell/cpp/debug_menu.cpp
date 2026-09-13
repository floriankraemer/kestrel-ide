#include "debug_menu.h"

#include "breakpoint_dialog.h"
#include "breakpoints_window.h"
#include "code_editor.h"
#include "debug_panel.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "editor_tabs.h"
#include "keymap_page.h"
#include "run_console_panel.h"

#include <QAction>
#include <QMainWindow>
#include <QMenu>
#include <QMenuBar>
#include <QPlainTextEdit>
#include <QInputDialog>
#include <QLineEdit>
#include <QTextCursor>

namespace ui_shell {

void buildDebugMenu(QMainWindow *window, DebugService *debugService, DebugPanel *debugPanel,
                     RunConsolePanel *runConsolePanel, EditorTabs *editorTabs, AppSettings *appSettings,
                     QHash<QString, QAction *> &actions, DockRegistry *docks, QMenu *viewMenu)
{
    QMenu *debugMenu = window->menuBar()->addMenu(QObject::tr("&Debug"));
    QObject::connect(debugMenu, &QMenu::aboutToShow, debugMenu,
                      []() { e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"debug_menu\"}"); });
    QObject::connect(debugMenu, &QMenu::aboutToHide, debugMenu,
                      []() { e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"debug_menu\"}"); });

    const auto showDock = [docks]() { docks->show(QStringLiteral("debug")); };

    QAction *debugAction = registerAction(debugMenu, QStringLiteral("debug.debug"),
                                          QObject::tr("Debug"), appSettings, actions);
    QObject::connect(debugAction, &QAction::triggered, debugPanel, [runConsolePanel, showDock]() {
        showDock();
        // The same configuration the toolbar is showing, started by the
        // adapter instead of by the run console — which is the only
        // difference between Run and Debug.
        runConsolePanel->debugSelected();
    });

    struct Entry
    {
        const char *id;
        const char *label;
        void (DebugPanel::*action)();
    };
    // The stepping set IntelliJ's debugging page names, minus the ones that
    // need a capability no adapter here declares yet (force step into,
    // smart step into) — those arrive with D4 rather than as buttons that
    // do nothing.
    const Entry entries[] = {
      {"debug.resume", "Resume Program", &DebugPanel::resume},
      {"debug.pause", "Pause Program", &DebugPanel::pause},
      {"debug.stepOver", "Step Over", &DebugPanel::stepOver},
      {"debug.stepInto", "Step Into", &DebugPanel::stepInto},
      {"debug.stepOut", "Step Out", &DebugPanel::stepOut},
      {"debug.stop", "Stop Debugging", &DebugPanel::stopSession},
    };
    for (const Entry &entry : entries) {
        QAction *action = registerAction(debugMenu, QString::fromLatin1(entry.id),
                                          QObject::tr(entry.label), appSettings, actions);
        const auto method = entry.action;
        QObject::connect(action, &QAction::triggered, debugPanel,
                          [debugPanel, method]() { (debugPanel->*method)(); });
    }

    QAction *attachAction = registerAction(debugMenu, QStringLiteral("debug.attach"),
                                           QObject::tr("Attach to Process..."), appSettings,
                                           actions);
    QObject::connect(attachAction, &QAction::triggered, window, [window, debugService, showDock]() {
        bool accepted = false;
        // A number, not a process list: enumerating processes portably is
        // three implementations and a permissions story, and the pid is
        // something the user already has in the terminal they got it from.
        const int pid = QInputDialog::getInt(window, QObject::tr("Attach to Process"),
                                              QObject::tr("Process id:"), 0, 1, 2147483647, 1,
                                              &accepted);
        if (accepted && pid > 0) {
            showDock();
            debugService->attach(static_cast<quint32>(pid));
        }
    });

    QAction *attachRemoteAction =
      registerAction(debugMenu, QStringLiteral("debug.attachRemote"),
                      QObject::tr("Attach to Remote..."), appSettings, actions);
    QObject::connect(attachRemoteAction, &QAction::triggered, window,
                      [window, debugService, showDock]() {
                          bool accepted = false;
                          // Prefilled with wherever this project attached
                          // last: a debug server's address rarely changes,
                          // and `DebugService` is the one that remembers.
                          const QString target = QInputDialog::getText(
                            window, QObject::tr("Attach to Remote"),
                            QObject::tr("Debug server (host:port):"), QLineEdit::Normal,
                            debugService->lastRemoteTarget(), &accepted);
                          if (!accepted) {
                              return;
                          }
                          const qsizetype colon = target.lastIndexOf(QLatin1Char(':'));
                          if (colon <= 0) {
                              return;
                          }
                          showDock();
                          debugService->attachRemote(target.left(colon),
                                                      target.mid(colon + 1).toUInt());
                      });

    debugMenu->addSeparator();

    QAction *toggleBreakpoint = registerAction(debugMenu, QStringLiteral("debug.toggleBreakpoint"),
                                                QObject::tr("Toggle Breakpoint"), appSettings,
                                                actions);
    QObject::connect(toggleBreakpoint, &QAction::triggered, editorTabs, [editorTabs]() {
        auto *editor = qobject_cast<CodeEditor *>(editorTabs->currentEditor());
        if (!editor) {
            return;
        }
        editorTabs->toggleBreakpointAt(editor, editor->textCursor().blockNumber());
    });

    // R5: run the current session to the caret's line without a permanent
    // breakpoint there.
    QAction *runToCursor = registerAction(debugMenu, QStringLiteral("debug.runToCursor"),
                                           QObject::tr("Run to Cursor"), appSettings, actions);
    QObject::connect(runToCursor, &QAction::triggered, editorTabs,
                      [debugService, debugPanel, editorTabs]() {
                          const quint64 sessionId = debugPanel->currentSession();
                          auto *editor = qobject_cast<CodeEditor *>(editorTabs->currentEditor());
                          const QString path = editorTabs->currentPath();
                          if (sessionId == 0 || !editor || path.isEmpty()) {
                              return;
                          }
                          debugService->runToCursor(
                            sessionId, path,
                            static_cast<quint32>(editor->textCursor().blockNumber() + 1));
                      });

    // R5: the Edit Breakpoint dialog for whatever is on the caret's line —
    // adding one there if there is none yet, the same as the gutter menu.
    QAction *editBreakpoint = registerAction(debugMenu, QStringLiteral("debug.editBreakpoint"),
                                              QObject::tr("Edit Breakpoint..."), appSettings,
                                              actions);
    QObject::connect(editBreakpoint, &QAction::triggered, editorTabs,
                      [debugService, editorTabs, window]() {
                          auto *editor = qobject_cast<CodeEditor *>(editorTabs->currentEditor());
                          const QString path = editorTabs->currentPath();
                          if (!editor || path.isEmpty()) {
                              return;
                          }
                          const quint32 line =
                            static_cast<quint32>(editor->textCursor().blockNumber() + 1);
                          if (!debugService->breakpointLines(path)
                                 .split(QLatin1Char('\n'), Qt::SkipEmptyParts)
                                 .contains(QString::number(line))) {
                              debugService->toggleBreakpoint(path, line);
                          }
                          showBreakpointDialog(window, debugService, path, line);
                      });

    // R5: every breakpoint in the project, editable in place.
    QAction *viewBreakpoints = registerAction(debugMenu, QStringLiteral("debug.viewBreakpoints"),
                                               QObject::tr("View Breakpoints..."), appSettings,
                                               actions);
    QObject::connect(viewBreakpoints, &QAction::triggered, window, [debugService, editorTabs,
                                                                      window]() {
        showBreakpointsWindow(window, debugService, [editorTabs](const QString &path, int line,
                                                                   int column) {
            editorTabs->openFileAtLine(path, line, column);
        });
    });

    QAction *muteBreakpoints = registerAction(debugMenu, QStringLiteral("debug.muteBreakpoints"),
                                               QObject::tr("Mute Breakpoints"), appSettings,
                                               actions);
    muteBreakpoints->setCheckable(true);
    muteBreakpoints->setChecked(debugService->muted());
    QObject::connect(muteBreakpoints, &QAction::toggled, debugService,
                      [debugService](bool muted) { debugService->setMuted(muted); });

    // Enabled only where the adapter can actually do it (D4-4) — the
    // answer is `canReloadClasses`', re-asked each time the menu opens
    // because it depends on which session is current.
    QAction *reloadClasses = registerAction(debugMenu, QStringLiteral("debug.reloadClasses"),
                                             QObject::tr("Reload Changed Classes"), appSettings,
                                             actions);
    QObject::connect(debugMenu, &QMenu::aboutToShow, reloadClasses,
                      [reloadClasses, debugService, debugPanel]() {
                          reloadClasses->setEnabled(
                            debugService->canReloadClasses(debugPanel->currentSession()));
                      });
    QObject::connect(reloadClasses, &QAction::triggered, debugService,
                      [debugService, debugPanel]() {
                          debugService->reloadClasses(debugPanel->currentSession());
                      });

    // The adapter decides which exception classes exist, so the submenu is
    // rebuilt from `exceptionFilters` each time it opens rather than being
    // populated once from a list this view invented (D4-3).
    QMenu *exceptionMenu = debugMenu->addMenu(QObject::tr("Exception Breakpoints"));
    QObject::connect(exceptionMenu, &QMenu::aboutToShow, exceptionMenu,
                      [exceptionMenu, debugService, debugPanel]() {
                          exceptionMenu->clear();
                          const QString filters =
                            debugService->exceptionFilters(debugPanel->currentSession());
                          if (filters.isEmpty()) {
                              QAction *none = exceptionMenu->addAction(
                                QObject::tr("No debug session, or the adapter offers none"));
                              none->setEnabled(false);
                              return;
                          }
                          for (const QString &line :
                               filters.split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
                              const QStringList parts = line.split(QLatin1Char('\t'));
                              if (parts.size() < 3) {
                                  continue;
                              }
                              QAction *filter = exceptionMenu->addAction(parts.at(1));
                              filter->setCheckable(true);
                              filter->setChecked(parts.at(2) == QLatin1String("true"));
                              const QString id = parts.at(0);
                              QObject::connect(filter, &QAction::toggled, debugService,
                                                [debugService, id](bool on) {
                                                    debugService->setExceptionFilter(id, on);
                                                });
                          }
                      });

    QAction *viewDebugAction = registerAction(viewMenu, QStringLiteral("view.debug"),
                                               QObject::tr("Debug"), appSettings, actions);
    QObject::connect(viewDebugAction, &QAction::triggered, window, showDock);
}

} // namespace ui_shell
