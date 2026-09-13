#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

#include <functional>

class QTableWidget;
class QTabWidget;
class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {

class TerminalWidget;

// The Containers dock's per-container detail tabs (C3), added into the
// same `QTabWidget` `ContainersPanel` already shows the Dashboard in (its
// tab 0, permanent, untouched by this class): Log (auto-opens for a
// container node, "Restart" re-runs it), Processes and Files (opened on
// request from the context menu), plus however many closable Terminal/
// Exec/Attach tabs the user has open — each just a `TerminalWidget` over a
// `container_core::session` command, the same PTY plumbing
// `TerminalSessionsPanel` already uses for a local shell.
//
// Humble view: which actions apply, a directory's entries, a process
// table's rows and every command's argv are `ContainerService`'s answers;
// this only lays them out and wires clicks back into it.
class ContainerDetailArea : public QObject
{
    Q_OBJECT

public:
    using OpenAt = std::function<void(const QString &, int, int)>;

    ContainerDetailArea(ContainerService *containerService, TerminalSupervisor *terminalSupervisor,
                        AppSettings *appSettings, OpenAt openAt, QTabWidget *tabs,
                        QObject *parent = nullptr);

    // Called on every tree selection change. Opens/replaces the Log tab
    // when `kind` is `"container"`; otherwise closes whatever this class
    // still has open for the previous one (Log/Processes/Files — Terminal/
    // Exec/Attach tabs the user opened stay, since they are independent
    // sessions the user chose to keep).
    void onSelectionChanged(const QString &nodeId, const QString &kind);
    void clearSelection();

    // Context-menu actions for the currently selected container.
    void openTerminal(bool asRoot);
    void openExecDialog(QWidget *dialogParent);
    void openAttach();
    void showProcesses();
    void showFiles();

private:
    void openOrReplaceLogTab();
    void closeLogTab();
    void addTerminalTab(const FfiCommand &command, const QString &title);
    void closeTerminalTab(int index);

    void refreshProcesses();
    void ensureFilesTab();
    void populateFilesRoot();
    void requestChildren(QTreeWidgetItem *dirItem, const QString &dir);
    void downloadPrompt(const QString &path, QWidget *dialogParent);

    ContainerService *containerService_;
    TerminalSupervisor *terminalSupervisor_;
    AppSettings *appSettings_;
    OpenAt openAt_;
    QTabWidget *tabs_;

    QString nodeId_;
    bool isContainer_ = false;

    QWidget *logPage_ = nullptr;
    TerminalWidget *logWidget_ = nullptr;

    QWidget *processesPage_ = nullptr;
    QTableWidget *processesTable_ = nullptr;

    QWidget *filesPage_ = nullptr;
    QTreeWidget *filesTree_ = nullptr;
};

} // namespace ui_shell
