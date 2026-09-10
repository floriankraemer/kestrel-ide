#include "editor_tabs.h"

#include "e2e_mark.h"
#include "hex_viewer.h"

#include <QAction>
#include <QByteArray>
#include <QDrag>
#include <QDragEnterEvent>
#include <QDragMoveEvent>
#include <QDropEvent>
#include <QIcon>
#include <QJsonArray>
#include <QJsonDocument>
#include <QJsonObject>
#include <QJsonValue>
#include <QList>
#include <QMenu>
#include <QMimeData>
#include <QMouseEvent>
#include <QPlainTextEdit>
#include <QPoint>
#include <QSplitter>
#include <QString>
#include <QTabBar>
#include <QTabWidget>
#include <QVariant>
#include <QWidget>

#include <functional>

namespace ui_shell {
namespace {

// Carries a dragged tab's TabId. Process-private on purpose: a TabId means
// nothing outside the session that issued it.
const QLatin1String kTabMimeType("application/x-ide-editor-tab");

// A tab strip a tab can be dragged out of and dropped into.
//
// Qt's own setMovable() reorders within one strip and stops dead at its
// edge, which is why a tab could not previously reach another pane. The
// gesture continues here as a QDrag, started only once the pointer leaves
// the strip — so as long as it stays inside, the built-in reorder still
// owns the whole interaction and behaves exactly as before.
//
// No Q_OBJECT: like EditorTabs itself it hands out std::function hooks
// instead of signals, so it needs no moc target.
class PaneTabBar : public QTabBar
{
public:
    explicit PaneTabBar(QWidget *parent)
      : QTabBar(parent)
    {
        setAcceptDrops(true);
    }

    // Qt's stylesheet style pads every tab with an icon by a hardcoded
    // 12px — `spaceForIcon = 6 + 4 + 2 /* magic */` in QStyleSheetStyle::
    // sizeFromContents(CT_TabBarTab) — on top of the icon width and spacing
    // QTabBar::tabSizeHint() has already counted. Nothing consumes it: the
    // text rect comes out 12px wider than the text, so Qt centres the label
    // in it and the slack lands as a gap on either side of the label, the
    // one before the [x] most visibly. Taking it back here is the only
    // seam that reaches it — the size hint is the widget's, while
    // CT_TabBarTab itself is answered by the stylesheet style and ignores a
    // QProxyStyle override of it (see makeTabCloseStyle() in theme.cpp for
    // the same trap one sub-element over).
    QSize tabSizeHint(int index) const override
    {
        constexpr int kStyleSheetIconPadding = 12;
        QSize hint = QTabBar::tabSizeHint(index);
        if (!tabIcon(index).isNull()) {
            hint.setWidth(hint.width() - kStyleSheetIconPadding);
        }
        return hint;
    }

    // Both set by EditorTabs::makeGroup; the bar itself decides nothing
    // about what a tab is or where it may land.
    std::function<quint64(int index)> tabIdAt_;
    std::function<void(quint64 tabId, int index)> tabDropped_;

protected:
    void mousePressEvent(QMouseEvent *event) override
    {
        if (event->button() == Qt::LeftButton && tabIdAt_) {
            // The id, not the index: a reorder inside the strip moves the
            // tab before the pointer ever leaves it.
            dragTabId_ = tabIdAt_(tabAt(event->position().toPoint()));
        }
        QTabBar::mousePressEvent(event);
    }

    void mouseReleaseEvent(QMouseEvent *event) override
    {
        dragTabId_ = 0;
        QTabBar::mouseReleaseEvent(event);
    }

    void mouseMoveEvent(QMouseEvent *event) override
    {
        if (!(event->buttons() & Qt::LeftButton) || dragTabId_ == 0
            || rect().contains(event->position().toPoint())) {
            QTabBar::mouseMoveEvent(event);
            return;
        }
        startDrag(event);
    }

    void dragEnterEvent(QDragEnterEvent *event) override { acceptTab(event); }

    void dragMoveEvent(QDragMoveEvent *event) override { acceptTab(event); }

