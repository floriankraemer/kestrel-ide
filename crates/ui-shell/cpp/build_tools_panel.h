#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>
#include <functional>

class QAction;
class QCheckBox;
class QLabel;
class QLineEdit;
class QPoint;
class QTreeWidget;
class QTreeWidgetItem;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;

// The Build Tools dock (the jvm-build-tools plan's B2/B3): a tree built
// from `BuildToolsService::rows()`, a toolbar (Reload / Execute… / Offline /
// Skip Tests / Settings), and a context menu to run a task/goal. The tree
// SHAPE is `jvm_build_core::view::rows`'s (Qt-free, tested); this class only
// paints whatever `rows()` returns and never groups/orders a row itself —
// see `docs/architecture/layering.md`'s "cpp/ is a humble view" rule.
class BuildToolsPanel : public QWidget
{
    Q_OBJECT

public:
    using OpenSettingsHandler = std::function<void()>;
    using OpenAt = std::function<void(const QString &, int, int)>;

    BuildToolsPanel(BuildToolsService *buildToolsService, RunService *runService,
                     OpenAt openAt, QWidget *parent);

    void setOpenSettingsHandler(OpenSettingsHandler handler) { openSettings_ = std::move(handler); }

private:
    void refreshTree();
    void refreshTitle();
    void refreshBanner();
    void runNode(const QString &nodeId, const QString &extraArgs);
    void showContextMenu(const QPoint &pos);

    BuildToolsService *buildToolsService_;
    RunService *runService_;
    OpenAt openAt_;
    OpenSettingsHandler openSettings_;

    QTreeWidget *tree_;
    QLineEdit *executeEdit_;
    QCheckBox *offlineCheck_;
    QCheckBox *skipTestsCheck_;
    QLabel *statusLabel_;
};

// `buildBuildToolsDock` (B2): copies `tests_panel.cpp`'s own
// `buildTestsDock` construction — a right-side dock, hidden by default,
// registered with the id `buildTools`.
BuildToolsPanel *buildBuildToolsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                      ads::CDockAreaWidget *relativeTo,
                                      BuildToolsService *buildToolsService, RunService *runService,
                                      BuildToolsPanel::OpenAt openAt);

} // namespace ui_shell
