#include "project_tree_dock.h"

#include "project_tree_git_menu.h"

#include "ai_chat_panel.h"
#include "dock_layout.h"
#include "e2e_mark.h"
#include "icon_cache.h"
#include "icon_decoration_proxy.h"
#include "keymap_page.h"
#include "ui_tokens.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include "DockAreaWidget.h"
#include "DockManager.h"
#include "DockWidget.h"

#include <QAbstractItemModel>
#include <QEvent>
#include <QAction>
#include <QFileDialog>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QIcon>
#include <QInputDialog>
#include <QLineEdit>
#include <QMainWindow>
#include <QMenu>
#include <QMessageBox>
#include <QModelIndex>
#include <QPainter>
#include <QPalette>
#include <QPixmap>
#include <QPoint>
#include <QSize>
#include <QStatusBar>
#include <QStyle>
#include <QTimer>
#include <QToolButton>
#include <QTreeView>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

// ProjectTreeModel::Roles are offsets from Qt::UserRole, not role numbers —
// cxx-qt cannot give a qenum explicit discriminants, so the variants would
// otherwise collide with Qt::DecorationRole and friends. The Rust side adds
// the same base before it matches on the role.
int treeRole(ProjectTreeModel::Roles role)
{
    return Qt::UserRole + static_cast<int>(role);
}

// Turns the tree viewport's resize into the same coalesced row report every
// other layout change already produces. An event filter rather than a
// QTreeView subclass: the view is a plain QTreeView everywhere else, and one
// signal's worth of behaviour does not earn a type.
class ViewportResizeRelay : public QObject
{
public:
    explicit ViewportResizeRelay(QTimer *coalesce)
      : QObject(coalesce)
      , m_coalesce(coalesce)
    {
    }

protected:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        if (event->type() == QEvent::Resize || event->type() == QEvent::Show) {
            m_coalesce->start();
        }
        return QObject::eventFilter(watched, event);
    }

private:
    QTimer *m_coalesce;
};

// Every visible (expanded, scrolled-into-view) tree row, as a path and a
// screen rect.
//
// The project tree emitted no markers at all until the Git submenu needed
// one: an E2E flow cannot right-click a row whose position it can only guess
// from font metrics and indentation. Same shape as `changes_panel.cpp`'s
// `changes_row` and `file_history_panel.cpp`'s `history_row`, and equally
// free when `IDE_E2E_EVENTS` is unset.
void markVisibleRows(QTreeView *treeView)
{
    QAbstractItemModel *model = treeView->model();
    if (!model) {
        return;
    }
    int count = 0;
    // `indexBelow` walks exactly what the user can see — collapsed subtrees
    // are skipped, which is the same set a click can reach.
    for (QModelIndex index = treeView->indexAt(QPoint(0, 0)); index.isValid();
          index = treeView->indexBelow(index)) {
        const QRect rect = treeView->visualRect(index);
        const QPoint origin =
          rect.isEmpty() ? QPoint() : treeView->viewport()->mapToGlobal(rect.topLeft());
        e2eMark(QStringLiteral("{\"ev\":\"project_tree_row\",\"path\":%1,"
                                "\"rect\":[%2,%3,%4,%5]}")
                  .arg(e2eJson(model->data(index, treeRole(ProjectTreeModel::Roles::Path))
                                  .toString()))
                  .arg(origin.x())
                  .arg(origin.y())
                  .arg(rect.width())
                  .arg(rect.height()));
        ++count;
    }
    e2eMark(QStringLiteral("{\"ev\":\"project_tree_rows\",\"count\":%1}").arg(count));
}