    void dropEvent(QDropEvent *event) override
    {
        if (!event->mimeData()->hasFormat(kTabMimeType)) {
            return;
        }
        event->acceptProposedAction();
        if (tabDropped_) {
            // tabAt() answers -1 past the last tab, which is the drop
            // "after everything" this strip should append.
            tabDropped_(event->mimeData()->data(kTabMimeType).toULongLong(),
                        tabAt(event->position().toPoint()));
        }
    }

private:
    void acceptTab(QDragMoveEvent *event)
    {
        if (event->mimeData()->hasFormat(kTabMimeType)) {
            event->acceptProposedAction();
        }
    }

    int indexOfTab(quint64 tabId) const
    {
        for (int i = 0; i < count(); ++i) {
            if (tabIdAt_(i) == tabId) {
                return i;
            }
        }
        return -1;
    }

    void startDrag(QMouseEvent *event)
    {
        const quint64 tabId = dragTabId_;
        dragTabId_ = 0;
        const int index = indexOfTab(tabId);
        if (index < 0) {
            return;
        }

        // End QTabBar's own reorder before taking the pointer off it: a
        // QDrag runs its own event loop, and the strip would otherwise sit
        // stuck mid-move for the rest of the gesture.
        QMouseEvent release(QEvent::MouseButtonRelease, event->position(),
                            event->globalPosition(), Qt::LeftButton, Qt::NoButton,
                            Qt::NoModifier);
        QTabBar::mouseReleaseEvent(&release);

        auto *mime = new QMimeData;
        mime->setData(kTabMimeType, QByteArray::number(tabId));
        auto *drag = new QDrag(this);
        drag->setMimeData(mime);
        drag->setPixmap(grab(tabRect(index)));
        drag->exec(Qt::MoveAction);
    }

