#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>
#include <functional>

class QAction;
class QCheckBox;
class QComboBox;
class QLabel;
class QLineEdit;
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
    void refreshDependencyScopes();
    void runNode(const QString &nodeId, const QString &extraArgs);
    void showContextMenu(const QPoint &pos);

    BuildToolsService *buildToolsService_;
    RunService *runService_;
    OpenAt openAt_;
    OpenSettingsHandler openSettings_;

    QTreeWidget *tree_;
    QLineEdit *executeEdit_;
    QToolButton *offlineButton_;
    QToolButton *skipTestsButton_;
    // D8's dependency analyzer: the Dependencies subtree's scope filter
    // and "Conflicts only" toggle. A second toolbar row rather than
    // crowding the first (review-fix-round-6's own reasoning: the first
    // row is already tight on a right-side dock's width, and these two
    // controls matter only while looking at Dependencies).
    QComboBox *dependencyScopeCombo_;
    QCheckBox *conflictsOnlyCheck_;
    QLabel *statusLabel_;
    // D8 (screenshot review): shown centered in place of `tree_` for a
    // project with nothing to show yet — either no Gradle/Maven marker
    // found at all, or one found but not synced. A separate label from
    // `statusLabel_`, which stays the sync-failure banner alone
    // (`refreshBanner`'s own doc comment) — the two used to share one
    // label and fight over its visibility every time both had something to
    // say.
    QLabel *emptyStateLabel_;
};

// `buildBuildToolsDock` (B2): copies `tests_panel.cpp`'s own
// `buildTestsDock` construction — a right-side dock, hidden by default,
// registered with the id `buildTools`.
BuildToolsPanel *buildBuildToolsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                      ads::CDockAreaWidget *relativeTo,
                                      BuildToolsService *buildToolsService, RunService *runService,
                                      BuildToolsPanel::OpenAt openAt);

} // namespace ui_shell