// Re-emit the row markers after anything that can move a row, coalesced onto
// the next event-loop turn: one project open is many `rowsInserted`, and a
// marker per insertion would say nothing a reader could act on.
void wireRowMarkers(QTreeView *treeView)
{
    auto *coalesce = new QTimer(treeView);
    coalesce->setSingleShot(true);
    coalesce->setInterval(0);
    QObject::connect(coalesce, &QTimer::timeout, treeView,
                      [treeView]() { markVisibleRows(treeView); });
    const auto schedule = [coalesce]() { coalesce->start(); };

    QAbstractItemModel *model = treeView->model();
    QObject::connect(model, &QAbstractItemModel::modelReset, treeView, schedule);
    QObject::connect(model, &QAbstractItemModel::layoutChanged, treeView, schedule);
    QObject::connect(model, &QAbstractItemModel::rowsInserted, treeView, schedule);
    QObject::connect(model, &QAbstractItemModel::rowsRemoved, treeView, schedule);
    QObject::connect(treeView, &QTreeView::expanded, treeView, schedule);
    QObject::connect(treeView, &QTreeView::collapsed, treeView, schedule);

    // A resize moves every row, and the first rows are laid out before the
    // dock has its final geometry — so without this the very first report is
    // published with rects that are about to be wrong, and a reader has no
    // way to tell it from the settled one that follows.
    treeView->viewport()->installEventFilter(new ViewportResizeRelay(coalesce));
}

// Icon + tooltip for the title-bar sort toggle reflect its current state —
// the title-bar button is icon-only (DockAreaTitleBar wraps it with no
// text), so the direction has to read from the arrow, JetBrains-style.
void updateSortAction(QAction *action, QWidget *iconSource, bool descending)
{
    const auto standardIcon = descending ? QStyle::SP_ArrowDown : QStyle::SP_ArrowUp;
    action->setIcon(iconSource->style()->standardIcon(standardIcon));
    action->setToolTip(descending ? QObject::tr("Sort Z to A") : QObject::tr("Sort A to Z"));
}

// A reticle (circle + four ticks) drawn directly rather than loaded from an
// asset: no standard QStyle icon looks like "locate", and unlike the tab
// close icon (which had to match a specific vendored glyph), any reasonable
// crosshair shape satisfies this button — QPainter is the smaller diff.
QIcon locateIcon(const QColor &tint)
{
    constexpr int kSide = 16;
    QPixmap pixmap(kSide, kSide);
    pixmap.fill(Qt::transparent);
    QPainter painter(&pixmap);
    painter.setRenderHint(QPainter::Antialiasing);
    painter.setPen(QPen(tint, 1.4));
    const QPointF center(kSide / 2.0, kSide / 2.0);
    constexpr qreal kRadius = 4.5;
    constexpr qreal kTickGap = 1.0;
    constexpr qreal kTickLength = 2.5;
    painter.drawEllipse(center, kRadius, kRadius);
    painter.drawLine(QPointF(center.x(), center.y() - kRadius - kTickGap - kTickLength),
                     QPointF(center.x(), center.y() - kRadius - kTickGap));
    painter.drawLine(QPointF(center.x(), center.y() + kRadius + kTickGap),
                     QPointF(center.x(), center.y() + kRadius + kTickGap + kTickLength));
    painter.drawLine(QPointF(center.x() - kRadius - kTickGap - kTickLength, center.y()),
                     QPointF(center.x() - kRadius - kTickGap, center.y()));
    painter.drawLine(QPointF(center.x() + kRadius + kTickGap, center.y()),
                     QPointF(center.x() + kRadius + kTickGap + kTickLength, center.y()));
    return QIcon(pixmap);
}

// Descends the tree one level at a time rather than splitting `path` into
// components: `Roles::Path` is always a node's full path, so the child
// whose path is a directory prefix of the target is the next step down —
// no separator-splitting or root-relative math needed.
void revealPathInTree(QTreeView *treeView, const QString &path)
{
    if (path.isEmpty()) {
        return;
    }
    QAbstractItemModel *model = treeView->model();
    QModelIndex parent;
    QModelIndex found;
    while (true) {
        const int rows = model->rowCount(parent);
        QModelIndex next;
        for (int row = 0; row < rows; ++row) {
            const QModelIndex index = model->index(row, 0, parent);
            const QString indexPath =
              model->data(index, treeRole(ProjectTreeModel::Roles::Path)).toString();
            if (indexPath == path) {
                next = index;
                break;
            }
            const bool isDir = model->data(index, treeRole(ProjectTreeModel::Roles::IsDir)).toBool();
            const bool isAncestor = path.length() > indexPath.length()
              && path.startsWith(indexPath)
              && (path.at(indexPath.length()) == QLatin1Char('/')
                  || path.at(indexPath.length()) == QLatin1Char('\\'));
            if (isDir && isAncestor) {
                next = index;
                break;
            }
        }
        if (!next.isValid()) {
            return; // Not found — e.g. the file sits outside the open project.
        }
        found = next;
        if (model->data(next, treeRole(ProjectTreeModel::Roles::Path)).toString() == path) {
            break;
        }
        treeView->expand(next);
        parent = next;
    }
    treeView->setCurrentIndex(found);
    treeView->scrollTo(found);
}

} // namespace

