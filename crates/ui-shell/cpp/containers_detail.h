#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

#include <functional>

class QLabel;
class QListWidget;
class QListWidgetItem;
class QPushButton;
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

    // The panel's own generic Name/ID/Status/Details page (tab 0) — this
    // class swaps it out for a per-kind Dashboard when an image/network/
    // volume is selected, and restores it otherwise.
    void setGenericDashboardPage(QWidget *page);

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

    // C4: an image's Layers (`history`) and any image/network/volume's
    // Labels — opened on request, same lazy-tab shape as Processes/Files.
    void showLayers();
    void showLabels();
    // C9: the Layers tab's "Analyze image" button — a per-layer tree of
    // path/size/kind, from a headers-only `save` tar walk.
    void triggerAnalyzeImage();

    // C4: the Images console's Pull button and the Pull toolbar action —
    // a closable "Pull: <reference>" `TerminalWidget` tab, not tied to
    // whatever node happens to be selected.
    void openPullTab(const QString &connectionId, const QString &reference);

    // C7: a closable, titled terminal tab for a command built entirely
    // outside this class (a registry pull/push) — the same
    // `addTerminalTab` every tab above already goes through, exposed
    // publicly since C7's caller has no `nodeId_`-scoped node to select
    // through `onSelectionChanged` first.
    void openTerminalCommandTab(const FfiCommand &command, const QString &title);

signals:
    // A "containers using it" row was clicked on an image/network/volume
    // Dashboard — the panel selects that container node in the tree.
    void containerNodeRequested(const QString &nodeId);

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

    // C4: per-kind Dashboard (tab 0), swapped in place of the generic page
    // for image/network/volume nodes.
    void updateDashboardTab();
    QWidget *ensureImageDashboardPage();
    QWidget *ensureNetworkDashboardPage();
    QWidget *ensureVolumeDashboardPage();
    void populateImageDashboard();
    void populateNetworkDashboard();
    void populateVolumeDashboard();
    // `containers` is `\n`-joined `"<name>\t<node id>"` pairs
    // (`FfiImageDashboard::containers`'s own convention).
    void fillContainersList(QListWidget *list, const QString &containers);

    ContainerService *containerService_;
    TerminalSupervisor *terminalSupervisor_;
    AppSettings *appSettings_;
    OpenAt openAt_;
    QTabWidget *tabs_;

    QString nodeId_;
    QString kind_;
    bool isContainer_ = false;

    QWidget *logPage_ = nullptr;
    TerminalWidget *logWidget_ = nullptr;

    QWidget *processesPage_ = nullptr;
    QTableWidget *processesTable_ = nullptr;

    QWidget *filesPage_ = nullptr;
    QTreeWidget *filesTree_ = nullptr;

    QWidget *layersPage_ = nullptr;
    QTableWidget *layersTable_ = nullptr;
    QPushButton *analyzeImageButton_ = nullptr;
    // C9: per-layer path/size/kind, populated from `layerFsReady`. A top-
    // level item per layer id, one child per entry — no per-entry open/
    // download yet (a documented gap; see the PR description).
    QTreeWidget *layerFsTree_ = nullptr;

    QWidget *labelsPage_ = nullptr;
    QTableWidget *labelsTable_ = nullptr;

    // C4: the generic page (owned by `ContainersPanel`) plus the three
    // lazily-built, per-kind ones. At most one of the four sits in tab 0
    // at a time; the other three stay parented to `tabs_` but hidden.
    QWidget *genericDashboardPage_ = nullptr;

    QWidget *imageDashboardPage_ = nullptr;
    QLabel *imageDashName_ = nullptr;
    QLabel *imageDashId_ = nullptr;
    QLabel *imageDashSize_ = nullptr;
    QLabel *imageDashCreated_ = nullptr;
    QListWidget *imageDashTags_ = nullptr;
    QListWidget *imageDashDigests_ = nullptr;
    QListWidget *imageDashContainers_ = nullptr;

    QWidget *networkDashboardPage_ = nullptr;
    QLabel *networkDashName_ = nullptr;
    QLabel *networkDashId_ = nullptr;
    QLabel *networkDashDriver_ = nullptr;
    QLabel *networkDashScope_ = nullptr;
    QListWidget *networkDashSubnets_ = nullptr;
    QListWidget *networkDashContainers_ = nullptr;
    QTableWidget *networkDashLabels_ = nullptr;

    QWidget *volumeDashboardPage_ = nullptr;
    QLabel *volumeDashName_ = nullptr;
    QLabel *volumeDashDriver_ = nullptr;
    QLabel *volumeDashMountpoint_ = nullptr;
    QListWidget *volumeDashContainers_ = nullptr;
    QTableWidget *volumeDashLabels_ = nullptr;
};

} // namespace ui_shell
