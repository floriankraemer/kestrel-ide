// C5 (ADR-0056): the container-kind run-config page, split out of
// run_config_dialog.cpp under the file-size ratchet.

#include "run_config_container_pages.h"

#include <QAction>
#include <QCheckBox>
#include <QComboBox>
#include <QFileDialog>
#include <QFormLayout>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QInputDialog>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMenu>
#include <QPushButton>
#include <QTableWidget>
#include <QTableWidgetItem>
#include <QToolButton>
#include <QVBoxLayout>

#include <algorithm>

namespace ui_shell {

namespace {

// One column's cell text, for a plain (no per-cell widget) row table.
QString cellText(QTableWidget *table, int row, int column)
{
    const auto *item = table->item(row, column);
    return item == nullptr ? QString() : item->text();
}

void appendPlainRow(QTableWidget *table, const QStringList &values)
{
    const int row = table->rowCount();
    table->insertRow(row);
    for (int column = 0; column < values.size(); ++column) {
        table->setItem(row, column, new QTableWidgetItem(values.at(column)));
    }
}

// Builds a table with `headers.size()` plain-text columns and an Add/Remove
// strip beneath it, inside `parent`'s layout — the same shape
// `file_associations_page.cpp`'s `buildRuleTable` already established for a
// per-cell-widget table; this one keeps every cell plain text since none of
// C5's tables need a combo per row.
QTableWidget *buildRowTable(QWidget *parent, QVBoxLayout *into, const QStringList &headers)
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

    QObject::connect(addButton, &QPushButton::clicked, table, [table]() {
        appendPlainRow(table, QStringList(table->columnCount(), QString()));
    });
    QObject::connect(removeButton, &QPushButton::clicked, table, [table]() {
        QVector<int> rowsToRemove;
        for (const QModelIndex &index : table->selectionModel()->selectedRows()) {
            rowsToRemove.push_back(index.row());
        }
        std::sort(rowsToRemove.begin(), rowsToRemove.end(), std::greater<int>());
        for (const int row : rowsToRemove) {
            table->removeRow(row);
        }
    });
    return table;
}

QComboBox *buildCombo(QWidget *parent, const QVector<QPair<QString, QString>> &items)
{
    auto *combo = new QComboBox(parent);
    for (const auto &item : items) {
        combo->addItem(item.first, item.second);
    }
    return combo;
}

void selectData(QComboBox *combo, const QString &data)
{
    const int index = combo->findData(data.isEmpty() ? combo->itemData(0) : QVariant(data));
    combo->setCurrentIndex(index >= 0 ? index : 0);
}

// A group box whose visibility is toggled by a checkable action in the
// "Modify options ▾" menu — the disclosure pattern: an option group stays
// out of the way until the user asks to see it, and starts visible when
// loading a row that already has content in it.
QWidget *disclosureGroup(QWidget *parent, const QString &title, QMenu *modifyMenu,
                        std::function<QWidget *(QWidget *)> buildContent)
{
    auto *box = new QGroupBox(title, parent);
    auto *layout = new QVBoxLayout(box);
    layout->addWidget(buildContent(box));

    QAction *toggle = modifyMenu->addAction(title);
    toggle->setCheckable(true);
    box->setVisible(false);
    QObject::connect(toggle, &QAction::toggled, box, &QWidget::setVisible);
    QObject::connect(box, &QWidget::destroyed, toggle, &QObject::deleteLater);
    // Stash the action on the box itself so `setOptions` can flip it (and
    // therefore the group's visibility) without the caller keeping a
    // separate map from group to action.
    box->setProperty("modifyAction", QVariant::fromValue(static_cast<QObject *>(toggle)));
    return box;
}

void showGroup(QWidget *group, bool visible)
{
    if (group == nullptr) {
        return;
    }
    if (auto *action = qvariant_cast<QObject *>(group->property("modifyAction"))) {
        qobject_cast<QAction *>(action)->setChecked(visible);
    }
}

} // namespace

ContainerOptionsPage::ContainerOptionsPage(ContainerService *containerService,
                                          RunConfigEditor *editor, QWidget *parent)
  : QWidget(parent)
  , containerService_(containerService)
  , editor_(editor)
{
    buildUi();
    QObject::connect(editor_, &RunConfigEditor::composeServicesReady, this,
                     [this](const QString &services) { onComposeServicesReady(services); });
}

