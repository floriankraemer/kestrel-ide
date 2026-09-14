// The container node's Dashboard (C9): editable Env/Ports/Mounts tables
// and "Recreate with changes" — split out of `containers_detail.cpp`
// under the file-size ratchet, the same way `containers_actions.cpp`
// splits `ContainersPanel`'s methods across files. Humble view: whether
// recreate is even offered is `ContainerService::nodeActions`'s answer
// (`canRecreate`, false for a compose-managed container); this file only
// builds the tables, tracks whether they differ from what was loaded, and
// dispatches the click.

#include "containers_detail.h"

#include <QAbstractItemView>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QMessageBox>
#include <QPushButton>
#include <QTableWidget>
#include <QTableWidgetItem>
#include <QVBoxLayout>
#include <QVector>

#include <algorithm>
#include <functional>

namespace ui_shell {

namespace {

QLabel *readOnlyValue(QWidget *parent)
{
    auto *label = new QLabel(parent);
    label->setTextInteractionFlags(Qt::TextSelectableByMouse);
    label->setWordWrap(true);
    return label;
}

// A plain-text table with an Add/Remove strip beneath it — the same shape
// `run_config_container_pages.cpp`'s `buildRowTable` already established,
// duplicated rather than shared through a header (that file's copy is
// file-local, and a shared header would outweigh one ~25-line helper —
// the same tradeoff `containers_actions.cpp`'s own item-data-role comment
// already documents for this codebase).
QTableWidget *buildRowTable(QWidget *parent, QVBoxLayout *into, const QStringList &headers,
                            const std::function<void()> &onChanged)
{
    auto *table = new QTableWidget(0, headers.size(), parent);
    table->setHorizontalHeaderLabels(headers);
    table->horizontalHeader()->setSectionResizeMode(0, QHeaderView::Stretch);
    table->verticalHeader()->setVisible(false);
    table->setSelectionBehavior(QAbstractItemView::SelectRows);
    table->setMaximumHeight(120);
    into->addWidget(table);

    auto *buttonRow = new QHBoxLayout();
    auto *addButton = new QPushButton(QObject::tr("Add"), parent);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), parent);
    buttonRow->addWidget(addButton);
    buttonRow->addWidget(removeButton);
    buttonRow->addStretch(1);
    into->addLayout(buttonRow);

    QObject::connect(addButton, &QPushButton::clicked, table, [table, onChanged]() {
        const int row = table->rowCount();
        table->insertRow(row);
        for (int column = 0; column < table->columnCount(); ++column) {
            table->setItem(row, column, new QTableWidgetItem());
        }
        onChanged();
    });
    QObject::connect(removeButton, &QPushButton::clicked, table, [table, onChanged]() {
        QVector<int> rowsToRemove;
        for (const QModelIndex &index : table->selectionModel()->selectedRows()) {
            rowsToRemove.push_back(index.row());
        }
        std::sort(rowsToRemove.begin(), rowsToRemove.end(), std::greater<int>());
        for (const int row : rowsToRemove) {
            table->removeRow(row);
        }
        onChanged();
    });
    QObject::connect(table, &QTableWidget::cellChanged, table,
                     [onChanged](int, int) { onChanged(); });
    return table;
}

QString cell(QTableWidget *table, int row, int column)
{
    const auto *item = table->item(row, column);
    return item == nullptr ? QString() : item->text().trimmed();
}

// `\n`-joined `"KEY=VALUE"` lines — `container_core::recreate::
// format_env_lines`'s own format.
QString envLines(QTableWidget *table)
{
    QStringList lines;
    for (int row = 0; row < table->rowCount(); ++row) {
        const QString key = cell(table, row, 0);
        if (key.isEmpty()) {
            continue;
        }
        lines << key + QLatin1Char('=') + cell(table, row, 1);
    }
    return lines.join(QLatin1Char('\n'));
}

// `\n`-joined `"[host_ip:]host_port:container_port[/protocol]"` lines —
// `container_core::recreate::format_port_lines`'s own format.
QString portLines(QTableWidget *table)
{
    QStringList lines;
    for (int row = 0; row < table->rowCount(); ++row) {
        const QString hostPort = cell(table, row, 1);
        const QString containerPort = cell(table, row, 2);
        if (hostPort.isEmpty() || containerPort.isEmpty()) {
            continue;
        }
        QString line;
        const QString hostIp = cell(table, row, 0);
        if (!hostIp.isEmpty()) {
            line += hostIp + QLatin1Char(':');
        }
        line += hostPort + QLatin1Char(':') + containerPort;
        if (cell(table, row, 3).compare(QStringLiteral("udp"), Qt::CaseInsensitive) == 0) {
            line += QStringLiteral("/udp");
        }
        lines << line;
    }
    return lines.join(QLatin1Char('\n'));
}