    quint64 dragTabId_ = 0;
};

// A tab group whose strip is a PaneTabBar. QTabWidget::setTabBar is
// protected, so installing one takes a subclass — this is the whole reason
// this class exists.
class PaneTabWidget : public QTabWidget
{
public:
    explicit PaneTabWidget(QWidget *parent)
      : QTabWidget(parent)
    {
        setTabBar(new PaneTabBar(this));
    }
};

} // namespace

QString EditorTabs::saveLayout() const
{
    return QString::fromUtf8(
      QJsonDocument(serializeSplitter(root_, /*includeFiles=*/true)).toJson(QJsonDocument::Compact));
}

QString EditorTabs::saveGrid() const
{
    return QString::fromUtf8(
      QJsonDocument(serializeSplitter(root_, /*includeFiles=*/false)).toJson(QJsonDocument::Compact));
}

// True when `json` described a grid that produced at least one pane.
//
// False covers two different failures, and both leave the caller with a
// usable window: unparseable JSON is rejected *before* anything is torn down,
// so the panes on screen survive it untouched, while a grid that yields no
// pane at all leaves a single fresh group behind. Neither is worth
// distinguishing at the call sites — both mean "there is no restored layout
// to finish setting up".
bool EditorTabs::rebuildFrom(const QString &json, bool allowEmptyGroups)
{
    const QJsonDocument doc = QJsonDocument::fromJson(json.toUtf8());
    if (!doc.isObject()) {
        return false;
    }
    const QJsonObject rootObject = doc.object();
    if (rootObject.value(QStringLiteral("type")).toString() != QLatin1String("splitter")) {
        return false;
    }

    for (QTabWidget *group : std::as_const(groups_)) {
        group->setParent(nullptr);
        delete group;
    }
    groups_.clear();
    activeGroup_ = nullptr;
    // Cleared, not merely overwritten: without this a grid with no `focused`
    // pane would inherit whichever group the *previous* rebuild focused, and
    // that pointer names a widget the loop above has just deleted.
    restoredActiveGroup_ = nullptr;

    applySplitter(root_, rootObject, allowEmptyGroups);

    if (groups_.isEmpty()) {
        activeGroup_ = makeGroup();
        root_->addWidget(activeGroup_);
        return false;
    }
    return true;
}

void EditorTabs::restoreLayout(const QString &json)
{
    suspendActivation_ = true;
    const bool restored = rebuildFrom(json, /*allowEmptyGroups=*/false);
    suspendActivation_ = false;
    if (!restored) {
        return;
    }

    QTabWidget *group = restoredActiveGroup_ ? restoredActiveGroup_ : groups_.first();
    setActiveGroup(group, group->currentIndex());
    markPaneCount();
}

void EditorTabs::applyGrid(const QString &json)
{
    // Parsed before anything moves: a grid that turns out to be unusable must
    // not cost the user the arrangement they already had. `rebuildFrom`
    // re-parses it, which is a few microseconds against never having half
    // torn down the editor.
    const QJsonDocument doc = QJsonDocument::fromJson(json.toUtf8());
    if (!doc.isObject()
        || doc.object().value(QStringLiteral("type")).toString() != QLatin1String("splitter")) {
        return;
    }

    // Every open document comes out of its strip before the strips are torn
    // down — applying a layout rearranges the workspace and closes nothing.
    // `removeTab` drops the tab's decoration along with the tab, so title,
    // icon and tooltip are carried by hand; same reason `moveTabToGroup`
    // does it.
    struct DetachedTab
    {
        QWidget *page;
        QIcon icon;
        QString title;
        QString toolTip;
    };
    QList<DetachedTab> detached;
    QWidget *wasCurrent = activeGroup_ ? activeGroup_->currentWidget() : nullptr;

    suspendActivation_ = true;
    for (QTabWidget *group : std::as_const(groups_)) {
        while (group->count() > 0) {
            detached.append(
              DetachedTab{group->widget(0), group->tabIcon(0), group->tabText(0),
                          group->tabToolTip(0)});
            group->removeTab(0);
        }
    }

    // Return value ignored on purpose: either branch leaves `groups_`
    // non-empty, and the documents below have to land somewhere regardless.
    rebuildFrom(json, /*allowEmptyGroups=*/true);

    QTabWidget *target = restoredActiveGroup_ ? restoredActiveGroup_ : groups_.first();
    int currentIndex = -1;
    for (const DetachedTab &tab : std::as_const(detached)) {
        const int at = target->addTab(tab.page, tab.icon, tab.title);
        target->setTabToolTip(at, tab.toolTip);
        if (tab.page == wasCurrent) {
            currentIndex = at;
        }
    }
    suspendActivation_ = false;

    setActiveGroup(target, currentIndex >= 0 ? currentIndex : target->currentIndex());
    markPaneCount();
}

quint64 EditorTabs::tabIdAt(QTabWidget *group, int index) const
{
    QWidget *widget = group ? group->widget(index) : nullptr;
    return widget ? widget->property("tabId").toULongLong() : 0;
}

EditorTabs::TabLoc EditorTabs::locate(quint64 tabId) const
{
    for (QTabWidget *group : std::as_const(groups_)) {
        for (int i = 0; i < group->count(); ++i) {
            if (tabIdAt(group, i) == tabId) {
                return {group, i};
            }
        }
    }
    return {};
}

QPlainTextEdit *EditorTabs::editorForTab(quint64 tabId) const
{
    const TabLoc loc = locate(tabId);
    return loc.group ? qobject_cast<QPlainTextEdit *>(loc.group->widget(loc.index)) : nullptr;
}

void EditorTabs::forEachEditor(const std::function<void(QPlainTextEdit *)> &apply) const
{
    for (QTabWidget *group : std::as_const(groups_)) {
        for (int i = 0; i < group->count(); ++i) {
            if (auto *editor = qobject_cast<QPlainTextEdit *>(group->widget(i))) {
                apply(editor);
            }
        }
    }
}

void EditorTabs::forEachHexViewer(const std::function<void(HexViewer *)> &apply) const
{
    for (QTabWidget *group : std::as_const(groups_)) {
        for (int i = 0; i < group->count(); ++i) {
            if (auto *viewer = qobject_cast<HexViewer *>(group->widget(i))) {
                apply(viewer);
            }
        }
    }
}

void EditorTabs::focusTab(quint64 tabId)
{
    const TabLoc loc = locate(tabId);
    if (!loc.group) {
        return;
    }
    loc.group->setCurrentIndex(loc.index);
    setActiveGroup(loc.group, loc.index);
}

QTabWidget *EditorTabs::makeGroup()
{
    auto *group = new PaneTabWidget(root_);
    auto *bar = static_cast<PaneTabBar *>(group->tabBar());
    bar->tabIdAt_ = [this, group](int index) { return tabIdAt(group, index); };
    bar->tabDropped_ = [this, group](quint64 tabId, int index) {
        moveTabToGroup(tabId, group, index);
    };
    group->setTabsClosable(true);
    group->setUsesScrollButtons(true);
    // G2: drag-reorder is safe with no adapter/app-core change because
    // TabId is looked up by scanning each page's dynamic property, not
    // by a maintained index map (see tabIdAt/locate above) — a reorder
    // can't desynchronize anything.
    group->setMovable(true);

    connect(group, &QTabWidget::tabCloseRequested, this,
            [this, group](int index) { requestCloseTab(group, index); });
    connect(group, &QTabWidget::currentChanged, this, [this, group](int index) {
        // Ignored while a split/restore is moving pages around: the
        // structural code sets the active group itself once it's done.
        if (suspendActivation_ || index < 0) {
            return;
        }
        setActiveGroup(group, index);
    });

    group->tabBar()->setContextMenuPolicy(Qt::CustomContextMenu);
    connect(group->tabBar(), &QTabBar::customContextMenuRequested, this,
            [this, group](const QPoint &pos) { showTabContextMenu(group, pos); });

    groups_.append(group);
    return group;
}

void EditorTabs::setActiveGroup(QTabWidget *group, int index)
{
    activeGroup_ = group;
    if (index >= 0) {
        docManager_->setActiveTab(tabIdAt(group, index));
    }
    updateStatusBar();
    if (activeTabChanged_) {
        activeTabChanged_();
    }
}

void EditorTabs::showTabContextMenu(QTabWidget *group, const QPoint &pos)
{
    const int index = group->tabBar()->tabAt(pos);
    if (index < 0) {
        return;
    }

    QMenu menu(group);
    QAction *closeAction = menu.addAction(tr("Close"));
    QAction *closeOthersAction = menu.addAction(tr("Close Others"));
    closeOthersAction->setEnabled(group->count() > 1);
    menu.addSeparator();
    // JetBrains naming: "vertical" describes the divider, so a vertical
    // split puts the panes side by side (a Qt::Horizontal splitter).
    QAction *splitVerticalAction = menu.addAction(tr("Split Vertical"));
    QAction *splitHorizontalAction = menu.addAction(tr("Split Horizontal"));
    splitVerticalAction->setEnabled(group->count() > 1);
    splitHorizontalAction->setEnabled(group->count() > 1);

    // A popup menu takes a keyboard grab rather than the input focus, so
    // this mark is the only way anything outside the process can know it
    // is up. `exec()` does not return until it is gone, hence the mark
    // before rather than after.
    e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"tab_context_menu\"}");
    QAction *chosen = menu.exec(group->tabBar()->mapToGlobal(pos));
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"tab_context_menu\","
                            "\"accepted\":%1}")
              .arg(chosen != nullptr ? QLatin1String("true") : QLatin1String("false")));
    if (chosen == closeAction) {
        requestCloseTab(group, index);
    } else if (chosen == closeOthersAction) {
        closeOtherTabs(group, index);
    } else if (chosen == splitVerticalAction) {
        splitTab(group, index, Qt::Horizontal);
    } else if (chosen == splitHorizontalAction) {
        splitTab(group, index, Qt::Vertical);
    }
}

