#include "coverage_panel.h"

#include "dock_layout.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QDir>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kAbsPathRole = Qt::UserRole;
constexpr int kIsFileRole = Qt::UserRole + 1;

QString parentOf(const QString &relativePath)
{
    const int slash = relativePath.lastIndexOf(QLatin1Char('/'));
    return slash < 0 ? QString() : relativePath.left(slash);
}

QString nameOf(const QString &relativePath)
{
    return relativePath.mid(relativePath.lastIndexOf(QLatin1Char('/')) + 1);
}

} // namespace

CoveragePanel::CoveragePanel(TestService *testService, OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , testService_(testService)
  , openAt_(std::move(openAt))
{
    auto *runButton = new QToolButton(this);
    runButton->setText(tr("Run All with Coverage"));
    auto *clearButton = new QToolButton(this);
    clearButton->setText(tr("Clear"));
    statusLabel_ = new QLabel(this);

    auto *toolbar = new QHBoxLayout();
    toolbar->addWidget(runButton);
    toolbar->addWidget(clearButton);
    toolbar->addWidget(statusLabel_, 1);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(2);
    tree_->setHeaderLabels({tr("Element"), tr("Lines covered")});
    tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    tree_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    tree_->setUniformRowHeights(true);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addLayout(toolbar);
    layout->addWidget(tree_, 1);

    connect(runButton, &QToolButton::clicked, this, [this]() {
        const FfiResult result = testService_->runAllWithCoverage();
        if (result.code != 0) {
            statusLabel_->setText(QString(result.message));
        }
    });
    connect(clearButton, &QToolButton::clicked, this, [this]() { testService_->clearCoverage(); });
    connect(tree_, &QTreeWidget::itemActivated, this, [this](QTreeWidgetItem *item) {
        if (item->data(0, kIsFileRole).toBool()) {
            openAt_(item->data(0, kAbsPathRole).toString(), 0, 0);
        }
    });
    connect(testService_, &TestService::coverageChanged, this, &CoveragePanel::refresh);
    refresh();
}

void CoveragePanel::refresh()
{
    tree_->clear();
    QHash<QString, QTreeWidgetItem *> itemsByPath;
    // `coverageRows` lists a directory before everything under it.
    for (const FfiCoverageRow &row : testService_->coverageRows()) {
        const QString path(row.path);
        QTreeWidgetItem *parent = itemsByPath.value(parentOf(path), nullptr);
        auto *item = parent != nullptr ? new QTreeWidgetItem(parent) : new QTreeWidgetItem(tree_);
        item->setText(0, path.isEmpty() ? tr("All files") : nameOf(path));
        if (row.hasLines) {
            item->setText(1, tr("%1% (%2/%3)").arg(row.percent, 0, 'f', 1).arg(row.covered).arg(row.total));
        } else {
            item->setText(1, QStringLiteral("\u2014"));
            item->setToolTip(1, tr("No executable lines"));
        }
        item->setData(0, kAbsPathRole, QString(row.absPath));
        item->setData(0, kIsFileRole, row.isFile);
        itemsByPath.insert(path, item);
    }
    tree_->expandToDepth(1);
    statusLabel_->setText(itemsByPath.isEmpty() ? tr("No coverage collected yet.") : QString());
}

CoveragePanel *buildCoverageDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                 ads::CDockAreaWidget *relativeTo, TestService *testService,
                                 CoveragePanel::OpenAt openAt)
{
    auto *panel = new CoveragePanel(testService, std::move(openAt), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Coverage"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("coverage"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("coverage"));
    QObject::connect(testService, &TestService::coverageChanged, panel, [docks, testService]() {
        if (!testService->coverageRows().empty()) {
            docks->show(QStringLiteral("coverage"));
        }
    });
    return panel;
}

} // namespace ui_shell