void ContainerOptionsPage::refreshConnections()
{
    const QString previous = serverCombo_->currentData().toString();
    serverCombo_->clear();
    for (const FfiConnectionSummary &connection : containerService_->connections()) {
        serverCombo_->addItem(
          QStringLiteral("%1 (%2)").arg(QString(connection.name), QString(connection.engine)),
          QString(connection.id));
    }
    const int index = serverCombo_->findData(previous);
    if (index >= 0) {
        serverCombo_->setCurrentIndex(index);
    }
}

void ContainerOptionsPage::buildUi()
{
    auto *root = new QVBoxLayout(this);

    serverCombo_ = new QComboBox(this);
    auto *serverRow = new QFormLayout();
    serverRow->addRow(tr("Server:"), serverCombo_);
    root->addLayout(serverRow);
    QObject::connect(serverCombo_, &QComboBox::currentIndexChanged, this,
                     [this]() { emit changed(); requestServices(); });

    modifyOptionsButton_ = new QToolButton(this);
    modifyOptionsButton_->setText(tr("Modify options ▾"));
    modifyOptionsButton_->setPopupMode(QToolButton::InstantPopup);
    auto *modifyMenu = new QMenu(modifyOptionsButton_);
    modifyOptionsButton_->setMenu(modifyMenu);
    root->addWidget(modifyOptionsButton_, 0, Qt::AlignLeft);

    // --- Image / Containerfile shared section -----------------------
    imageContainerfileSection_ = new QWidget(this);
    auto *sharedForm = new QFormLayout(imageContainerfileSection_);
    imageEdit_ = new QLineEdit(imageContainerfileSection_);
    sharedForm->addRow(tr("Image:"), imageEdit_);
    containerNameEdit_ = new QLineEdit(imageContainerfileSection_);
    sharedForm->addRow(tr("Container name:"), containerNameEdit_);
    publishAllPortsCheck_ = new QCheckBox(tr("Publish all exposed ports"), imageContainerfileSection_);
    sharedForm->addRow(QString(), publishAllPortsCheck_);
    pullPolicyCombo_ = buildCombo(imageContainerfileSection_,
                                 { { tr("If missing"), QStringLiteral("missing") },
                                   { tr("Always"), QStringLiteral("always") },
                                   { tr("Never"), QStringLiteral("never") } });
    sharedForm->addRow(tr("Pull policy:"), pullPolicyCombo_);
    runAttachCheck_ = new QCheckBox(tr("Attach console (instead of running detached)"),
                                   imageContainerfileSection_);
    sharedForm->addRow(QString(), runAttachCheck_);
    entrypointEdit_ = new QLineEdit(imageContainerfileSection_);
    commandEdit_ = new QLineEdit(imageContainerfileSection_);
    runOptionsEdit_ = new QLineEdit(imageContainerfileSection_);

    portsGroup_ = disclosureGroup(imageContainerfileSection_, tr("Port bindings"), modifyMenu,
                                 [this](QWidget *box) {
                                     auto *layout = qobject_cast<QVBoxLayout *>(box->layout());
                                     portsTable_ = buildRowTable(
                                       box, layout,
                                       { tr("Host IP"), tr("Host port"), tr("Container port"),
                                         tr("Protocol") });
                                     return portsTable_;
                                 });
    mountsGroup_ = disclosureGroup(imageContainerfileSection_, tr("Bind mounts"), modifyMenu,
                                  [this](QWidget *box) {
                                      auto *layout = qobject_cast<QVBoxLayout *>(box->layout());
                                      mountsTable_ = buildRowTable(
                                        box, layout,
                                        { tr("Host path"), tr("Container path"), tr("Read only (true/false)") });
                                      return mountsTable_;
                                  });
    envGroup_ = disclosureGroup(imageContainerfileSection_, tr("Environment"), modifyMenu,
                               [this](QWidget *box) {
                                   auto *layout = qobject_cast<QVBoxLayout *>(box->layout());
                                   envTable_ = buildRowTable(box, layout, { tr("Key"), tr("Value") });
                                   return envTable_;
                               });

    sharedForm->addRow(tr("Entrypoint:"), entrypointEdit_);
    sharedForm->addRow(tr("Command:"), commandEdit_);
    sharedForm->addRow(tr("Extra run options:"), runOptionsEdit_);
    sharedForm->addRow(portsGroup_);
    sharedForm->addRow(mountsGroup_);
    sharedForm->addRow(envGroup_);
    root->addWidget(imageContainerfileSection_);

    // --- Containerfile-only section ----------------------------------
    containerfileOnlySection_ = new QWidget(this);
    auto *cfForm = new QFormLayout(containerfileOnlySection_);
    dockerfileEdit_ = new QLineEdit(containerfileOnlySection_);
    cfForm->addRow(tr("Dockerfile:"), dockerfileEdit_);
    contextDirEdit_ = new QLineEdit(containerfileOnlySection_);
    cfForm->addRow(tr("Context directory:"), contextDirEdit_);
    imageTagEdit_ = new QLineEdit(containerfileOnlySection_);
    cfForm->addRow(tr("Image tag:"), imageTagEdit_);
    buildOptionsEdit_ = new QLineEdit(containerfileOnlySection_);
    cfForm->addRow(tr("Extra build options:"), buildOptionsEdit_);
    runBuiltImageCheck_ = new QCheckBox(tr("Run built image"), containerfileOnlySection_);
    cfForm->addRow(QString(), runBuiltImageCheck_);
    buildArgsGroup_ = disclosureGroup(containerfileOnlySection_, tr("Build arguments"), modifyMenu,
                                    [this](QWidget *box) {
                                        auto *layout = qobject_cast<QVBoxLayout *>(box->layout());
                                        buildArgsTable_ =
                                          buildRowTable(box, layout, { tr("Key"), tr("Value") });
                                        return buildArgsTable_;
                                    });
    cfForm->addRow(buildArgsGroup_);
    root->addWidget(containerfileOnlySection_);

    // --- Compose-only section -----------------------------------------
    composeSection_ = new QWidget(this);
    auto *composeLayout = new QVBoxLayout(composeSection_);
    composeLayout->setContentsMargins(0, 0, 0, 0);

    composeLayout->addWidget(new QLabel(tr("Compose files:"), composeSection_));
    composeFilesList_ = new QListWidget(composeSection_);
    composeFilesList_->setMaximumHeight(90);
    composeLayout->addWidget(composeFilesList_);
    auto *filesButtons = new QHBoxLayout();
    auto *addFileButton = new QPushButton(tr("Add..."), composeSection_);
    auto *removeFileButton = new QPushButton(tr("Remove"), composeSection_);
    auto *upFileButton = new QPushButton(tr("Up"), composeSection_);
    auto *downFileButton = new QPushButton(tr("Down"), composeSection_);
    filesButtons->addWidget(addFileButton);
    filesButtons->addWidget(removeFileButton);
    filesButtons->addWidget(upFileButton);
    filesButtons->addWidget(downFileButton);
    filesButtons->addStretch(1);
    composeLayout->addLayout(filesButtons);
    QObject::connect(addFileButton, &QPushButton::clicked, this, [this]() {
        const QString file = QFileDialog::getOpenFileName(
          this, tr("Add Compose File"), QString(),
          tr("Compose files (*compose*.yml *compose*.yaml);;All files (*)"));
        if (!file.isEmpty()) {
            composeFilesList_->addItem(file);
            emit changed();
            requestServices();
        }
    });
    QObject::connect(removeFileButton, &QPushButton::clicked, this, [this]() {
        qDeleteAll(composeFilesList_->selectedItems());
        emit changed();
        requestServices();
    });
    QObject::connect(upFileButton, &QPushButton::clicked, this, [this]() {
        const int row = composeFilesList_->currentRow();
        if (row > 0) {
            auto *item = composeFilesList_->takeItem(row);
            composeFilesList_->insertItem(row - 1, item);
            composeFilesList_->setCurrentRow(row - 1);
            emit changed();
        }
    });
    QObject::connect(downFileButton, &QPushButton::clicked, this, [this]() {
        const int row = composeFilesList_->currentRow();
        if (row >= 0 && row < composeFilesList_->count() - 1) {
            auto *item = composeFilesList_->takeItem(row);
            composeFilesList_->insertItem(row + 1, item);
            composeFilesList_->setCurrentRow(row + 1);
            emit changed();
        }
    });

    composeLayout->addWidget(new QLabel(tr("Services (none selected = every service):"), composeSection_));
    servicesList_ = new QListWidget(composeSection_);
    servicesList_->setMaximumHeight(90);
    composeLayout->addWidget(servicesList_);
    QObject::connect(servicesList_, &QListWidget::itemChanged, this, [this]() { emit changed(); });

    auto *composeForm = new QFormLayout();
    projectNameEdit_ = new QLineEdit(composeSection_);
    composeForm->addRow(tr("Project name:"), projectNameEdit_);
    composeLayout->addLayout(composeForm);

    advancedComposeGroup_ =
      disclosureGroup(composeSection_, tr("Advanced compose options"), modifyMenu, [this](QWidget *box) {
          auto *container = new QWidget(box);
          auto *form = new QFormLayout(container);
          profilesEdit_ = new QLineEdit(container);
          form->addRow(tr("Profiles:"), profilesEdit_);
          envFilesEdit_ = new QLineEdit(container);
          form->addRow(tr("Env files:"), envFilesEdit_);
          compatibilityCheck_ = new QCheckBox(tr("Compatibility mode"), container);
          form->addRow(QString(), compatibilityCheck_);
          removeOrphansOnDownCheck_ = new QCheckBox(tr("Remove orphans on Down"), container);
          form->addRow(QString(), removeOrphansOnDownCheck_);
          removeVolumesOnDownCheck_ = new QCheckBox(tr("Remove volumes on Down"), container);
          form->addRow(QString(), removeVolumesOnDownCheck_);
          removeImagesOnDownCombo_ =
            buildCombo(container, { { tr("None"), QStringLiteral("none") },
                                    { tr("All"), QStringLiteral("all") },
                                    { tr("Local"), QStringLiteral("local") } });
          form->addRow(tr("Remove images on Down:"), removeImagesOnDownCombo_);
          sigkillTimeoutEdit_ = new QLineEdit(container);
          form->addRow(tr("SIGKILL timeout (s):"), sigkillTimeoutEdit_);
          exitCodeFromEdit_ = new QLineEdit(container);
          form->addRow(tr("Exit code from service:"), exitCodeFromEdit_);
          alwaysRecreateDepsCheck_ = new QCheckBox(tr("Always recreate dependencies"), container);
          form->addRow(QString(), alwaysRecreateDepsCheck_);
          renewAnonVolumesCheck_ = new QCheckBox(tr("Renew anonymous volumes"), container);
          form->addRow(QString(), renewAnonVolumesCheck_);
          removeOrphansCheck_ = new QCheckBox(tr("Remove orphan containers"), container);
          form->addRow(QString(), removeOrphansCheck_);
          noLogPrefixCheck_ = new QCheckBox(tr("No log prefix"), container);
          form->addRow(QString(), noLogPrefixCheck_);
          startCombo_ = buildCombo(container,
                                   { { tr("Selected and dependencies"), QStringLiteral("selected_and_deps") },
                                     { tr("None"), QStringLiteral("none") },
                                     { tr("Selected only"), QStringLiteral("selected_only") } });
          form->addRow(tr("Start:"), startCombo_);
          composeAttachCombo_ =
            buildCombo(container, { { tr("Selected"), QStringLiteral("selected") },
                                    { tr("None"), QStringLiteral("none") },
                                    { tr("Selected and dependencies"), QStringLiteral("selected_and_deps") } });
          form->addRow(tr("Attach:"), composeAttachCombo_);
          recreateCombo_ = buildCombo(container, { { tr("Changed"), QStringLiteral("changed") },
                                                   { tr("All"), QStringLiteral("all") },
                                                   { tr("None"), QStringLiteral("none") } });
          form->addRow(tr("Recreate:"), recreateCombo_);
          buildCombo_ = buildCombo(container, { { tr("If missing"), QStringLiteral("missing") },
                                                { tr("Never"), QStringLiteral("never") },
                                                { tr("Always"), QStringLiteral("always") } });
          form->addRow(tr("Build:"), buildCombo_);
          abortOnExitCheck_ = new QCheckBox(tr("Abort on container exit"), container);
          form->addRow(QString(), abortOnExitCheck_);
          return container;
      });
    composeLayout->addWidget(advancedComposeGroup_);

    scaleGroup_ = disclosureGroup(composeSection_, tr("Scale"), modifyMenu, [this](QWidget *box) {
        auto *layout = qobject_cast<QVBoxLayout *>(box->layout());
        scaleTable_ = buildRowTable(box, layout, { tr("Service"), tr("Replicas") });
        return scaleTable_;
    });
    composeLayout->addWidget(scaleGroup_);

    root->addWidget(composeSection_);
    root->addStretch(1);

    refreshConnections();

    // Every widget above re-emits `changed()` on edit, generically: rather
    // than one connect per field, walk the ones that matter for the
    // preview. Line edits/checkboxes/combos are covered here; the tables
    // and lists already connect their own Add/Remove/edit paths above.
    for (QLineEdit *edit : { imageEdit_, containerNameEdit_, entrypointEdit_, commandEdit_,
                            runOptionsEdit_, dockerfileEdit_, contextDirEdit_, imageTagEdit_,
                            buildOptionsEdit_, projectNameEdit_ }) {
        QObject::connect(edit, &QLineEdit::textChanged, this, [this]() { emit changed(); });
    }
    for (QCheckBox *check : { publishAllPortsCheck_, runAttachCheck_, runBuiltImageCheck_ }) {
        QObject::connect(check, &QCheckBox::toggled, this, [this]() { emit changed(); });
    }
    QObject::connect(pullPolicyCombo_, &QComboBox::currentIndexChanged, this,
                     [this]() { emit changed(); });
    for (QTableWidget *table : { portsTable_, mountsTable_, envTable_, buildArgsTable_ }) {
        QObject::connect(table, &QTableWidget::itemChanged, this, [this]() { emit changed(); });
        QObject::connect(table->model(), &QAbstractItemModel::rowsRemoved, this,
                         [this]() { emit changed(); });
    }
    QObject::connect(composeFilesList_->model(), &QAbstractItemModel::rowsInserted, this,
                     [this]() { emit changed(); });
    QObject::connect(composeFilesList_->model(), &QAbstractItemModel::rowsRemoved, this,
                     [this]() { emit changed(); });
}

