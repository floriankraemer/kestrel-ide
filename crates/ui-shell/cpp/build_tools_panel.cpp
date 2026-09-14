#include "build_tools_panel.h"

#include "dock_layout.h"
#include "e2e_mark.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAction>
#include <QApplication>
#include <QClipboard>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QHash>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QMenu>
#include <QMessageBox>
#include <QScrollBar>
#include <QShowEvent>
#include <QSignalBlocker>
#include <QSizePolicy>
#include <QStyle>
#include <QTimer>
#include <QToolButton>
#include <QTreeWidget>
#include <QTreeWidgetItemIterator>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kIdRole = Qt::UserRole;
constexpr int kToolRole = Qt::UserRole + 1;
constexpr int kRunnableRole = Qt::UserRole + 2;
constexpr int kBuildFileRole = Qt::UserRole + 3;
constexpr int kIsProfileRole = Qt::UserRole + 4;

// A row icon per kind (review fix 3, pixel scrutiny): plain platform-style
// icons, the same "no vendored asset for a handful of kinds" call
// `tests_panel.cpp`'s own status dot makes, rather than the project tree's
// icon-theme pipeline — that pipeline resolves a *file's* icon from its
// path/language, which a Gradle task or a Maven scope has neither of.
QIcon iconForKind(FfiBuildToolNodeKind kind)
{
    QStyle *style = QApplication::style();
    switch (kind) {
    case FfiBuildToolNodeKind::ToolRoot:
        return style->standardIcon(QStyle::SP_DriveHDIcon);
    case FfiBuildToolNodeKind::Group:
        return style->standardIcon(QStyle::SP_DirIcon);
    case FfiBuildToolNodeKind::Task:
        return style->standardIcon(QStyle::SP_MediaPlay);
    case FfiBuildToolNodeKind::Module:
        return style->standardIcon(QStyle::SP_DirClosedIcon);
    case FfiBuildToolNodeKind::SourceRoot:
        return style->standardIcon(QStyle::SP_FileIcon);
    case FfiBuildToolNodeKind::Dependency:
        return style->standardIcon(QStyle::SP_FileDialogDetailedView);
    case FfiBuildToolNodeKind::Profile:
        return QIcon();
    case FfiBuildToolNodeKind::Plugin:
        return style->standardIcon(QStyle::SP_DriveNetIcon);
    case FfiBuildToolNodeKind::Goal:
        return style->standardIcon(QStyle::SP_ArrowRight);
    }
    return QIcon();
}

// Icon-only, tooltip-carries-the-meaning toolbar buttons — the same shape
// `run_toolbar.cpp`'s own `makeGlyphButton` uses for its Run/Stop/Rerun
// cluster, reused here rather than invented fresh (review fix 3,
// follow-up): a right-side dock is narrower than a bottom one, and Maven's
// toolbar has one more toggle than Gradle's, so a row of full-text buttons
// clips under that width pressure regardless of any one widget's own
// minimum width — the fix is fewer pixels demanded, not a floor that only
// moves the squeeze to whichever button is still text.
QToolButton *glyphButton(QStyle::StandardPixmap icon, const QString &tooltip, QWidget *parent)
{
    auto *button = new QToolButton(parent);
    button->setIcon(QApplication::style()->standardIcon(icon));
    button->setToolTip(tooltip);
    button->setAutoRaise(true);
    return button;
}

// Same shape, checkable: the Offline/Skip Tests toggles need a pressed
// state, not just a click — `changes_toolbar.cpp`'s own icon-only,
// tooltip-carries-the-meaning buttons (its `refreshButton_`) are the
// precedent this follows, `setCheckable` the only addition a toggle needs.
QToolButton *checkableGlyphButton(QStyle::StandardPixmap icon, const QString &tooltip,
                                   QWidget *parent)
{
    QToolButton *button = glyphButton(icon, tooltip, parent);
    button->setCheckable(true);
    return button;
}

