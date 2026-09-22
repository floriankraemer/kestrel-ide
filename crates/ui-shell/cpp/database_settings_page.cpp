#include "database_settings_page.h"

#include "data_source_dialog.h"
#include "driver_install_dialog.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QHBoxLayout>
#include <QLabel>
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

// F8.5: every `adbc`-backend row, its status, and an Install/Re-enable
// button per row — reachable from the "Drivers..." button below.
void showDriversListDialog(QWidget *parent, AppSettings *appSettings,
                            DriverInstallService *driverInstallService)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Database Drivers"));
    dialog.resize(420, 320);
    auto *layout = new QVBoxLayout(&dialog);

    auto *list = new QListWidget(&dialog);
    layout->addWidget(list, 1);

    const auto reload = [list, appSettings, driverInstallService]() {
        list->clear();
        for (const FfiDriverOption &option : appSettings->databaseDrivers()) {
            if (QString(option.backend) != QStringLiteral("adbc")) {
                continue;
            }
            const FfiDriverStatus status = driverInstallService->status(option.id);
            auto *item = new QListWidgetItem(
              QStringLiteral("%1 — %2").arg(QString(option.name), QString(status.text)), list);
            item->setData(Qt::UserRole, QString(option.id));
            item->setData(Qt::UserRole + 1, QString(option.name));
        }
    };
    reload();

    auto *actionButton = new QPushButton(QObject::tr("Install / Re-enable..."), &dialog);
    layout->addWidget(actionButton);

    QObject::connect(actionButton, &QPushButton::clicked, &dialog,
                     [&dialog, list, driverInstallService, reload]() {
                         QListWidgetItem *item = list->currentItem();
                         if (!item) {
                             return;
                         }
                         const QString driverId = item->data(Qt::UserRole).toString();
                         const QString driverName = item->data(Qt::UserRole + 1).toString();
                         const FfiDriverStatus status = driverInstallService->status(driverId);
                         if (status.canReenable) {
                             driverInstallService->reenable(driverId);
                         } else {
                             showDriverInstallDialog(&dialog, driverInstallService, driverId, driverName);
                         }
                         reload();
                     });

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, &dialog);
    layout->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);

    dialog.exec();
}

} // namespace

QWidget *buildDatabaseSettingsPage(QWidget *parent, AppSettings *appSettings,
                                    DataSourceEditor *editor,
                                    DriverInstallService *driverInstallService)
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
    auto *driversButton = new QPushButton(QObject::tr("Drivers..."), page);
    buttonsRow->addWidget(addButton);
    buttonsRow->addWidget(editButton);
    buttonsRow->addWidget(duplicateButton);
    buttonsRow->addWidget(removeButton);
    buttonsRow->addStretch(1);
    buttonsRow->addWidget(driversButton);
    layout->addLayout(buttonsRow);

    auto *allowThirdParty =
      new QCheckBox(QObject::tr("Allow installing drivers contributed by third-party plugins"), page);
    allowThirdParty->setChecked(appSettings->allowThirdPartyDrivers());
    layout->addWidget(allowThirdParty);

    const auto currentId = [list]() -> QString {
        QListWidgetItem *item = list->currentItem();
        return item ? item->data(Qt::UserRole).toString() : QString();
    };

    QObject::connect(addButton, &QPushButton::clicked, page,
                     [page, appSettings, editor, driverInstallService, list]() {
                         showDataSourceDialog(page, appSettings, editor, driverInstallService,
                                               QString(), QStringLiteral("global"));
                         reloadList(list, appSettings);
                     });
    QObject::connect(editButton, &QPushButton::clicked, page,
                     [page, appSettings, editor, driverInstallService, list, currentId]() {
                         const QString id = currentId();
                         if (id.isEmpty()) {
                             return;
                         }
                         showDataSourceDialog(page, appSettings, editor, driverInstallService, id,
                                               QStringLiteral("global"));
                         reloadList(list, appSettings);
                     });
    QObject::connect(list, &QListWidget::itemDoubleClicked, page,
                     [page, appSettings, editor, driverInstallService, list, currentId](QListWidgetItem *) {
                         const QString id = currentId();
                         if (id.isEmpty()) {
                             return;
                         }
                         showDataSourceDialog(page, appSettings, editor, driverInstallService, id,
                                               QStringLiteral("global"));
                         reloadList(list, appSettings);
                     });
    QObject::connect(driversButton, &QPushButton::clicked, page,
                     [page, appSettings, driverInstallService]() {
                         showDriversListDialog(page, appSettings, driverInstallService);
                     });
    QObject::connect(allowThirdParty, &QCheckBox::toggled, page,
                     [appSettings](bool on) { appSettings->setAllowThirdPartyDrivers(on); });
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