void ContainerOptionsPage::setKind(const QString &kind)
{
    kind_ = kind;
    imageContainerfileSection_->setVisible(kind == QLatin1String("container-image")
                                          || kind == QLatin1String("containerfile"));
    imageEdit_->setVisible(kind == QLatin1String("container-image"));
    containerfileOnlySection_->setVisible(kind == QLatin1String("containerfile"));
    composeSection_->setVisible(kind == QLatin1String("compose"));
    setVisible(!kind.isEmpty());
}

void ContainerOptionsPage::requestServices()
{
    if (kind_ != QLatin1String("compose")) {
        return;
    }
    QStringList files;
    for (int row = 0; row < composeFilesList_->count(); ++row) {
        files << composeFilesList_->item(row)->text();
    }
    editor_->requestComposeServices(serverCombo_->currentData().toString(),
                                    files.join(QLatin1Char('\n')));
}

void ContainerOptionsPage::onComposeServicesReady(const QString &services)
{
    knownServices_ = services.split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    const QSignalBlocker blocker(servicesList_);
    servicesList_->clear();
    for (const QString &service : knownServices_) {
        auto *item = new QListWidgetItem(service, servicesList_);
        item->setFlags(item->flags() | Qt::ItemIsUserCheckable);
        item->setCheckState(selectedServices_.contains(service) ? Qt::Checked : Qt::Unchecked);
    }
}

