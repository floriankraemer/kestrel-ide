#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

#include <functional>

class QLabel;
class QTreeWidget;

namespace ads {
class CDockAreaWidget;
class CDockManager;
} // namespace ads

namespace ui_shell {

class DockRegistry;

// The Coverage dock (PHP parity plan T5): the last coverage run's
// directory/file tree with the share of covered lines.
//
// Humble view: the rows, their rollups and percentages are `test-core`'s
// (`coverage::Coverage::rows`), read back through `TestService`; this
// widget lays them out and forwards a click.
class CoveragePanel : public QWidget
{
public:
    using OpenAt = std::function<void(const QString &, int, int)>;

    CoveragePanel(TestService *testService, OpenAt openAt, QWidget *parent);

    // Rebuild the tree from `TestService::coverageRows()`.
    void refresh();

private:
    TestService *testService_;
    OpenAt openAt_;
    QLabel *statusLabel_ = nullptr;
    QTreeWidget *tree_ = nullptr;
};

// Builds the panel, wraps it in a dock registered under id `"coverage"`,
// and shows it whenever a coverage run delivers a report.
CoveragePanel *buildCoverageDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                 ads::CDockAreaWidget *relativeTo, TestService *testService,
                                 CoveragePanel::OpenAt openAt);

} // namespace ui_shell
