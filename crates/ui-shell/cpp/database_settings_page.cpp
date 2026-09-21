#include "database_settings_page.h"

#include "data_source_dialog.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHBoxLayout>
#include <QListWidget>
#include <QListWidgetItem>
#include <QPushButton>
#include <QString>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

QString rowLabel(const FfiDataSourceRow &row)
{
    const QString name = row.name.isEmpty() ? QObject::tr("(unnamed)") : QString(row.name);
    const QString scope =
      QString(row.scope) == QStringLiteral("project") ? QObject::tr("project") : QObject::tr("global");
    return QStringLiteral("%1  [%2 · %3]").arg(name, QString(row.driver), scope);
}

void reloadList(QListWidget *list, AppSettings *appSettings)
{
    const QString selectedId =
      list->currentItem() ? list->currentItem()->data(Qt::UserRole).toString() : QString();
    list->clear();
    int selectedRow = -1;
    const ::rust::Vec<FfiDataSourceRow> rows = appSettings->databaseSources();
    for (const FfiDataSourceRow &row : rows) {
        auto *item = new QListWidgetItem(rowLabel(row), list);
        item->setData(Qt::UserRole, QString(row.id));
        if (QString(row.id) == selectedId) {
            selectedRow = list->count() - 1;
        }
    }
    if (selectedRow >= 0) {
        list->setCurrentRow(selectedRow);
    } else if (list->count() > 0) {
        list->setCurrentRow(0);
    }
}

} // namespace

QWidget *buildDatabaseSettingsPage(QWidget *parent, AppSettings *appSettings,
                                    DataSourceEditor *editor)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 0, 0, 0);

    auto *list = new QListWidget(page);
    layout->addWidget(list, 1);
    reloadList(list, appSettings);

    auto *buttonsRow = new QHBoxLayout();
    auto *addButton = new QPushButton(QObject::tr("Add"), page);
    auto *editButton = new QPushButton(QObject::tr("Edit..."), page);
    auto *duplicateButton = new QPushButton(QObject::tr("Duplicate"), page);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), page);
    buttonsRow->addWidget(addButton);
    buttonsRow->addWidget(editButton);
    buttonsRow->addWidget(duplicateButton);
    buttonsRow->addWidget(removeButton);
    buttonsRow->addStretch(1);
    layout->addLayout(buttonsRow);

    const auto currentId = [list]() -> QString {
        QListWidgetItem *item = list->currentItem();
        return item ? item->data(Qt::UserRole).toString() : QString();
    };

    QObject::connect(addButton, &QPushButton::clicked, page, [page, appSettings, editor, list]() {
        showDataSourceDialog(page, appSettings, editor, QString(), QStringLiteral("global"));
        reloadList(list, appSettings);
    });
    QObject::connect(editButton, &QPushButton::clicked, page,
                     [page, appSettings, editor, list, currentId]() {
                         const QString id = currentId();
                         if (id.isEmpty()) {
                             return;
                         }
                         showDataSourceDialog(page, appSettings, editor, id, QStringLiteral("global"));
                         reloadList(list, appSettings);
                     });
    QObject::connect(list, &QListWidget::itemDoubleClicked, page,
                     [page, appSettings, editor, list, currentId](QListWidgetItem *) {
                         const QString id = currentId();
                         if (id.isEmpty()) {
                             return;
                         }
                         showDataSourceDialog(page, appSettings, editor, id, QStringLiteral("global"));
                         reloadList(list, appSettings);
                     });
    QObject::connect(duplicateButton, &QPushButton::clicked, page,
                     [appSettings, list, currentId]() {
                         const QString id = currentId();
                         if (id.isEmpty()) {
                             return;
                         }
                         appSettings->duplicateDatabaseSource(id);
                         reloadList(list, appSettings);
                     });
    QObject::connect(removeButton, &QPushButton::clicked, page, [appSettings, list, currentId]() {
        const QString id = currentId();
        if (id.isEmpty()) {
            return;
        }
        appSettings->removeDatabaseSource(id);
        reloadList(list, appSettings);
    });

    return page;
}

} // namespace ui_shell