void EditorTabs::closeOtherTabs(QTabWidget *group, int keptIndex)
{
    QList<quint64> victims;
    for (int i = 0; i < group->count(); ++i) {
        if (i != keptIndex) {
            victims.append(tabIdAt(group, i));
        }
    }
    for (const quint64 tabId : std::as_const(victims)) {
        const TabLoc loc = locate(tabId);
        if (!loc.group) {
            continue;
        }
        if (!confirmCloseTab(loc.group, loc.index)) {
            return; // Cancel on one tab abandons the rest, as on exit.
        }
        docManager_->closeTab(tabId);
    }
}

void EditorTabs::splitTab(QTabWidget *group, int index, Qt::Orientation orientation)
{
    if (group->count() < 2) {
        return;
    }
    auto *parent = qobject_cast<QSplitter *>(group->parentWidget());
    if (!parent) {
        return;
    }

    QWidget *page = group->widget(index);
    const QString title = group->tabText(index);
    // A split moves the page rather than reopening it, and removeTab
    // drops the decoration along with the tab.
    const QIcon icon = group->tabIcon(index);

    QSplitter *target = parent;
    int insertPos = parent->indexOf(group) + 1;
    if (parent->count() > 1 && parent->orientation() != orientation) {
        // The parent already splits the other way and has siblings to
        // keep — nest a new splitter around just this group.
        const QList<int> parentSizes = parent->sizes();
        const int groupPos = parent->indexOf(group);
        // Parentless on purpose: a QSplitter adopts any child created
        // with it as parent, which would append the new splitter as an
        // extra pane before replaceWidget() could put it in place.
        auto *nested = new QSplitter(orientation);
        parent->replaceWidget(groupPos, nested);
        nested->addWidget(group);
        // replaceWidget() hands the old widget back hidden.
        group->show();
        parent->setSizes(parentSizes);
        target = nested;
        insertPos = 1;
    } else {
        parent->setOrientation(orientation);
    }

    suspendActivation_ = true;
    auto *newGroup = makeGroup();
    target->insertWidget(insertPos, newGroup);
    group->removeTab(index);
    newGroup->addTab(page, icon, title);
    suspendActivation_ = false;

    target->setSizes(evenSizes(target));
    setActiveGroup(newGroup, newGroup->indexOf(page));
    page->setFocus();
    // A split *moves* the tab, so it reports the move like any other: the
    // marker is what tells a test where the tab now is on screen.
    markTab("tab_moved", tabIdAt(newGroup, newGroup->indexOf(page)), newGroup,
            newGroup->indexOf(page), title);
    e2eMark(QStringLiteral("{\"ev\":\"split_created\",\"orientation\":\"%1\"}")
              .arg(orientation == Qt::Horizontal ? QLatin1String("h") : QLatin1String("v")));
    markPaneCount();
}