// Row rects for a headless E2E driver to click precisely instead of
// guessing coordinates — `project_tree_dock.cpp`'s own `project_tree_row`
// marker, reused here at this tree's much smaller scale (no coalescing
// timer: a Build Tools tree tops out at a few dozen rows, not a whole
// project's worth of files).
void markVisibleRows(QTreeWidget *tree)
{
    for (QTreeWidgetItemIterator it(tree, QTreeWidgetItemIterator::NotHidden); *it; ++it) {
        QTreeWidgetItem *item = *it;
        const QRect rect = tree->visualItemRect(item);
        const QPoint origin = tree->viewport()->mapToGlobal(rect.topLeft());
        e2eMark(QStringLiteral("{\"ev\":\"build_tools_row\",\"id\":%1,\"rect\":[%2,%3,%4,%5]}")
                  .arg(e2eJson(item->data(0, kIdRole).toString()))
                  .arg(origin.x())
                  .arg(origin.y())
                  .arg(rect.width())
                  .arg(rect.height()));
    }
}

// A row's geometry is not valid the instant it is inserted or
// expanded/collapsed — `QTreeWidget` defers that layout pass to the next
// trip round the event loop — so `markVisibleRows` needs to run one turn
// later, `project_tree_dock.cpp`'s own reason for coalescing its equivalent
// report onto a zero-interval `QTimer` rather than calling it inline.
void scheduleRowMarkers(QTreeWidget *tree)
{
    QTimer::singleShot(0, tree, [tree]() { markVisibleRows(tree); });
}

QString titleFor(FfiBuildToolTitleKind kind)
{
    switch (kind) {
    case FfiBuildToolTitleKind::Gradle:
        return QObject::tr("Gradle");
    case FfiBuildToolTitleKind::Maven:
        return QObject::tr("Maven");
    case FfiBuildToolTitleKind::Both:
        return QObject::tr("Gradle & Maven");
    case FfiBuildToolTitleKind::None:
        break;
    }
    return QObject::tr("Build Tools");
}

} // namespace

