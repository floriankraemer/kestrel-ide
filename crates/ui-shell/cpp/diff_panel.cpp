#include "diff_panel.h"

#include "dock_layout.h"
#include "editor_tabs.h"
#include "styled_tab_widget.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QTabWidget>
#include <QVBoxLayout>
#include <QVariant>

namespace ui_shell {

namespace {

int indexOfTab(const QTabWidget *tabs, quint64 tabId)
{
    for (int i = 0; i < tabs->count(); ++i) {
        if (tabs->widget(i)->property("tabId").toULongLong() == tabId) {
            return i;
        }
    }
    return -1;
}

} // namespace

DiffPanel::DiffPanel(DiffClosed onClosed, QWidget *parent)
  : QWidget(parent), onClosed_(std::move(onClosed))
{
    tabs_ = new StyledTabWidget(this);
    tabs_->setTabsClosable(true);
    connect(tabs_, &QTabWidget::tabCloseRequested, this, &DiffPanel::closeTab);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(tabs_);
}

void DiffPanel::openDiff(quint64 tabId, const QString &title, QWidget *page)
{
    page->setProperty("tabId", QVariant::fromValue(tabId));
    const int index = tabs_->addTab(page, title);
    tabs_->setCurrentIndex(index);
}

bool DiffPanel::raiseIfOpen(quint64 tabId)
{
    const int index = indexOfTab(tabs_, tabId);
    if (index < 0) {
        return false;
    }
    tabs_->setCurrentIndex(index);
    return true;
}

void DiffPanel::discardDiff(quint64 tabId)
{
    const int index = indexOfTab(tabs_, tabId);
    if (index < 0) {
        return;
    }
    QWidget *page = tabs_->widget(index);
    tabs_->removeTab(index);
    delete page;
}

void DiffPanel::closeTab(int index)
{
    QWidget *page = tabs_->widget(index);
    const quint64 tabId = page->property("tabId").toULongLong();
    tabs_->removeTab(index);
    if (onClosed_) {
        onClosed_(tabId, page);
    }
    page->deleteLater();
}

void buildDiffDock(ads::CDockManager *dockManager, DockRegistry *docks,
                    ads::CDockAreaWidget *relativeTo, EditorTabs *editorTabs)
{
    auto *panel = new DiffPanel(
      [editorTabs](quint64 tabId, QWidget *page) {
          editorTabs->restoreEditorFromDiffWindow(tabId, page);
      },
      dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Diff"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("diff"), dock, ads::CenterDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("diff"));
    editorTabs->setDiffPanel(
      panel, [docks] { docks->dock(QStringLiteral("diff"))->toggleView(true); });
}

} // namespace ui_shell