void EditorTabs::moveTabToGroup(quint64 tabId, QTabWidget *target, int index)
{
    const TabLoc loc = locate(tabId);
    if (!loc.group || loc.group == target) {
        return; // Within one strip, QTabBar's own reorder has already run.
    }

    QTabWidget *source = loc.group;
    QWidget *page = source->widget(loc.index);
    // As in splitTab: the page moves rather than reopening, and removeTab
    // drops the decoration along with the tab.
    const QIcon icon = source->tabIcon(loc.index);
    const QString title = source->tabText(loc.index);
    const QString toolTip = source->tabToolTip(loc.index);

    suspendActivation_ = true;
    source->removeTab(loc.index);
    const int at = index < 0 ? target->count() : qMin(index, target->count());
    const int moved = target->insertTab(at, page, icon, title);
    target->setTabToolTip(moved, toolTip);
    target->setCurrentIndex(moved);
    suspendActivation_ = false;

    if (source->count() == 0) {
        collapseGroup(source);
    }
    setActiveGroup(target, moved);
    page->setFocus();
    markTab("tab_moved", tabId, target, moved, title);
    markPaneCount();
}

QList<int> EditorTabs::evenSizes(QSplitter *splitter)
{
    const int count = qMax(1, splitter->count());
    const int extent =
      splitter->orientation() == Qt::Horizontal ? splitter->width() : splitter->height();
    return QList<int>(count, qMax(1, extent / count));
}

void EditorTabs::collapseGroup(QTabWidget *group)
{
    if (groups_.size() < 2) {
        return;
    }
    auto *parent = qobject_cast<QSplitter *>(group->parentWidget());
    groups_.removeAll(group);
    group->setParent(nullptr);
    group->deleteLater();
    pruneSplitters(parent);

    if (activeGroup_ == group) {
        QTabWidget *next = groups_.first();
        setActiveGroup(next, next->currentIndex());
    }
}

void EditorTabs::pruneSplitters(QSplitter *splitter)
{
    while (splitter && splitter != root_ && splitter->count() == 1) {
        auto *grandParent = qobject_cast<QSplitter *>(splitter->parentWidget());
        if (!grandParent) {
            return;
        }
        const QList<int> sizes = grandParent->sizes();
        grandParent->replaceWidget(grandParent->indexOf(splitter), splitter->widget(0));
        splitter->setParent(nullptr);
        splitter->deleteLater();
        grandParent->setSizes(sizes);
        splitter = grandParent;
    }
}

void EditorTabs::markPaneCount()
{
    e2eMark(QStringLiteral("{\"ev\":\"pane_count\",\"n\":%1}").arg(groups_.size()));
}

