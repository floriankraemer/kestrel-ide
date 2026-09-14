// New Target wizard (C8, ADR-0056's "Run targets" section).

#include "container_target_wizard.h"

#include <QComboBox>
#include <QCompleter>
#include <QFileDialog>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QStackedWidget>
#include <QStringListModel>
#include <QTableWidget>
#include <QTableWidgetItem>
#include <QVBoxLayout>
#include <QWizard>
#include <QWizardPage>

#include <algorithm>

namespace ui_shell {

namespace {

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

// Same shape as `run_config_container_pages.cpp`'s own `buildRowTable`: a
// plain-text table with an Add/Remove strip, kept small and duplicated here
// rather than shared — three helper functions do not earn a third header.
QTableWidget *buildRowTable(QWidget *parent, QVBoxLayout *into, const QStringList &headers)
{
    auto *table = new QTableWidget(0, headers.size(), parent);
    table->setHorizontalHeaderLabels(headers);
    table->horizontalHeader()->setSectionResizeMode(0, QHeaderView::Stretch);
    table->verticalHeader()->setVisible(false);
    table->setSelectionBehavior(QAbstractItemView::SelectRows);
    table->setMaximumHeight(110);
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

// `"image"`, `"containerfile"`, `"compose-service"` — the combo's data role,
// the exact string `container_core::target` reads.
const QStringList kSourceIds = { QStringLiteral("image"), QStringLiteral("containerfile"),
                                 QStringLiteral("compose-service") };

int sourceIndex(const QString &source)
{
    const int index = kSourceIds.indexOf(source);
    return index >= 0 ? index : 0;
}

} // namespace

bool showContainerTargetWizard(QWidget *parent, ContainerService *containerService,
                               RunConfigEditor *editor, const FfiContainerTarget &prefill,
                               FfiContainerTarget &target)
{
    QWizard wizard(parent);
    wizard.setWindowTitle(QObject::tr("New Run Target"));
    wizard.setWizardStyle(QWizard::ModernStyle);
    wizard.resize(560, 460);

    // ---- Page 1: connection + source -----------------------------------
    auto *page1 = new QWizardPage(&wizard);
    page1->setTitle(QObject::tr("Where does this run?"));
    auto *page1Layout = new QFormLayout(page1);

    auto *nameEdit = new QLineEdit(page1);
    nameEdit->setText(prefill.name);
    page1Layout->addRow(QObject::tr("Name:"), nameEdit);

    auto *serverCombo = new QComboBox(page1);
    for (const FfiConnectionSummary &connection : containerService->connections()) {
        serverCombo->addItem(
          QStringLiteral("%1 (%2)").arg(QString(connection.name), QString(connection.engine)),
          QString(connection.id));
    }
    const int serverIndex = serverCombo->findData(QString(prefill.connection_id));
    serverCombo->setCurrentIndex(serverIndex >= 0 ? serverIndex : 0);
    page1Layout->addRow(QObject::tr("Server:"), serverCombo);

    auto *sourceCombo = new QComboBox(page1);
    sourceCombo->addItem(QObject::tr("Pull or use an existing image"));
    sourceCombo->addItem(QObject::tr("Build from a Dockerfile"));
    sourceCombo->addItem(QObject::tr("Compose service"));
    sourceCombo->setCurrentIndex(sourceIndex(prefill.source));
    page1Layout->addRow(QObject::tr("Source:"), sourceCombo);

    wizard.addPage(page1);

    // ---- Page 2: source-specific fields ----------------------------------
    auto *page2 = new QWizardPage(&wizard);
    page2->setTitle(QObject::tr("Image"));
    auto *page2Layout = new QVBoxLayout(page2);
    auto *sourceStack = new QStackedWidget(page2);
    page2Layout->addWidget(sourceStack);

    // Image: `ContainerService::imageCompletions` (C4, local names) at
    // once, then Docker Hub's own half (C6) merged in when
    // `imageCompletionsReady` answers — the same `QCompleter` shape
    // `containers_panel.cpp`'s Pull row already drives from
    // `imageCompletions` alone, extended to the Hub-backed source
    // `requestImageCompletions` adds. Re-requested against the connection
    // chosen on page 1, so switching servers re-ranks local images against
    // the right daemon's own snapshot.
    auto *imagePane = new QWidget(sourceStack);
    auto *imageForm = new QFormLayout(imagePane);
    auto *imageEdit = new QLineEdit(imagePane);
    imageEdit->setText(prefill.image);
    imageEdit->setPlaceholderText(QStringLiteral("nginx:1.27"));
    auto *imageCompleter = new QCompleter(imagePane);
    imageCompleter->setCaseSensitivity(Qt::CaseInsensitive);
    imageEdit->setCompleter(imageCompleter);
    const auto requestImageCompletions = [=]() {
        const QStringList local =
          containerService
            ->requestImageCompletions(serverCombo->currentData().toString(), imageEdit->text())
            .split(QLatin1Char('\n'), Qt::SkipEmptyParts);
        imageCompleter->setModel(new QStringListModel(local, imageCompleter));
    };
    QObject::connect(imageEdit, &QLineEdit::textEdited, imagePane,
                     [=](const QString &) { requestImageCompletions(); });
    QObject::connect(serverCombo, &QComboBox::currentIndexChanged, imagePane,
                     [=](int) { requestImageCompletions(); });
    QObject::connect(containerService, &ContainerService::imageCompletionsReady, imagePane,
                     [=](const QString &completions) {
                         imageCompleter->setModel(new QStringListModel(
                           completions.split(QLatin1Char('\n'), Qt::SkipEmptyParts), imageCompleter));
                     });
    imageForm->addRow(QObject::tr("Image:"), imageEdit);
    sourceStack->addWidget(imagePane);

    // Containerfile.
    auto *containerfilePane = new QWidget(sourceStack);
    auto *containerfileForm = new QFormLayout(containerfilePane);
    auto *dockerfileEdit = new QLineEdit(containerfilePane);
    dockerfileEdit->setText(prefill.dockerfile.isEmpty() ? QStringLiteral("Dockerfile")
                                                          : QString(prefill.dockerfile));
    containerfileForm->addRow(QObject::tr("Dockerfile:"), dockerfileEdit);
    auto *contextDirEdit = new QLineEdit(containerfilePane);
    contextDirEdit->setText(prefill.context_dir.isEmpty() ? QStringLiteral("$PROJECT_DIR$")
                                                           : QString(prefill.context_dir));
    containerfileForm->addRow(QObject::tr("Context:"), contextDirEdit);
    auto *imageTagEdit = new QLineEdit(containerfilePane);
    imageTagEdit->setText(prefill.image_tag);
    imageTagEdit->setPlaceholderText(QObject::tr("leave blank for an automatic tag"));
    containerfileForm->addRow(QObject::tr("Image tag:"), imageTagEdit);
    sourceStack->addWidget(containerfilePane);

    // Compose service.
    auto *composePane = new QWidget(sourceStack);
    auto *composeLayout = new QVBoxLayout(composePane);
    composeLayout->addWidget(new QLabel(QObject::tr("Compose files:"), composePane));
    auto *composeFilesList = new QListWidget(composePane);
    composeFilesList->setMaximumHeight(70);
    for (const QString &file :
        QString(prefill.compose_files).split(QLatin1Char('\n'), Qt::SkipEmptyParts)) {
        composeFilesList->addItem(file);
    }
    composeLayout->addWidget(composeFilesList);
    auto *composeFilesButtons = new QHBoxLayout();
    auto *addComposeFileButton = new QPushButton(QObject::tr("Add..."), composePane);
    auto *removeComposeFileButton = new QPushButton(QObject::tr("Remove"), composePane);
    composeFilesButtons->addWidget(addComposeFileButton);
    composeFilesButtons->addWidget(removeComposeFileButton);
    composeFilesButtons->addStretch(1);
    composeLayout->addLayout(composeFilesButtons);
    QObject::connect(addComposeFileButton, &QPushButton::clicked, composePane, [=, &wizard]() {
        const QString file =
          QFileDialog::getOpenFileName(&wizard, QObject::tr("Add Compose File"));
        if (!file.isEmpty()) {
            composeFilesList->addItem(file);
        }
    });
    QObject::connect(removeComposeFileButton, &QPushButton::clicked, composePane, [=]() {
        qDeleteAll(composeFilesList->selectedItems());
    });

    auto *serviceCombo = new QComboBox(composePane);
    serviceCombo->setEditable(true);
    if (!QString(prefill.service).isEmpty()) {
        serviceCombo->addItem(prefill.service);
    }
    composeLayout->addWidget(new QLabel(QObject::tr("Service:"), composePane));
    composeLayout->addWidget(serviceCombo);
    auto *needsBuildLabel =
      new QLabel(QObject::tr("Build step: not checked yet"), composePane);
    composeLayout->addWidget(needsBuildLabel);
    // `needsBuild` is what actually gets saved — the label is display only,
    // updated by `composeNeedsBuildReady` below.
    auto needsBuild = std::make_shared<bool>(prefill.needs_build);

    const auto requestServices = [=]() {
        QStringList files;
        for (int row = 0; row < composeFilesList->count(); ++row) {
            files << composeFilesList->item(row)->text();
        }
        editor->requestComposeServices(serverCombo->currentData().toString(),
                                       files.join(QLatin1Char('\n')));
    };
    QObject::connect(editor, &RunConfigEditor::composeServicesReady, composePane,
                     [=](const QString &services) {
                         const QString current = serviceCombo->currentText();
                         serviceCombo->clear();
                         serviceCombo->addItems(
                           services.split(QLatin1Char('\n'), Qt::SkipEmptyParts));
                         if (!current.isEmpty()) {
                             const int index = serviceCombo->findText(current);
                             if (index >= 0) {
                                 serviceCombo->setCurrentIndex(index);
                             } else {
                                 serviceCombo->setEditText(current);
                             }
                         }
                     });
    const auto requestNeedsBuild = [=]() {
        QStringList files;
        for (int row = 0; row < composeFilesList->count(); ++row) {
            files << composeFilesList->item(row)->text();
        }
        needsBuildLabel->setText(QObject::tr("Build step: checking..."));
        editor->requestComposeNeedsBuild(serverCombo->currentData().toString(),
                                         files.join(QLatin1Char('\n')),
                                         serviceCombo->currentText());
    };
    QObject::connect(editor, &RunConfigEditor::composeNeedsBuildReady, composePane,
                     [=](bool value) {
                         *needsBuild = value;
                         needsBuildLabel->setText(
                           value ? QObject::tr("Build step: this service builds before it runs")
                                 : QObject::tr("Build step: none (uses a published image)"));
                     });
    QObject::connect(addComposeFileButton, &QPushButton::clicked, composePane, requestServices);
    QObject::connect(removeComposeFileButton, &QPushButton::clicked, composePane, requestServices);
    QObject::connect(serviceCombo, &QComboBox::currentTextChanged, composePane,
                     [=]() { requestNeedsBuild(); });
    sourceStack->addWidget(composePane);

    const auto applySourcePage = [=](int sourceIdx) {
        sourceStack->setCurrentIndex(sourceIdx);
        page2->setTitle(sourceIdx == 0 ? QObject::tr("Image")
                                        : sourceIdx == 1 ? QObject::tr("Dockerfile")
                                                          : QObject::tr("Compose service"));
        if (sourceIdx == 2) {
            requestServices();
        }
    };
    QObject::connect(sourceCombo, &QComboBox::currentIndexChanged, page2, applySourcePage);
    applySourcePage(sourceCombo->currentIndex());

    wizard.addPage(page2);

    // ---- Page 3: mount, env, ports, run options, preview -----------------
    auto *page3 = new QWizardPage(&wizard);
    page3->setTitle(QObject::tr("Mount and options"));
    auto *page3Layout = new QVBoxLayout(page3);

    auto *workdirEdit = new QLineEdit(page3);
    workdirEdit->setText(prefill.workdir.isEmpty() ? QStringLiteral("/workspace")
                                                    : QString(prefill.workdir));
    auto *workdirForm = new QFormLayout();
    workdirForm->addRow(QObject::tr("Mount at:"), workdirEdit);
    page3Layout->addLayout(workdirForm);

    page3Layout->addWidget(new QLabel(QObject::tr("Environment:"), page3));
    auto *envTable = buildRowTable(page3, page3Layout, { QObject::tr("Key"), QObject::tr("Value") });
    for (const FfiKeyValue &pair : prefill.env) {
        appendPlainRow(envTable, { pair.key, pair.value });
    }

    page3Layout->addWidget(new QLabel(QObject::tr("Port bindings:"), page3));
    auto *portsTable = buildRowTable(
      page3, page3Layout, { QObject::tr("Host IP"), QObject::tr("Host port"), QObject::tr("Container port"), QObject::tr("Protocol") });
    for (const FfiPortBinding &binding : prefill.port_bindings) {
        appendPlainRow(portsTable,
                       { binding.host_ip, binding.host_port, binding.container_port,
                         binding.protocol });
    }

    page3Layout->addWidget(new QLabel(QObject::tr("Extra mounts:"), page3));
    auto *mountsTable = buildRowTable(page3, page3Layout,
                                      { QObject::tr("Host path"), QObject::tr("Container path"), QObject::tr("Read-only") });
    for (const FfiBindMount &mount : prefill.extra_mounts) {
        appendPlainRow(mountsTable,
                       { mount.host_path, mount.container_path,
                         mount.read_only ? QStringLiteral("ro") : QString() });
    }

    auto *runOptionsEdit = new QLineEdit(page3);
    runOptionsEdit->setText(prefill.run_options);
    auto *runOptionsForm = new QFormLayout();
    runOptionsForm->addRow(QObject::tr("Run options:"), runOptionsEdit);
    page3Layout->addLayout(runOptionsForm);

    page3Layout->addWidget(new QLabel(QObject::tr("Command preview:"), page3));
    auto *previewEdit = new QPlainTextEdit(page3);
    previewEdit->setReadOnly(true);
    previewEdit->setMaximumHeight(50);
    page3Layout->addWidget(previewEdit);

    // Collects everything the page tree holds into one `FfiContainerTarget`
    // — the single source of truth `refreshPreview`, Finish and (via
    // `target` out-parameter) the caller all read from.
    const auto collect = [=]() {
        FfiContainerTarget result{};
        result.id = prefill.id;
        result.name = nameEdit->text();
        result.connection_id = serverCombo->currentData().toString();
        result.source = kSourceIds.at(sourceCombo->currentIndex());
        result.image = imageEdit->text();
        result.dockerfile = dockerfileEdit->text();
        result.context_dir = contextDirEdit->text();
        result.image_tag = imageTagEdit->text();
        QStringList files;
        for (int row = 0; row < composeFilesList->count(); ++row) {
            files << composeFilesList->item(row)->text();
        }
        result.compose_files = files.join(QLatin1Char('\n'));
        result.service = serviceCombo->currentText();
        result.needs_build = *needsBuild;
        result.workdir = workdirEdit->text();
        result.run_options = runOptionsEdit->text();
        for (int row = 0; row < envTable->rowCount(); ++row) {
            const QString key = cellText(envTable, row, 0);
            if (!key.isEmpty()) {
                FfiKeyValue pair;
                pair.key = key;
                pair.value = cellText(envTable, row, 1);
                result.env.push_back(pair);
            }
        }
        for (int row = 0; row < portsTable->rowCount(); ++row) {
            const QString hostPort = cellText(portsTable, row, 1);
            if (!hostPort.isEmpty()) {
                FfiPortBinding binding;
                binding.host_ip = cellText(portsTable, row, 0);
                binding.host_port = hostPort;
                binding.container_port = cellText(portsTable, row, 2);
                binding.protocol = cellText(portsTable, row, 3);
                result.port_bindings.push_back(binding);
            }
        }
        for (int row = 0; row < mountsTable->rowCount(); ++row) {
            const QString hostPath = cellText(mountsTable, row, 0);
            if (!hostPath.isEmpty()) {
                FfiBindMount mount;
                mount.host_path = hostPath;
                mount.container_path = cellText(mountsTable, row, 1);
                mount.read_only = !cellText(mountsTable, row, 2).isEmpty();
                result.extra_mounts.push_back(mount);
            }
        }
        return result;
    };

    const auto refreshPreview = [=]() {
        previewEdit->setPlainText(editor->targetCommandPreview(collect()));
    };
    QObject::connect(page3, &QWizardPage::initializePage, page3, refreshPreview);
    for (QLineEdit *field : { workdirEdit, runOptionsEdit, imageEdit, dockerfileEdit,
                              contextDirEdit, imageTagEdit }) {
        QObject::connect(field, &QLineEdit::textChanged, page3, refreshPreview);
    }
    QObject::connect(envTable, &QTableWidget::cellChanged, page3, refreshPreview);
    QObject::connect(portsTable, &QTableWidget::cellChanged, page3, refreshPreview);
    QObject::connect(mountsTable, &QTableWidget::cellChanged, page3, refreshPreview);

    wizard.addPage(page3);

    if (wizard.exec() != QDialog::Accepted) {
        return false;
    }
    target = collect();
    return true;
}

} // namespace ui_shell