ProjectTreeDock createProjectTreeDock(ads::CDockManager *dockManager,
                                       ads::CDockAreaWidget *editorArea,
                                       ProjectTreeModel *treeModel,
                                       DockRegistry *docks)
{
    auto *treeView = new QTreeView();
    // The style's small-icon metric rather than a literal 16: it already
    // follows the platform's own scaling settings.
    const int iconPx = smallIconPx(treeView);
    treeView->setIconSize(QSize(iconPx, iconPx));

    // Between model and view, never inside the model: the icon key is the
    // Rust side's answer, and turning it into a decoration is the only part
    // that needs a QIcon. See icon_decoration_proxy.h.
    auto *proxy =
      new IconDecorationProxy(treeRole(ProjectTreeModel::Roles::IconKey), iconPx, treeView);
    proxy->setSourceModel(treeModel);

    treeView->setModel(proxy);
    treeView->setHeaderHidden(true);

    // Toolbar above the tree: the sort toggle (moved off the dock's title
    // bar) and the locate-in-tree button, so both stay visible even when
    // the dock is docked next to others and its title bar gets crowded.
    auto *toolbar = new QWidget();
    auto *toolbarLayout = new QHBoxLayout(toolbar);
    toolbarLayout->setContentsMargins(tokens::kSp1, tokens::kSp1 / 2, tokens::kSp1, tokens::kSp1 / 2);
    toolbarLayout->setSpacing(tokens::kSp1 / 2);

    auto *sortAction = new QAction(toolbar);
    sortAction->setCheckable(true);
    sortAction->setChecked(treeModel->sortDescending());
    updateSortAction(sortAction, treeView, sortAction->isChecked());
    QObject::connect(sortAction, &QAction::toggled, treeModel, [treeModel, sortAction, treeView](bool descending) {
        treeModel->setSortDescending(descending);
        updateSortAction(sortAction, treeView, descending);
    });
    auto *sortButton = new QToolButton(toolbar);
    sortButton->setDefaultAction(sortAction);

    // Disabled until an editor tab is open — main_window.cpp's active-tab-
    // changed callback flips this — and the icon is tinted to the tree's
    // own text color (QPalette::WindowText) rather than a TabColors field,
    // since this button lives in the tree's own chrome, not a tab strip.
    auto *locateAction = new QAction(QObject::tr("Locate in Project Tree"), toolbar);
    locateAction->setIcon(locateIcon(treeView->palette().color(QPalette::WindowText)));
    locateAction->setEnabled(false);
    auto *locateButton = new QToolButton(toolbar);
    locateButton->setDefaultAction(locateAction);

    toolbarLayout->addWidget(sortButton);
    toolbarLayout->addWidget(locateButton);
    toolbarLayout->addStretch(1);

    auto *container = new QWidget();
    auto *containerLayout = new QVBoxLayout(container);
    containerLayout->setContentsMargins(0, 0, 0, 0);
    containerLayout->setSpacing(0);
    containerLayout->addWidget(toolbar);
    containerLayout->addWidget(treeView);

    auto *treeDock = new ads::CDockWidget(dockManager, QObject::tr("Project"));
    treeDock->setWidget(container);

    docks->registerDock(QStringLiteral("projectTree"), treeDock, ads::LeftDockWidgetArea,
                        editorArea);
    return ProjectTreeDock{treeView, locateAction};
}