QJsonObject EditorTabs::serializeSplitter(const QSplitter *splitter, bool includeFiles) const
{
    QJsonArray children;
    for (int i = 0; i < splitter->count(); ++i) {
        QWidget *child = splitter->widget(i);
        if (auto *group = qobject_cast<QTabWidget *>(child)) {
            children.append(serializeGroup(group, includeFiles));
        } else if (auto *nested = qobject_cast<QSplitter *>(child)) {
            children.append(serializeSplitter(nested, includeFiles));
        }
    }
    QJsonArray sizes;
    for (const int size : splitter->sizes()) {
        sizes.append(size);
    }

    QJsonObject object;
    object[QStringLiteral("type")] = QStringLiteral("splitter");
    object[QStringLiteral("orientation")] =
      splitter->orientation() == Qt::Horizontal ? QStringLiteral("h") : QStringLiteral("v");
    object[QStringLiteral("sizes")] = sizes;
    object[QStringLiteral("children")] = children;
    return object;
}

QJsonObject EditorTabs::serializeGroup(QTabWidget *group, bool includeFiles) const
{
    QJsonObject object;
    object[QStringLiteral("type")] = QStringLiteral("group");
    // `focused` is written either way: which pane the user was working in is
    // part of an arrangement, not part of the document set.
    object[QStringLiteral("focused")] = group == activeGroup_;
    if (!includeFiles) {
        return object;
    }

    QJsonArray files;
    for (int i = 0; i < group->count(); ++i) {
        const QString path = docManager_->tabPath(tabIdAt(group, i));
        if (!path.isEmpty()) {
            files.append(path);
        }
    }
    object[QStringLiteral("files")] = files;
    object[QStringLiteral("active")] = docManager_->tabPath(
      tabIdAt(group, group->currentIndex()));
    return object;
}

void EditorTabs::applySplitter(QSplitter *splitter, const QJsonObject &object,
                               bool allowEmptyGroups)
{
    splitter->setOrientation(
      object.value(QStringLiteral("orientation")).toString() == QLatin1String("v")
        ? Qt::Vertical
        : Qt::Horizontal);

    const QJsonArray children = object.value(QStringLiteral("children")).toArray();
    for (const QJsonValue &child : children) {
        const QJsonObject childObject = child.toObject();
        if (childObject.value(QStringLiteral("type")).toString() == QLatin1String("group")) {
            restoreGroup(splitter, childObject, allowEmptyGroups);
        } else if (childObject.value(QStringLiteral("type")).toString()
                   == QLatin1String("splitter")) {
            auto *nested = new QSplitter(splitter);
            splitter->addWidget(nested);
            applySplitter(nested, childObject, allowEmptyGroups);
        }
    }

    QList<int> sizes;
    const QJsonArray savedSizes = object.value(QStringLiteral("sizes")).toArray();
    for (const QJsonValue &size : savedSizes) {
        sizes.append(size.toInt());
    }
    if (sizes.size() == splitter->count()) {
        splitter->setSizes(sizes);
    }
}

void EditorTabs::restoreGroup(QSplitter *splitter, const QJsonObject &object,
                              bool allowEmptyGroups)
{
    const QJsonArray files = object.value(QStringLiteral("files")).toArray();
    if (files.isEmpty() && !allowEmptyGroups) {
        return; // Nothing to show in it — don't restore an empty pane.
    }

    auto *group = makeGroup();
    splitter->addWidget(group);
    // onTabOpened puts each new tab in the active group, so this is how
    // a restored file lands in the group it was saved from.
    activeGroup_ = group;

    const QString activePath = object.value(QStringLiteral("active")).toString();
    quint64 activeTabId = 0;
    for (const QJsonValue &file : files) {
        const QString path = file.toString();
        const auto result = docManager_->openFile(path);
        if (result.code != 0) {
            continue; // Deleted or unreadable since last run — skip it.
        }
        if (path == activePath) {
            activeTabId = result.tab_id;
        }
    }
    if (group->count() == 0 && !allowEmptyGroups) {
        // Every file in this group failed to reopen.
        groups_.removeAll(group);
        group->setParent(nullptr);
        delete group;
        return;
    }
    if (activeTabId != 0) {
        const TabLoc loc = locate(activeTabId);
        if (loc.group == group) {
            group->setCurrentIndex(loc.index);
        }
    }
    if (object.value(QStringLiteral("focused")).toBool()) {
        restoredActiveGroup_ = group;
    }
}

} // namespace ui_shell
