#include "plugins_page.h"

#include "e2e_mark.h"
#include "theme.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QDesktopServices>
#include <QHBoxLayout>
#include <QHash>
#include <QHeaderView>
#include <QLabel>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QStringList>
#include <QTimer>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QTreeWidgetItemIterator>
#include <QUrl>
#include <QVBoxLayout>
#include <QWidget>

#include <memory>

namespace ui_shell {

namespace {

constexpr int kPluginColumn = 0;
constexpr int kContributesColumn = 1;
constexpr int kVersionColumn = 2;
constexpr int kStatusColumn = 3;
constexpr int kIdRole = Qt::UserRole;

QString sourceGroup(FfiPluginSource source)
{
    switch (source) {
    case FfiPluginSource::Builtin:
        return QObject::tr("Built-in");
    case FfiPluginSource::Installed:
        return QObject::tr("Installed");
    }
    return QString();
}

QColor severityColorFor(FfiRowSeverity severity)
{
    const SemanticColors colors = semanticColors();
    switch (severity) {
    case FfiRowSeverity::Error:
        return colors.error;
    case FfiRowSeverity::Warning:
        return colors.warning;
    case FfiRowSeverity::Muted:
        return colors.muted;
    case FfiRowSeverity::Healthy:
        break;
    }
    return QColor();
}

QString selectedId(const QTreeWidget *tree)
{
    const QTreeWidgetItem *item = tree->currentItem();
    return item ? item->data(kPluginColumn, kIdRole).toString() : QString();
}

// Rebuilds the tree, grouped by source. A group with no plugins gets no
// header — a user who has installed nothing never sees an `Installed` group.
void populate(QTreeWidget *tree, PluginCatalog *catalog, bool problemsOnly, const QString &keepId)
{
    tree->clear();

    QHash<int, QTreeWidgetItem *> groups;
    for (const FfiPluginRow &row : catalog->plugins()) {
        if (problemsOnly && row.status.isEmpty()) {
            continue;
        }
        const int key = static_cast<int>(row.source);
        QTreeWidgetItem *group = groups.value(key);
        if (!group) {
            group = new QTreeWidgetItem(tree, QStringList{sourceGroup(row.source)});
            group->setFlags(Qt::ItemIsEnabled);
            group->setExpanded(true);
            groups.insert(key, group);
        }

        auto *item = new QTreeWidgetItem(
          group, QStringList{row.name, row.contributes, row.version, row.status});
        item->setData(kPluginColumn, kIdRole, row.id);
        item->setToolTip(kPluginColumn, row.description);
        const QColor color = severityColorFor(row.severity);
        if (color.isValid()) {
            item->setForeground(kStatusColumn, color);
        }
        if (row.id == keepId) {
            tree->setCurrentItem(item);
        }
    }
    tree->expandAll();

    // G1.6: a row's rect is only valid once this page is actually the
    // settings dialog's current one, which `deferPage`'s lazy build makes
    // true one event-loop turn *after* this very call on a first switch to
    // Plugins (`settings_dialog.cpp`'s own `editor_page_shown`/
    // `containers_settings_page_shown` give the same reasoning) — a
    // `QTimer::singleShot(0, …)` lands after that either way, first switch
    // or a later reload/filter toggle alike.
    QTimer::singleShot(0, tree, [tree]() {
        QStringList rows;
        for (auto it = QTreeWidgetItemIterator(tree); *it; ++it) {
            QTreeWidgetItem *item = *it;
            const QString id = item->data(kPluginColumn, kIdRole).toString();
            if (id.isEmpty()) {
                continue; // a source-group header, not a plugin row
            }
            const QRect rect = tree->visualItemRect(item);
            if (!rect.isValid() || rect.isEmpty() || !tree->viewport()->rect().contains(rect)) {
                continue;
            }
            const QPoint origin = tree->viewport()->mapToGlobal(rect.topLeft());
            rows << QStringLiteral("{\"id\":%1,\"rect\":[%2,%3,%4,%5]}")
                      .arg(e2eJson(id))
                      .arg(origin.x())
                      .arg(origin.y())
                      .arg(rect.width())
                      .arg(rect.height());
        }
        e2eMark(QStringLiteral("{\"ev\":\"plugins_page_rows\",\"rows\":[%1]}")
                  .arg(rows.join(QLatin1Char(','))));
    });
}

} // namespace

QWidget *buildPluginsPage(QWidget *parent,
                          PluginCatalog *catalog,
                          std::function<void()> pluginsChanged)
{
    auto *page = new QWidget(parent);
    page->setMinimumSize(560, 460);
    auto *layout = new QVBoxLayout(page);

    auto *problemsOnly = new QCheckBox(QObject::tr("Show only plugins with problems"), page);
    // Re-scans the plugins directory and re-renders — what a user who just
    // dropped a plugin folder in reaches for.
    auto *reloadButton = new QPushButton(QObject::tr("Reload Plugins"), page);
    auto *topRow = new QHBoxLayout();
    topRow->addWidget(problemsOnly, 1);
    topRow->addWidget(reloadButton);
    layout->addLayout(topRow);

    auto *tree = new QTreeWidget(page);
    tree->setColumnCount(4);
    tree->setHeaderLabels({QObject::tr("Plugin"), QObject::tr("Contributes"),
                           QObject::tr("Version"), QObject::tr("Status")});
    // Contributes is the one column allowed to absorb the leftover width and
    // therefore the one that elides, for the reason the Languages page gives
    // about its Matches column: Status is what the page exists for and must
    // never be pushed off the right edge.
    tree->header()->setStretchLastSection(false);
    tree->header()->setSectionResizeMode(kPluginColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kContributesColumn, QHeaderView::Stretch);
    tree->header()->setSectionResizeMode(kVersionColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kStatusColumn, QHeaderView::ResizeToContents);
    tree->setTextElideMode(Qt::ElideRight);
    tree->setRootIsDecorated(false);
    tree->setIndentation(12);
    layout->addWidget(tree, 1);

    // The details pane: selectable so the message and the path can be
    // copied, and hidden entirely when the selected plugin is healthy.
    auto *details = new QPlainTextEdit(page);
    details->setReadOnly(true);
    details->setMaximumHeight(110);
    details->setVisible(false);
    layout->addWidget(details);

    auto *toggleButton = new QPushButton(page);
    auto *openFolderButton = new QPushButton(QObject::tr("Open Folder"), page);
    auto *actionsRow = new QHBoxLayout();
    actionsRow->addWidget(toggleButton);
    actionsRow->addStretch(1);
    actionsRow->addWidget(openFolderButton);
    layout->addLayout(actionsRow);

    auto *pathLabel =
      new QLabel(QObject::tr("Plugins are read from %1").arg(catalog->pluginsDir()), page);
    pathLabel->setTextInteractionFlags(Qt::TextSelectableByMouse);
    pathLabel->setStyleSheet(QStringLiteral("color: %1;").arg(semanticColors().muted.name()));
    layout->addWidget(pathLabel);

    // The problem currently shown and which way the toggle currently goes,
    // so the click handlers act on what the user was looking at rather than
    // re-deriving it from the widgets.
    auto current = std::make_shared<FfiPluginProblem>();
    auto toggleState = std::make_shared<FfiPluginToggle>();

    auto showProblem = [=](const QString &id) {
        *current = catalog->problem(id);
        *toggleState = catalog->toggle(id);
        toggleButton->setText(toggleState->label);
        toggleButton->setEnabled(toggleState->enabled);
        // G1.6: which way the toggle for the *selected* row currently
        // goes, so an E2E flow (disable a plugin, relaunch, check its
        // View-menu entry is gone) can find the button's own rect and
        // confirm it read "Disable" before clicking, the same deferred-
        // rect reasoning `populate`'s own `plugins_page_rows` mark gives.
        QTimer::singleShot(0, toggleButton, [toggleButton, id, disable = toggleState->disable]() {
            const QRect rect(toggleButton->mapToGlobal(QPoint(0, 0)), toggleButton->size());
            e2eMark(QStringLiteral("{\"ev\":\"plugins_page_toggle\",\"id\":%1,"
                                    "\"disable\":%2,\"rect\":[%3,%4,%5,%6]}")
                      .arg(e2eJson(id))
                      .arg(disable ? QLatin1String("true") : QLatin1String("false"))
                      .arg(rect.x())
                      .arg(rect.y())
                      .arg(rect.width())
                      .arg(rect.height()));
        });
        const bool hasProblem = !current->sentence.isEmpty();
        details->setVisible(hasProblem);
        // A built-in has no directory on disk, so there is nothing to open.
        openFolderButton->setVisible(!current->path.isEmpty());
        if (!hasProblem) {
            details->clear();
            return;
        }
        QStringList lines;
        lines << current->sentence;
        if (!current->detail.isEmpty()) {
            lines << current->detail;
        }
        if (!current->path.isEmpty()) {
            lines << current->path;
        }
        details->setPlainText(lines.join(QLatin1Char('\n')));
    };

    auto refresh = [=](const QString &keepId) {
        catalog->refresh();
        populate(tree, catalog, problemsOnly->isChecked(), keepId);
        showProblem(selectedId(tree));
    };

    QObject::connect(tree, &QTreeWidget::currentItemChanged, page,
                      [=]() { showProblem(selectedId(tree)); });
    QObject::connect(problemsOnly, &QCheckBox::toggled, page, [=]() {
        populate(tree, catalog, problemsOnly->isChecked(), selectedId(tree));
    });
    QObject::connect(reloadButton, &QPushButton::clicked, page,
                      [=]() { refresh(selectedId(tree)); });

    QObject::connect(openFolderButton, &QPushButton::clicked, page, [=]() {
        QDesktopServices::openUrl(QUrl::fromLocalFile(current->path));
    });

    // Turning a plugin off takes effect immediately — the registry is
    // rebuilt and everything standing on it restarted — so there is no
    // OK-shaped promise on this page and no confirmation to give: it is
    // reversible from the same button that did it.
    QObject::connect(toggleButton, &QPushButton::clicked, page, [=]() {
        const QString id = selectedId(tree);
        const FfiResult result = catalog->setDisabled(id, toggleState->disable);
        if (result.code != 0) {
            QMessageBox::critical(page,
                                  toggleState->disable ? QObject::tr("Cannot disable plugin")
                                                       : QObject::tr("Cannot enable plugin"),
                                  result.message);
            return;
        }
        refresh(id);
        if (pluginsChanged) {
            pluginsChanged();
        }
    });

    refresh(QString());
    return page;
}

} // namespace ui_shell