BuildToolsPanel::BuildToolsPanel(BuildToolsService *buildToolsService, RunService *runService,
                                  OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , buildToolsService_(buildToolsService)
  , runService_(runService)
  , openAt_(std::move(openAt))
{
    auto *reloadButton =
      glyphButton(QStyle::SP_BrowserReload, tr("Reload All Gradle/Maven Projects"), this);
    executeEdit_ = new QLineEdit(this);
    executeEdit_->setPlaceholderText(tr("Execute…"));
    // Expanding (not a bare stretch factor) so Execute is the row's own
    // pressure-release valve: every other control here is icon-only with a
    // fixed natural width, so the field is the one thing free to shrink
    // before a fixed-width sibling ever would, matching `find_bar.cpp`'s
    // "the field gives up space last" rule while still keeping a floor.
    executeEdit_->setSizePolicy(QSizePolicy::Expanding, QSizePolicy::Fixed);
    executeEdit_->setMinimumWidth(90);
    auto *runButton = glyphButton(QStyle::SP_MediaPlay, tr("Run"), this);
    // Checkable icon-only toggles, not text checkboxes: `changes_toolbar.cpp`'s
    // own icon-only `refreshButton_` is the precedent (review fix 4) — a row
    // of text checkboxes plus three more buttons overflows a right-side
    // dock's width, especially Maven's, which carries one toggle more than
    // Gradle's.
    offlineButton_ =
      checkableGlyphButton(QStyle::SP_DriveNetIcon, tr("Toggle Offline Mode"), this);
    skipTestsButton_ =
      checkableGlyphButton(QStyle::SP_MediaSkipForward, tr("Toggle Skip Tests"), this);
    auto *settingsButton =
      glyphButton(QStyle::SP_FileDialogDetailedView, tr("Settings…"), this);

    auto *toolbar = new QHBoxLayout();
    toolbar->addWidget(reloadButton);
    toolbar->addWidget(executeEdit_, 1);
    toolbar->addWidget(runButton);
    toolbar->addWidget(offlineButton_);
    toolbar->addWidget(skipTestsButton_);
    toolbar->addWidget(settingsButton);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(2);
    tree_->setHeaderLabels({tr("Name"), tr("Detail")});
    tree_->header()->setSectionResizeMode(0, QHeaderView::Stretch);
    // Detail sized to its own content rather than the default Interactive
    // width (an arbitrary starting size that otherwise fights Name, the
    // stretch column, for space on a narrow dock) — `problems_panel.cpp`'s
    // own convention for a wide stretch column beside narrower fixed ones.
    tree_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    // `QHeaderView`'s own default: the LAST section stretches to fill
    // whatever the header's own resize modes leave over, regardless of what
    // mode that section was just given — which was quietly overriding
    // Detail's `ResizeToContents` above and starving Name (review fix 2,
    // root cause). `plugins_page.cpp`/`languages_page.cpp` already turn this
    // off for the same reason; this tree needs it too since Detail, not
    // Name, is its last column.
    tree_->header()->setStretchLastSection(false);
    tree_->setContextMenuPolicy(Qt::CustomContextMenu);

    statusLabel_ = new QLabel(this);
    statusLabel_->setWordWrap(true);

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addLayout(toolbar);
    layout->addWidget(statusLabel_);
    layout->addWidget(tree_, 1);

    connect(reloadButton, &QToolButton::clicked, this,
            [this]() { buildToolsService_->sync(); });
    connect(runButton, &QToolButton::clicked, this, [this]() {
        if (QTreeWidgetItem *item = tree_->currentItem()) {
            runNode(item->data(0, kIdRole).toString(), executeEdit_->text());
        }
    });
    connect(tree_, &QTreeWidget::itemDoubleClicked, this, [this](QTreeWidgetItem *item, int) {
        if (item->data(0, kRunnableRole).toBool()) {
            runNode(item->data(0, kIdRole).toString(), QString());
        }
    });
    connect(tree_, &QTreeWidget::itemChanged, this, [this](QTreeWidgetItem *item, int column) {
        if (column != 0 || !item->data(0, kIsProfileRole).toBool()) {
            return;
        }
        buildToolsService_->setProfileChecked(item->text(0), item->checkState(0) == Qt::Checked);
    });
    connect(tree_, &QTreeWidget::customContextMenuRequested, this,
            &BuildToolsPanel::showContextMenu);
    // Every row below an expanded/collapsed one moves, so a driver relying
    // on `build_tools_row`'s rects needs a fresh report after either — the
    // same reason `project_tree_dock.cpp` re-marks on `expanded`/`collapsed`.
    connect(tree_, &QTreeWidget::itemExpanded, this, [this]() { scheduleRowMarkers(tree_); });
    connect(tree_, &QTreeWidget::itemCollapsed, this, [this]() { scheduleRowMarkers(tree_); });
    // Scrolling moves every row's rect exactly as much as expanding one
    // does, so a driver relying on `build_tools_row` needs a fresh report
    // after this too, not just after expand/collapse.
    connect(tree_->verticalScrollBar(), &QScrollBar::valueChanged, this,
            [this]() { scheduleRowMarkers(tree_); });
    connect(offlineButton_, &QToolButton::toggled, this,
            [this](bool on) { buildToolsService_->setOffline(on); });
    connect(skipTestsButton_, &QToolButton::toggled, this,
            [this](bool on) { buildToolsService_->setSkipTests(on); });
    connect(settingsButton, &QToolButton::clicked, this, [this]() {
        if (openSettings_) {
            openSettings_();
        }
    });

    connect(buildToolsService_, &BuildToolsService::modelChanged, this, [this]() {
        refreshTree();
        refreshTitle();
    });
    connect(buildToolsService_, &BuildToolsService::syncStateChanged, this,
            &BuildToolsPanel::refreshTitle);
    connect(buildToolsService_, &BuildToolsService::bannerChanged, this,
            &BuildToolsPanel::refreshBanner);

    refreshTree();
    refreshTitle();
    refreshBanner();
}

void BuildToolsPanel::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    scheduleRowMarkers(tree_);
}