void ContainerOptionsPage::setOptions(const FfiContainerOptions &options)
{
    const QSignalBlocker blockers[] = {
        QSignalBlocker(imageEdit_),          QSignalBlocker(containerNameEdit_),
        QSignalBlocker(publishAllPortsCheck_), QSignalBlocker(entrypointEdit_),
        QSignalBlocker(commandEdit_),        QSignalBlocker(runOptionsEdit_),
        QSignalBlocker(runAttachCheck_),     QSignalBlocker(pullPolicyCombo_),
        QSignalBlocker(dockerfileEdit_),     QSignalBlocker(contextDirEdit_),
        QSignalBlocker(imageTagEdit_),       QSignalBlocker(buildOptionsEdit_),
        QSignalBlocker(runBuiltImageCheck_), QSignalBlocker(projectNameEdit_),
    };

    imageEdit_->setText(QString(options.image));
    containerNameEdit_->setText(QString(options.container_name));
    publishAllPortsCheck_->setChecked(options.publish_all_ports);
    entrypointEdit_->setText(QString(options.entrypoint));
    commandEdit_->setText(QString(options.command));
    runOptionsEdit_->setText(QString(options.run_options));
    runAttachCheck_->setChecked(options.run_attach);
    selectData(pullPolicyCombo_, QString(options.pull_policy));

    portsTable_->setRowCount(0);
    for (const FfiPortBinding &port : options.port_bindings) {
        appendPlainRow(portsTable_, { QString(port.host_ip), QString(port.host_port),
                                     QString(port.container_port), QString(port.protocol) });
    }
    mountsTable_->setRowCount(0);
    for (const FfiBindMount &mount : options.bind_mounts) {
        appendPlainRow(mountsTable_, { QString(mount.host_path), QString(mount.container_path),
                                      mount.read_only ? QStringLiteral("true") : QStringLiteral("false") });
    }
    envTable_->setRowCount(0);
    for (const FfiKeyValue &pair : options.env) {
        appendPlainRow(envTable_, { QString(pair.key), QString(pair.value) });
    }

    dockerfileEdit_->setText(QString(options.dockerfile));
    contextDirEdit_->setText(QString(options.context_dir));
    imageTagEdit_->setText(QString(options.image_tag));
    buildOptionsEdit_->setText(QString(options.build_options));
    runBuiltImageCheck_->setChecked(options.run_built_image);
    buildArgsTable_->setRowCount(0);
    for (const FfiKeyValue &pair : options.build_args) {
        appendPlainRow(buildArgsTable_, { QString(pair.key), QString(pair.value) });
    }

    composeFilesList_->clear();
    composeFilesList_->addItems(
      QString(options.compose_files).split(QLatin1Char('\n'), Qt::SkipEmptyParts));
    selectedServices_ = QString(options.services).split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    projectNameEdit_->setText(QString(options.project_name));
    profilesEdit_->setText(QString(options.profiles));
    envFilesEdit_->setText(QString(options.env_files));
    compatibilityCheck_->setChecked(options.compatibility);
    removeOrphansOnDownCheck_->setChecked(options.remove_orphans_on_down);
    removeVolumesOnDownCheck_->setChecked(options.remove_volumes_on_down);
    selectData(removeImagesOnDownCombo_, QString(options.remove_images_on_down));
    sigkillTimeoutEdit_->setText(QString(options.sigkill_timeout));
    exitCodeFromEdit_->setText(QString(options.exit_code_from));
    scaleTable_->setRowCount(0);
    for (const FfiScaleEntry &entry : options.scale) {
        appendPlainRow(scaleTable_, { QString(entry.service), QString::number(entry.count) });
    }
    alwaysRecreateDepsCheck_->setChecked(options.always_recreate_deps);
    renewAnonVolumesCheck_->setChecked(options.renew_anon_volumes);
    removeOrphansCheck_->setChecked(options.remove_orphans);
    noLogPrefixCheck_->setChecked(options.no_log_prefix);
    selectData(startCombo_, QString(options.start));
    selectData(composeAttachCombo_, QString(options.compose_attach));
    selectData(recreateCombo_, QString(options.recreate));
    selectData(buildCombo_, QString(options.build));
    abortOnExitCheck_->setChecked(options.abort_on_container_exit);

    const int serverIndex = serverCombo_->findData(QString(options.connection_id));
    serverCombo_->setCurrentIndex(serverIndex >= 0 ? serverIndex : 0);

    // Reveal a group up front when it already has content — otherwise a
    // saved configuration's own ports/mounts/env would be hidden by
    // default and look lost.
    showGroup(portsGroup_, !options.port_bindings.empty());
    showGroup(mountsGroup_, !options.bind_mounts.empty());
    showGroup(envGroup_, !options.env.empty());
    showGroup(buildArgsGroup_, !options.build_args.empty());
    showGroup(scaleGroup_, !options.scale.empty());

    requestServices();
}