// `\n`-joined `"<kind>|<source>|<target>|<ro>"` lines —
// `container_core::recreate::format_mount_lines`'s own format.
QString mountLines(QTableWidget *table)
{
    QStringList lines;
    for (int row = 0; row < table->rowCount(); ++row) {
        const QString target = cell(table, row, 2);
        if (target.isEmpty()) {
            continue;
        }
        const QString kind =
          cell(table, row, 0).compare(QStringLiteral("volume"), Qt::CaseInsensitive) == 0
            ? QStringLiteral("volume")
            : QStringLiteral("bind");
        const auto *roItem = table->item(row, 3);
        const bool readOnly = roItem != nullptr && roItem->checkState() == Qt::Checked;
        lines << kind + QLatin1Char('|') + cell(table, row, 1) + QLatin1Char('|') + target
                   + QLatin1Char('|') + (readOnly ? QStringLiteral("1") : QStringLiteral("0"));
    }
    return lines.join(QLatin1Char('\n'));
}

void fillEnvTable(QTableWidget *table, const QString &lines)
{
    table->setRowCount(0);
    for (const QString &line : lines.split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        const int eq = line.indexOf(QLatin1Char('='));
        const QString key = eq < 0 ? line : line.left(eq);
        const QString value = eq < 0 ? QString() : line.mid(eq + 1);
        const int row = table->rowCount();
        table->insertRow(row);
        table->setItem(row, 0, new QTableWidgetItem(key));
        table->setItem(row, 1, new QTableWidgetItem(value));
    }
}

void fillPortsTable(QTableWidget *table, const QString &lines)
{
    table->setRowCount(0);
    for (const QString &line : lines.split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        const int lastColon = line.lastIndexOf(QLatin1Char(':'));
        if (lastColon < 0) {
            continue;
        }
        const QString host = line.left(lastColon);
        QString containerAndProto = line.mid(lastColon + 1);
        QString protocol = QStringLiteral("tcp");
        const int slash = containerAndProto.indexOf(QLatin1Char('/'));
        if (slash >= 0) {
            protocol = containerAndProto.mid(slash + 1);
            containerAndProto = containerAndProto.left(slash);
        }
        QString hostIp;
        QString hostPort = host;
        const int hostColon = host.lastIndexOf(QLatin1Char(':'));
        if (hostColon >= 0) {
            hostIp = host.left(hostColon);
            hostPort = host.mid(hostColon + 1);
        }
        const int row = table->rowCount();
        table->insertRow(row);
        table->setItem(row, 0, new QTableWidgetItem(hostIp));
        table->setItem(row, 1, new QTableWidgetItem(hostPort));
        table->setItem(row, 2, new QTableWidgetItem(containerAndProto));
        table->setItem(row, 3, new QTableWidgetItem(protocol));
    }
}

void fillMountsTable(QTableWidget *table, const QString &lines)
{
    table->setRowCount(0);
    for (const QString &line : lines.split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        const QStringList fields = line.split(QLatin1Char('|'));
        if (fields.size() != 4) {
            continue;
        }
        const int row = table->rowCount();
        table->insertRow(row);
        table->setItem(row, 0, new QTableWidgetItem(fields.at(0)));
        table->setItem(row, 1, new QTableWidgetItem(fields.at(1)));
        table->setItem(row, 2, new QTableWidgetItem(fields.at(2)));
        auto *readOnlyItem = new QTableWidgetItem();
        readOnlyItem->setFlags((readOnlyItem->flags() | Qt::ItemIsUserCheckable)
                                & ~Qt::ItemIsEditable);
        readOnlyItem->setCheckState(fields.at(3) == QStringLiteral("1") ? Qt::Checked
                                                                        : Qt::Unchecked);
        table->setItem(row, 3, readOnlyItem);
    }
}

} // namespace