void BuildToolsPanel::refreshTitle()
{
    const FfiBuildToolTitleKind title = buildToolsService_->titleKind();
    // Skip Tests only means something for Maven (`-DskipTests`); Gradle's
    // own toggle above already covers the "skip test task" case through
    // `-x test` regardless, so hiding this one for a pure-Gradle project
    // avoids a control that reads as redundant.
    skipTestsButton_->setVisible(title == FfiBuildToolTitleKind::Maven
                                  || title == FfiBuildToolTitleKind::Both);
}

void BuildToolsPanel::refreshBanner()
{
    // The editor banner (B4) owns the trust/reload prompts; this label only
    // reports a sync failure inline, since it is already visible when the
    // dock is open and the banner's own dismissal should not hide that.
    if (buildToolsService_->syncStateKind() == FfiSyncStateKind::Failed) {
        statusLabel_->setText(buildToolsService_->syncMessage());
        statusLabel_->setVisible(true);
    } else {
        statusLabel_->setVisible(false);
    }
}

void BuildToolsPanel::refreshTree()
{
    // Rebuilding sets every profile row's check state from the model, which
    // would otherwise re-fire `itemChanged` back into `setProfileChecked`
    // and loop: that slot calls `modelChanged`, which is exactly the signal
    // that reaches this function.
    const QSignalBlocker blocker(tree_);
    QHash<QString, QTreeWidgetItem *> itemsById;
    QHash<QString, bool> expandedById;
    for (auto *item : tree_->findItems(QString(), Qt::MatchContains | Qt::MatchRecursive)) {
        expandedById.insert(item->data(0, kIdRole).toString(), item->isExpanded());
    }
    tree_->clear();

    const ::rust::Vec<FfiBuildToolNode> rows = buildToolsService_->rows();
    if (rows.empty()) {
        statusLabel_->setText(
          tr("No Gradle or Maven project detected, or it has not been loaded yet."));
        statusLabel_->setVisible(buildToolsService_->syncStateKind() != FfiSyncStateKind::Failed);
        return;
    }

    for (const FfiBuildToolNode &node : rows) {
        const QString id = QString(node.id);
        const QString parentId = QString(node.parentId);
        QTreeWidgetItem *parentItem = parentId.isEmpty() ? nullptr : itemsById.value(parentId);
        auto *item = parentItem ? new QTreeWidgetItem(parentItem) : new QTreeWidgetItem(tree_);
        item->setText(0, QString(node.label));
        item->setIcon(0, iconForKind(node.kind));
        // Review fix 3: the root row's label is the project name, not its
        // path. The path stays reachable as the tooltip only — putting it in
        // the Detail column too is what was squeezing the Name column down
        // to "gradle-si…" in the first place (review fix 2): an absolute
        // path is the longest string this tree ever shows, so handing it to
        // the `ResizeToContents` column makes that column claim most of a
        // narrow dock's width for one row, starving every other row's Name.
        if (node.kind == FfiBuildToolNodeKind::ToolRoot) {
            item->setToolTip(0, QString(node.detail));
        } else {
            item->setText(1, QString(node.detail));
        }
        item->setData(0, kIdRole, id);
        item->setData(0, kToolRole, QString(node.tool));
        item->setData(0, kRunnableRole, node.kind == FfiBuildToolNodeKind::Task);
        item->setData(0, kBuildFileRole, QString(node.buildFile));
        if (node.kind == FfiBuildToolNodeKind::Profile) {
            item->setData(0, kIsProfileRole, true);
            item->setFlags(item->flags() | Qt::ItemIsUserCheckable);
            item->setCheckState(0, node.checked ? Qt::Checked : Qt::Unchecked);
        }
        item->setExpanded(expandedById.value(id, node.kind == FfiBuildToolNodeKind::ToolRoot));
        itemsById.insert(id, item);
    }
    e2eMark(QStringLiteral("{\"ev\":\"build_tools_model_changed\",\"nodes\":%1}")
              .arg(static_cast<int>(rows.size())));
    scheduleRowMarkers(tree_);
}