FfiContainerOptions ContainerOptionsPage::options() const
{
    FfiContainerOptions options;
    options.connection_id = serverCombo_->currentData().toString();
    options.image = imageEdit_->text();
    options.container_name = containerNameEdit_->text();
    options.publish_all_ports = publishAllPortsCheck_->isChecked();
    options.entrypoint = entrypointEdit_->text();
    options.command = commandEdit_->text();
    options.run_options = runOptionsEdit_->text();
    options.run_attach = runAttachCheck_->isChecked();
    options.pull_policy = pullPolicyCombo_->currentData().toString();

    for (int row = 0; row < portsTable_->rowCount(); ++row) {
        options.port_bindings.push_back(
          FfiPortBinding{ cellText(portsTable_, row, 0), cellText(portsTable_, row, 1),
                         cellText(portsTable_, row, 2), cellText(portsTable_, row, 3) });
    }
    for (int row = 0; row < mountsTable_->rowCount(); ++row) {
        const QString readOnly = cellText(mountsTable_, row, 2).trimmed().toLower();
        options.bind_mounts.push_back(FfiBindMount{ cellText(mountsTable_, row, 0),
                                                    cellText(mountsTable_, row, 1),
                                                    readOnly == QLatin1String("true")
                                                      || readOnly == QLatin1String("1") });
    }
    for (int row = 0; row < envTable_->rowCount(); ++row) {
        options.env.push_back(FfiKeyValue{ cellText(envTable_, row, 0), cellText(envTable_, row, 1) });
    }

    options.dockerfile = dockerfileEdit_->text();
    options.context_dir = contextDirEdit_->text();
    options.image_tag = imageTagEdit_->text();
    options.build_options = buildOptionsEdit_->text();
    options.run_built_image = runBuiltImageCheck_->isChecked();
    for (int row = 0; row < buildArgsTable_->rowCount(); ++row) {
        options.build_args.push_back(
          FfiKeyValue{ cellText(buildArgsTable_, row, 0), cellText(buildArgsTable_, row, 1) });
    }

    QStringList files;
    for (int row = 0; row < composeFilesList_->count(); ++row) {
        files << composeFilesList_->item(row)->text();
    }
    options.compose_files = files.join(QLatin1Char('\n'));
    QStringList selected;
    for (int row = 0; row < servicesList_->count(); ++row) {
        const auto *item = servicesList_->item(row);
        if (item->checkState() == Qt::Checked) {
            selected << item->text();
        }
    }
    options.services = selected.join(QLatin1Char('\n'));
    options.project_name = projectNameEdit_->text();
    options.profiles = profilesEdit_->text();
    options.env_files = envFilesEdit_->text();
    options.compatibility = compatibilityCheck_->isChecked();
    options.remove_orphans_on_down = removeOrphansOnDownCheck_->isChecked();
    options.remove_volumes_on_down = removeVolumesOnDownCheck_->isChecked();
    options.remove_images_on_down = removeImagesOnDownCombo_->currentData().toString();
    options.sigkill_timeout = sigkillTimeoutEdit_->text();
    options.exit_code_from = exitCodeFromEdit_->text();
    for (int row = 0; row < scaleTable_->rowCount(); ++row) {
        options.scale.push_back(
          FfiScaleEntry{ cellText(scaleTable_, row, 0),
                        static_cast<quint32>(cellText(scaleTable_, row, 1).toUInt()) });
    }
    options.always_recreate_deps = alwaysRecreateDepsCheck_->isChecked();
    options.renew_anon_volumes = renewAnonVolumesCheck_->isChecked();
    options.remove_orphans = removeOrphansCheck_->isChecked();
    options.no_log_prefix = noLogPrefixCheck_->isChecked();
    options.start = startCombo_->currentData().toString();
    options.compose_attach = composeAttachCombo_->currentData().toString();
    options.recreate = recreateCombo_->currentData().toString();
    options.build = buildCombo_->currentData().toString();
    options.abort_on_container_exit = abortOnExitCheck_->isChecked();
    return options;
}

} // namespace ui_shell