QWidget *ContainerDetailArea::ensureContainerDashboardPage()
{
    if (containerDashboardPage_ != nullptr) {
        return containerDashboardPage_;
    }
    containerDashboardPage_ = new QWidget(tabs_);
    auto *layout = new QVBoxLayout(containerDashboardPage_);

    auto *form = new QFormLayout();
    form->setLabelAlignment(Qt::AlignRight);
    containerDashName_ = readOnlyValue(containerDashboardPage_);
    containerDashId_ = readOnlyValue(containerDashboardPage_);
    containerDashImage_ = readOnlyValue(containerDashboardPage_);
    containerDashStatus_ = readOnlyValue(containerDashboardPage_);
    containerDashNetwork_ = readOnlyValue(containerDashboardPage_);
    containerDashRestartPolicy_ = readOnlyValue(containerDashboardPage_);
    form->addRow(tr("Name"), containerDashName_);
    form->addRow(tr("ID"), containerDashId_);
    form->addRow(tr("Image"), containerDashImage_);
    form->addRow(tr("Status"), containerDashStatus_);
    form->addRow(tr("Network"), containerDashNetwork_);
    form->addRow(tr("Restart policy"), containerDashRestartPolicy_);
    layout->addLayout(form);

    const auto onChanged = [this]() { refreshRecreateButtonState(); };

    layout->addWidget(new QLabel(tr("Env"), containerDashboardPage_));
    containerDashEnv_ =
      buildRowTable(containerDashboardPage_, layout, {tr("Key"), tr("Value")}, onChanged);

    layout->addWidget(new QLabel(tr("Ports"), containerDashboardPage_));
    containerDashPorts_ = buildRowTable(
      containerDashboardPage_, layout,
      {tr("Host IP"), tr("Host Port"), tr("Container Port"), tr("Protocol")}, onChanged);

    layout->addWidget(new QLabel(tr("Mounts"), containerDashboardPage_));
    containerDashMounts_ = buildRowTable(
      containerDashboardPage_, layout, {tr("Type"), tr("Source"), tr("Target"), tr("Read-only")},
      onChanged);
    connect(containerDashMounts_, &QTableWidget::itemChanged, this,
            [this](QTableWidgetItem *) { refreshRecreateButtonState(); });

    containerDashRecreateButton_ = new QPushButton(tr("Recreate with changes"),
                                                   containerDashboardPage_);
    containerDashRecreateButton_->setEnabled(false);
    connect(containerDashRecreateButton_, &QPushButton::clicked, this,
            &ContainerDetailArea::triggerRecreate);
    layout->addWidget(containerDashRecreateButton_, 0, Qt::AlignLeft);
    layout->addStretch(1);

    return containerDashboardPage_;
}

void ContainerDetailArea::populateContainerDashboard()
{
    const FfiContainerDashboard dashboard = containerService_->containerDashboard(nodeId_);
    containerDashName_->setText(QString(dashboard.name));
    containerDashId_->setText(QString(dashboard.id));
    containerDashImage_->setText(QString(dashboard.image));
    containerDashStatus_->setText(QString(dashboard.status));
    containerDashNetwork_->setText(QString(dashboard.network));
    containerDashRestartPolicy_->setText(QString(dashboard.restartPolicy).isEmpty()
                                           ? tr("no")
                                           : QString(dashboard.restartPolicy));

    containerDashOriginalEnv_ = QString(dashboard.env);
    containerDashOriginalPorts_ = QString(dashboard.ports);
    containerDashOriginalMounts_ = QString(dashboard.mounts);

    fillEnvTable(containerDashEnv_, containerDashOriginalEnv_);
    fillPortsTable(containerDashPorts_, containerDashOriginalPorts_);
    fillMountsTable(containerDashMounts_, containerDashOriginalMounts_);

    containerDashCanRecreate_ = containerService_->nodeActions(nodeId_).canRecreate;
    refreshRecreateButtonState();
}

void ContainerDetailArea::refreshRecreateButtonState()
{
    if (containerDashRecreateButton_ == nullptr) {
        return;
    }
    const bool dirty = envLines(containerDashEnv_) != containerDashOriginalEnv_
                     || portLines(containerDashPorts_) != containerDashOriginalPorts_
                     || mountLines(containerDashMounts_) != containerDashOriginalMounts_;
    containerDashRecreateButton_->setEnabled(dirty && containerDashCanRecreate_);
}

void ContainerDetailArea::triggerRecreate()
{
    if (nodeId_.isEmpty()) {
        return;
    }
    QMessageBox confirm(nullptr);
    confirm.setIcon(QMessageBox::Warning);
    confirm.setWindowTitle(tr("Recreate Container"));
    confirm.setText(tr("Recreate \"%1\" with these changes?").arg(QString(containerDashName_->text())));
    confirm.setInformativeText(
      tr("The container will be removed and a new one created in its place. "
         "If the new container fails to start, the old one is already gone."));
    confirm.setStandardButtons(QMessageBox::Cancel);
    QPushButton *recreateButton = confirm.addButton(tr("Recreate"), QMessageBox::AcceptRole);
    confirm.exec();
    if (confirm.clickedButton() != recreateButton) {
        return;
    }
    const FfiResult result = containerService_->recreateContainer(
      nodeId_, envLines(containerDashEnv_), portLines(containerDashPorts_),
      mountLines(containerDashMounts_));
    // A synchronous refusal (compose-managed, unknown node) — the actual
    // recreate runs on a worker thread and reports through
    // `actionFinished`, which `ContainersPanel` already surfaces as its
    // status-bar banner.
    if (result.code != 0) {
        QMessageBox::warning(nullptr, tr("Recreate Container"), QString(result.message));
    }
}

} // namespace ui_shell