void BuildToolsPanel::runNode(const QString &nodeId, const QString &extraArgs)
{
    const FfiRunConfig config = extraArgs.trimmed().isEmpty()
                                  ? buildToolsService_->taskConfig(nodeId)
                                  : buildToolsService_->taskConfigWithArgs(nodeId, extraArgs);
    if (QString(config.program).isEmpty()) {
        return;
    }
    const FfiResult result = runService_->runTemporary(config);
    if (result.code != 0) {
        QMessageBox::warning(this, tr("Run"), result.message);
    }
}

void BuildToolsPanel::showContextMenu(const QPoint &pos)
{
    QTreeWidgetItem *item = tree_->itemAt(pos);
    if (!item) {
        return;
    }
    const QString nodeId = item->data(0, kIdRole).toString();
    const bool runnable = item->data(0, kRunnableRole).toBool();
    const QString buildFile = item->data(0, kBuildFileRole).toString();
    if (!runnable && buildFile.isEmpty()) {
        return;
    }

    QMenu menu(tree_);
    QAction *run = runnable ? menu.addAction(tr("Run")) : nullptr;
    QAction *runWithArgs = runnable ? menu.addAction(tr("Run with Arguments…")) : nullptr;
    if (runnable) {
        menu.addSeparator();
    }
    QAction *openBuildFile =
      !buildFile.isEmpty() ? menu.addAction(tr("Open Build File")) : nullptr;
    QAction *copy = menu.addAction(tr("Copy Coordinate"));
    QAction *chosen = menu.exec(tree_->viewport()->mapToGlobal(pos));
    if (chosen == run) {
        runNode(nodeId, QString());
    } else if (chosen == runWithArgs) {
        runNode(nodeId, executeEdit_->text());
    } else if (chosen == openBuildFile) {
        if (openAt_) {
            openAt_(buildFile, 1, 0);
        }
    } else if (chosen == copy) {
        QGuiApplication::clipboard()->setText(item->text(0));
    }
}

BuildToolsPanel *buildBuildToolsDock(ads::CDockManager *dockManager, DockRegistry *docks,
                                      ads::CDockAreaWidget *relativeTo,
                                      BuildToolsService *buildToolsService, RunService *runService,
                                      BuildToolsPanel::OpenAt openAt)
{
    auto *panel = new BuildToolsPanel(buildToolsService, runService, std::move(openAt), dockManager);
    auto *dock = new ads::CDockWidget(dockManager, QObject::tr("Build Tools"));
    dock->setWidget(panel);
    docks->registerDock(QStringLiteral("buildTools"), dock, ads::RightDockWidgetArea, relativeTo);
    docks->hide(QStringLiteral("buildTools"));

    // The dock's own title tracks which tool(s) synced and the sync state —
    // set from here, where `dock` is a plain local pointer, rather than
    // walking up `panel`'s parent chain to find it.
    const auto updateTitle = [dock, buildToolsService]() {
        QString text = titleFor(buildToolsService->titleKind());
        switch (buildToolsService->syncStateKind()) {
        case FfiSyncStateKind::Syncing:
            text = QObject::tr("%1 — syncing…").arg(text);
            break;
        case FfiSyncStateKind::Failed:
            text = QObject::tr("%1 — sync failed").arg(text);
            break;
        case FfiSyncStateKind::Idle:
            break;
        }
        dock->setWindowTitle(text);
    };
    updateTitle();
    QObject::connect(buildToolsService, &BuildToolsService::modelChanged, dock, updateTitle);
    QObject::connect(buildToolsService, &BuildToolsService::syncStateChanged, dock, updateTitle);

    return panel;
}

} // namespace ui_shell