void wireProjectTree(QTreeView *treeView,
                     QAction *locateAction,
                     ProjectTreeModel *treeModel,
                     const ProjectTreeActions &actions)
{
    QObject::connect(locateAction, &QAction::triggered, treeView,
                     [treeView, currentEditorPath = actions.currentEditorPath]() {
                         revealPathInTree(treeView, currentEditorPath());
                     });

    wireRowMarkers(treeView);

    // Indexes from the view belong to the proxy, so every data() lookup goes
    // through the view's own model rather than the source directly.
    QAbstractItemModel *model = treeView->model();

    QObject::connect(
      treeView,
      &QTreeView::clicked,
      treeModel,
      [model, openFile = actions.openFile](const QModelIndex &index) {
          const bool isDir = model->data(index, treeRole(ProjectTreeModel::Roles::IsDir)).toBool();
          if (isDir) {
              return;
          }

          const QString path =
            model->data(index, treeRole(ProjectTreeModel::Roles::Path)).toString();
          openFile(path);
      });

    // Right-click context menu: create/rename/delete from the tree (US-2b).
    // Pure intent-forwarding: dialogs gather names/confirmation, the session
    // performs the operation (including retargeting any open tab).
    treeView->setContextMenuPolicy(Qt::CustomContextMenu);
    QObject::connect(
      treeView,
      &QTreeView::customContextMenuRequested,
      treeView,
      [treeView, treeModel, model, actions](const QPoint &pos) {
          QMainWindow *window = actions.window;
          const QString rootPath = treeModel->rootPath();
          if (rootPath.isEmpty()) {
              return; // No project open.
          }

          const QModelIndex index = treeView->indexAt(pos);
          const bool hasItem = index.isValid();
          QString itemPath;
          bool itemIsDir = false;
          QString targetDir = rootPath;
          if (hasItem) {
              itemPath = model->data(index, treeRole(ProjectTreeModel::Roles::Path)).toString();
              itemIsDir = model->data(index, treeRole(ProjectTreeModel::Roles::IsDir)).toBool();
              targetDir = itemIsDir ? itemPath : QFileInfo(itemPath).absolutePath();
          }

          QMenu menu(treeView);
          QAction *newFileAction = menu.addAction(QObject::tr("New File"));
          QAction *newFolderAction = menu.addAction(QObject::tr("New Folder"));
          QAction *renameAction = nullptr;
          QAction *deleteAction = nullptr;
          QAction *compareAction = nullptr;
          QAction *addToChatAction = nullptr;
          QAction *addToNewChatAction = nullptr;
          if (hasItem) {
              menu.addSeparator();
              renameAction = menu.addAction(QObject::tr("Rename"));
              deleteAction = menu.addAction(QObject::tr("Delete"));
              if (!itemIsDir) {
                  compareAction = menu.addAction(QObject::tr("Compare with…"));
                  // Files only: every entry under it is about one blob's
                  // history or one blob's changes.
                  appendGitSubmenu(menu, itemPath, actions);
              }
              // A folder attaches its contents, which is why the two entries
              // read the same for a file and a folder: what differs is the
              // rule `ai_chat_core::expand_folder` applies, not the gesture.
              menu.addSeparator();
              addToChatAction = menu.addAction(QObject::tr("Add to AI Chat"));
              addToNewChatAction = menu.addAction(QObject::tr("Add to New AI Chat"));
          }

          // The context menu is the only way into the Git submenu, and an
          // E2E flow needs to know it is a live toplevel before it types —
          // and where each entry is, since a popup has no model to ask.
          e2eMarkMenuActions(&menu, "project_tree_menu_action");
          e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"project_tree_context_menu\"}");
          QAction *chosen = menu.exec(treeView->viewport()->mapToGlobal(pos));
          e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\","
                                  "\"name\":\"project_tree_context_menu\",\"accepted\":%1}")
                    .arg(chosen ? "true" : "false"));
          if (!chosen) {
              return;
          }

          if (chosen == newFileAction) {
              const QString name = QInputDialog::getText(window, QObject::tr("New File"),
                                                           QObject::tr("File name:"));
              if (name.isEmpty()) {
                  return;
              }
              const auto result = treeModel->createFile(targetDir, name);
              if (result.code != 0) {
                  QMessageBox::critical(window, QObject::tr("Cannot create file"), result.message);
              }
          } else if (chosen == newFolderAction) {
              const QString name = QInputDialog::getText(window, QObject::tr("New Folder"),
                                                           QObject::tr("Folder name:"));
              if (name.isEmpty()) {
                  return;
              }
              const auto result = treeModel->createFolder(targetDir, name);
              if (result.code != 0) {
                  QMessageBox::critical(window, QObject::tr("Cannot create folder"),
                                         result.message);
              }
          } else if (chosen == renameAction) {
              const QString currentName = QFileInfo(itemPath).fileName();
              const QString newName = QInputDialog::getText(window, QObject::tr("Rename"),
                                                              QObject::tr("New name:"),
                                                              QLineEdit::Normal, currentName);
              if (newName.isEmpty() || newName == currentName) {
                  return;
              }
              const auto result = treeModel->renamePath(itemPath, newName);
              if (result.code != 0) {
                  QMessageBox::critical(window, QObject::tr("Cannot rename"), result.message);
              }
          } else if (chosen == deleteAction) {
              const QString itemName = QFileInfo(itemPath).fileName();
              const QString warning = itemIsDir
                ? QObject::tr("Delete folder \"%1\" and everything inside it? "
                               "This deletes its contents recursively and cannot be undone.")
                    .arg(itemName)
                : QObject::tr("Delete \"%1\"? This cannot be undone.").arg(itemName);
              const auto choice = QMessageBox::warning(window,
                                                         QObject::tr("Confirm delete"),
                                                         warning,
                                                         QMessageBox::Yes | QMessageBox::No,
                                                         QMessageBox::No);
              if (choice != QMessageBox::Yes) {
                  return;
              }
              const auto result = treeModel->deletePath(itemPath);
              if (result.code != 0) {
                  QMessageBox::critical(window, QObject::tr("Cannot delete"), result.message);
              }
          } else if (chosen == compareAction) {
              const QString otherPath = QFileDialog::getOpenFileName(
                window, QObject::tr("Compare \"%1\" With…").arg(QFileInfo(itemPath).fileName()));
              if (!otherPath.isEmpty()) {
                  actions.compareFiles(itemPath, otherPath);
              }
          } else if (chosen == addToChatAction || chosen == addToNewChatAction) {
              if (chosen == addToNewChatAction) {
                  actions.aiChat->newConversation();
              }
              // A folder attaches its contents and a file attaches itself;
              // which files a folder yields, and what to say about the ones
              // it did not, are both `ai-chat-core`'s answers.
              const FfiResult result = itemIsDir ? actions.aiChat->attachFolder(itemPath)
                                                  : actions.aiChat->attachFile(itemPath);
              if (result.code != 0) {
                  QMessageBox::information(window, QObject::tr("AI Chat"), result.message);
                  return;
              }
              actions.docks->show(QStringLiteral("aiChat"));
              // A folder's summary names what was skipped and what did not
              // fit; a single file has nothing to report, so only the folder
              // case is worth a line in the status bar.
              if (itemIsDir && !QString(result.message).isEmpty()) {
                  window->statusBar()->showMessage(result.message, 8000);
              }
              actions.aiChatPanel->attachAndFocus();
          }
      });
}

void wireProjectTreeViewAction(QMenu *viewMenu,
                               DockRegistry *docks,
                               AppSettings *appSettings,
                               QHash<QString, QAction *> &actions)
{
    QAction *projectTreeAction = registerAction(viewMenu, QStringLiteral("view.projectTree"),
                                                QObject::tr("Project"), appSettings, actions);
    QObject::connect(projectTreeAction, &QAction::triggered, viewMenu,
                     [docks]() { docks->show(QStringLiteral("projectTree")); });
}

} // namespace ui_shell
