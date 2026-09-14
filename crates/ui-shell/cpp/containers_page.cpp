#include "containers_page.h"

#include "container_target_wizard.h"
#include "containers_registries_page.h"
#include "e2e_mark.h"

#include <QAction>
#include <QCheckBox>
#include <QComboBox>
#include <QFormLayout>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMenu>
#include <QObject>
#include <QPoint>
#include <QPushButton>
#include <QSplitter>
#include <QStackedWidget>
#include <QTabWidget>
#include <QUuid>
#include <QVBoxLayout>
#include <QVector>
#include <QWidget>

namespace ui_shell {

namespace {

// The kind combo's index doubles as `FfiContainerConnection::kind`'s
// string and the field-stack page index — one fixed order, named once,
// rather than several switch statements that could drift apart.
enum KindIndex {
    kAuto = 0,
    kUnixSocket,
    kTcp,
    kNamedPipe,
    kContext,
    kSsh,
    kWsl,
    kPodmanMachine,
    kMinikube,
};

const QStringList &kindIds()
{
    static const QStringList ids = {
        QStringLiteral("auto"),      QStringLiteral("unix_socket"),
        QStringLiteral("tcp"),       QStringLiteral("named_pipe"),
        QStringLiteral("context"),   QStringLiteral("ssh"),
        QStringLiteral("wsl"),       QStringLiteral("podman_machine"),
        QStringLiteral("minikube"),
    };
    return ids;
}

QString kindLabel(int index)
{
    switch (index) {
    case kUnixSocket:
        return QObject::tr("Unix socket");
    case kTcp:
        return QObject::tr("TCP");
    case kNamedPipe:
        return QObject::tr("Named pipe");
    case kContext:
        return QObject::tr("Docker context / Podman connection");
    case kSsh:
        return QObject::tr("SSH");
    case kWsl:
        return QObject::tr("WSL distro");
    case kPodmanMachine:
        return QObject::tr("Podman machine");
    case kMinikube:
        return QObject::tr("Minikube");
    default:
        return QObject::tr("Auto (CLI default)");
    }
}

QString textOf(QLineEdit *edit)
{
    return edit == nullptr ? QString() : edit->text();
}

void setTextOf(QLineEdit *edit, const QString &text)
{
    if (edit != nullptr) {
        edit->setText(text);
    }
}

// One row's editable fields, plus every kind-specific widget — a distinct
// `QLineEdit` per (kind, role) rather than one shared "path"/"url" widget
// reused across kinds: unix-socket and named-pipe both feed
// `FfiContainerConnection::path`, but they are shown on different stack
// pages and must not share one `QLineEdit*` (the stack page that built it
// second would silently orphan the first kind's widget, discarding
// whatever the user had typed into it the moment they switched away and
// back).
struct ConnectionFields
{
    QLineEdit *name = nullptr;
    QComboBox *engine = nullptr;
    QComboBox *kind = nullptr;
    QStackedWidget *kindStack = nullptr;

    QLineEdit *socketPath = nullptr;
    QLineEdit *tcpUrl = nullptr;
    QLineEdit *tcpCertDir = nullptr;
    QLineEdit *pipePath = nullptr;
    QLineEdit *contextName = nullptr;
    QLineEdit *sshUrl = nullptr;
    QLineEdit *sshIdentity = nullptr;
    QLineEdit *wslDistro = nullptr;
    QLineEdit *machineName = nullptr;

    QLineEdit *executable = nullptr;
    QLineEdit *composeExecutable = nullptr;
    QPushButton *testButton = nullptr;
    QLabel *testResult = nullptr;
};

ConnectionFields buildConnectionFields(QWidget *parent, QFormLayout *form)
{
    ConnectionFields fields;
    fields.name = new QLineEdit(parent);
    form->addRow(QObject::tr("Name:"), fields.name);

    fields.engine = new QComboBox(parent);
    fields.engine->addItem(QStringLiteral("Docker"), QStringLiteral("docker"));
    fields.engine->addItem(QStringLiteral("Podman"), QStringLiteral("podman"));
    form->addRow(QObject::tr("Engine:"), fields.engine);

    fields.kind = new QComboBox(parent);
    for (int i = 0; i < kindIds().size(); ++i) {
        fields.kind->addItem(kindLabel(i), kindIds().at(i));
    }
    form->addRow(QObject::tr("Connect via:"), fields.kind);

    fields.kindStack = new QStackedWidget(parent);

    auto addInfoPage = [&](const QString &text) {
        fields.kindStack->addWidget(new QLabel(text, parent));
    };
    auto addOneFieldPage = [&](const QString &label, QLineEdit *&target) {
        auto *page = new QWidget(parent);
        auto *pageForm = new QFormLayout(page);
        target = new QLineEdit(page);
        pageForm->addRow(label, target);
        fields.kindStack->addWidget(page);
    };
    auto addTwoFieldPage = [&](const QString &firstLabel, QLineEdit *&first, const QString &secondLabel,
                               QLineEdit *&second) {
        auto *page = new QWidget(parent);
        auto *pageForm = new QFormLayout(page);
        first = new QLineEdit(page);
        pageForm->addRow(firstLabel, first);
        second = new QLineEdit(page);
        pageForm->addRow(secondLabel, second);
        fields.kindStack->addWidget(page);
    };

    addInfoPage(QObject::tr("Uses the CLI's own default connection."));
    addOneFieldPage(QObject::tr("Socket path:"), fields.socketPath);
    addTwoFieldPage(QObject::tr("Daemon URL:"), fields.tcpUrl, QObject::tr("Certificate directory:"),
                    fields.tcpCertDir);
    addOneFieldPage(QObject::tr("Pipe path:"), fields.pipePath);
    addOneFieldPage(QObject::tr("Context/connection name:"), fields.contextName);
    addTwoFieldPage(QObject::tr("SSH URL (user@host):"), fields.sshUrl, QObject::tr("Identity file (optional):"),
                    fields.sshIdentity);
    addOneFieldPage(QObject::tr("WSL distro:"), fields.wslDistro);
    addOneFieldPage(QObject::tr("Machine name:"), fields.machineName);
    addInfoPage(QObject::tr("Reads the environment from `minikube docker-env`."));

    form->addRow(fields.kindStack);

    fields.executable = new QLineEdit(parent);
    fields.executable->setPlaceholderText(QObject::tr("Default: docker / podman"));
    form->addRow(QObject::tr("Executable override:"), fields.executable);

    fields.composeExecutable = new QLineEdit(parent);
    fields.composeExecutable->setPlaceholderText(QObject::tr("Default: same as above"));
    form->addRow(QObject::tr("Compose executable override:"), fields.composeExecutable);

    auto *testRow = new QHBoxLayout();
    fields.testButton = new QPushButton(QObject::tr("Test connection"), parent);
    fields.testResult = new QLabel(parent);
    fields.testResult->setWordWrap(true);
    testRow->addWidget(fields.testButton);
    testRow->addWidget(fields.testResult, 1);
    form->addRow(testRow);

    QObject::connect(fields.kind, &QComboBox::currentIndexChanged, fields.kindStack,
                      &QStackedWidget::setCurrentIndex);

    return fields;
}

// Builds a full row from every field, the current kind deciding which
// kind-specific widgets feed the row's shared `path`/`url`/`cert_dir`/
// `identity`/`distro`/`resource_name` — the same fields
// `ContainerConnectionSetting` carries regardless of kind (ADR-0017/
// ADR-0039: persistence stays dumb).
FfiContainerConnection rowFromFields(const ConnectionFields &fields, const QString &id)
{
    FfiContainerConnection row;
    row.id = id;
    row.name = fields.name->text();
    row.engine = fields.engine->currentData().toString();
    row.kind = fields.kind->currentData().toString();
    row.executable = fields.executable->text();
    row.compose_executable = fields.composeExecutable->text();

    switch (fields.kind->currentIndex()) {
    case kUnixSocket:
        row.path = textOf(fields.socketPath);
        break;
    case kTcp:
        row.url = textOf(fields.tcpUrl);
        row.cert_dir = textOf(fields.tcpCertDir);
        break;
    case kNamedPipe:
        row.path = textOf(fields.pipePath);
        break;
    case kContext:
        row.resource_name = textOf(fields.contextName);
        break;
    case kSsh:
        row.url = textOf(fields.sshUrl);
        row.identity = textOf(fields.sshIdentity);
        break;
    case kWsl:
        row.distro = textOf(fields.wslDistro);
        break;
    case kPodmanMachine:
        row.resource_name = textOf(fields.machineName);
        break;
    default:
        break;
    }
    return row;
}

// `saveContainerConnections` takes cxx's own `rust::Vec<T>`, not a
// `QVector` — this crate's draft is a `QVector` for its `remove`/index-
// assignment, so this is the one conversion needed, and only at commit.
::rust::Vec<FfiContainerConnection> toRustVec(const QVector<FfiContainerConnection> &rows)
{
    ::rust::Vec<FfiContainerConnection> converted;
    for (const FfiContainerConnection &row : rows) {
        converted.push_back(row);
    }
    return converted;
}

// Run targets (C8): same conversion, `FfiContainerTarget` row.
::rust::Vec<FfiContainerTarget> toRustVec(const QVector<FfiContainerTarget> &rows)
{
    ::rust::Vec<FfiContainerTarget> converted;
    for (const FfiContainerTarget &row : rows) {
        converted.push_back(row);
    }
    return converted;
}

void fieldsFromRow(const ConnectionFields &fields, const FfiContainerConnection &row)
{
    fields.name->setText(row.name);
    const int engineIndex = fields.engine->findData(row.engine);
    fields.engine->setCurrentIndex(engineIndex >= 0 ? engineIndex : 0);
    const int kindIndex = kindIds().indexOf(row.kind);
    const int resolvedKind = kindIndex >= 0 ? kindIndex : kAuto;
    fields.kind->setCurrentIndex(resolvedKind);
    fields.kindStack->setCurrentIndex(resolvedKind);
    fields.executable->setText(row.executable);
    fields.composeExecutable->setText(row.compose_executable);

    setTextOf(fields.socketPath, resolvedKind == kUnixSocket ? row.path : QString());
    setTextOf(fields.pipePath, resolvedKind == kNamedPipe ? row.path : QString());
    setTextOf(fields.tcpUrl, resolvedKind == kTcp ? row.url : QString());
    setTextOf(fields.tcpCertDir, resolvedKind == kTcp ? row.cert_dir : QString());
    setTextOf(fields.contextName, resolvedKind == kContext ? row.resource_name : QString());
    setTextOf(fields.sshUrl, resolvedKind == kSsh ? row.url : QString());
    setTextOf(fields.sshIdentity, resolvedKind == kSsh ? row.identity : QString());
    setTextOf(fields.wslDistro, resolvedKind == kWsl ? row.distro : QString());
    setTextOf(fields.machineName, resolvedKind == kPodmanMachine ? row.resource_name : QString());

    fields.testResult->clear();
}

} // namespace

// Run targets (C8): a plain list + Add/Edit/Remove, all three going through
// the New Target wizard — Add opens it empty, Edit reopens it prefilled with
// the selected row. Unlike Connections/Registries there is no per-row form
// on this page: the wizard *is* the editor, the same "the wizard is the
// only way to shape one of these" rule the wizard's own doc comment states.
QWidget *buildRunTargetsTab(QWidget *parent, AppSettings *appSettings,
                            RunConfigEditor *runConfigEditor, ContainerService *containerService,
                            std::function<void()> &outCommit)
{
    auto *tab = new QWidget(parent);
    auto *layout = new QVBoxLayout(tab);

    auto targets = std::make_shared<QVector<FfiContainerTarget>>();
    for (const FfiContainerTarget &row : appSettings->containerTargets()) {
        targets->append(row);
    }

    auto *list = new QListWidget(tab);
    for (const FfiContainerTarget &row : *targets) {
        list->addItem(row.name.isEmpty() ? QObject::tr("(unnamed)") : row.name);
    }
    layout->addWidget(list, 1);

    auto *buttons = new QHBoxLayout();
    auto *addButton = new QPushButton(QObject::tr("Add..."), tab);
    auto *editButton = new QPushButton(QObject::tr("Edit..."), tab);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), tab);
    buttons->addWidget(addButton);
    buttons->addWidget(editButton);
    buttons->addWidget(removeButton);
    buttons->addStretch(1);
    layout->addLayout(buttons);

    const auto repaint = [targets, list](int keepRow) {
        list->clear();
        for (const FfiContainerTarget &row : *targets) {
            list->addItem(row.name.isEmpty() ? QObject::tr("(unnamed)") : row.name);
        }
        if (keepRow >= 0 && keepRow < list->count()) {
            list->setCurrentRow(keepRow);
        }
    };

    QObject::connect(addButton, &QPushButton::clicked, tab, [=]() {
        FfiContainerTarget prefill{};
        FfiContainerTarget created{};
        if (showContainerTargetWizard(tab, containerService, runConfigEditor, prefill, created)) {
            if (created.id.isEmpty()) {
                created.id = QUuid::createUuid().toString(QUuid::WithoutBraces);
            }
            targets->append(created);
            repaint(targets->size() - 1);
        }
    });
    QObject::connect(editButton, &QPushButton::clicked, tab, [=]() {
        const int row = list->currentRow();
        if (row < 0 || row >= targets->size()) {
            return;
        }
        FfiContainerTarget edited{};
        if (showContainerTargetWizard(tab, containerService, runConfigEditor, (*targets)[row],
                                      edited)) {
            (*targets)[row] = edited;
            repaint(row);
        }
    });
    QObject::connect(removeButton, &QPushButton::clicked, tab, [=]() {
        const int row = list->currentRow();
        if (row < 0 || row >= targets->size()) {
            return;
        }
        targets->remove(row);
        repaint(qMin(row, targets->size() - 1));
    });

    outCommit = [appSettings, targets]() { appSettings->saveContainerTargets(toRustVec(*targets)); };
    return tab;
}

ContainersPage buildContainersPage(QWidget *parent, AppSettings *appSettings,
                                   RunConfigEditor *runConfigEditor,
                                   ContainerService *containerService)
{
    auto *page = new QWidget(parent);
    auto *pageLayout = new QVBoxLayout(page);
    pageLayout->setContentsMargins(0, 0, 0, 0);

    // C7: Connections and Registries share this one Settings page as two
    // tabs, the same nesting `settings_dialog.cpp` already uses for other
    // multi-section categories — the dialog's own category list stays one
    // "Containers" entry rather than growing a second top-level row.
    auto *tabs = new QTabWidget(page);
    // Named so `settings_dialog.cpp` can select the Registries tab directly
    // when opening from "Registry..."/registry-node "Edit..." (C7 review
    // follow-up) without this page knowing anything about who is asking.
    tabs->setObjectName(QStringLiteral("containersTabs"));
    auto *connectionsTab = new QWidget(tabs);
    auto *connectionsLayout = new QVBoxLayout(connectionsTab);
    connectionsLayout->setContentsMargins(0, 0, 0, 0);

    auto connections = std::make_shared<QVector<FfiContainerConnection>>();
    for (const FfiContainerConnection &row : appSettings->containerConnections()) {
        connections->append(row);
    }

    auto *splitter = new QSplitter(connectionsTab);

    auto *listPane = new QWidget(splitter);
    auto *listLayout = new QVBoxLayout(listPane);
    listLayout->setContentsMargins(0, 0, 0, 0);
    auto *list = new QListWidget(listPane);
    for (const FfiContainerConnection &row : *connections) {
        list->addItem(row.name.isEmpty() ? QObject::tr("(unnamed)") : row.name);
    }
    listLayout->addWidget(list, 1);

    auto *listButtons = new QHBoxLayout();
    auto *addButton = new QPushButton(QObject::tr("Add"), listPane);
    auto *addFromContextsButton = new QPushButton(QObject::tr("Add from contexts..."), listPane);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), listPane);
    listButtons->addWidget(addButton);
    listButtons->addWidget(addFromContextsButton);
    listButtons->addWidget(removeButton);
    listLayout->addLayout(listButtons);
    splitter->addWidget(listPane);

    auto *formPane = new QWidget(splitter);
    auto *form = new QFormLayout(formPane);
    const ConnectionFields fields = buildConnectionFields(formPane, form);
    splitter->addWidget(formPane);
    splitter->setStretchFactor(0, 1);
    splitter->setStretchFactor(1, 2);
    connectionsLayout->addWidget(splitter, 1);

    auto *filtersBox = new QGroupBox(QObject::tr("Containers dock"), connectionsTab);
    auto *filtersLayout = new QVBoxLayout(filtersBox);
    const FfiContainerSettings currentSettings = appSettings->containerSettings();
    auto *showStopped = new QCheckBox(QObject::tr("Show stopped containers"), filtersBox);
    showStopped->setChecked(currentSettings.show_stopped_containers);
    auto *showUntagged = new QCheckBox(QObject::tr("Show untagged images"), filtersBox);
    showUntagged->setChecked(currentSettings.show_untagged_images);
    auto *selinuxRelabel = new QCheckBox(
      QObject::tr("Add the SELinux \":z\" relabel suffix to bind mounts this IDE creates"), filtersBox);
    selinuxRelabel->setChecked(currentSettings.selinux_relabel);
    filtersLayout->addWidget(showStopped);
    filtersLayout->addWidget(showUntagged);
    filtersLayout->addWidget(selinuxRelabel);
    connectionsLayout->addWidget(filtersBox);

    tabs->addTab(connectionsTab, QObject::tr("Connections"));
    const RegistriesPage registriesPage = buildRegistriesPage(tabs, appSettings);
    tabs->addTab(registriesPage.widget, QObject::tr("Registries"));
    std::function<void()> runTargetsCommit;
    QWidget *runTargetsTab =
      buildRunTargetsTab(tabs, appSettings, runConfigEditor, containerService, runTargetsCommit);
    tabs->addTab(runTargetsTab, QObject::tr("Run targets"));
    pageLayout->addWidget(tabs, 1);

    // Named for `settings_dialog.cpp`'s own `containers_settings_page_shown`
    // marker, which reports this button's rect too — only reliable once the
    // dialog is actually on screen (its own doc comment explains why),
    // which this page's construction is ahead of.
    fields.testButton->setObjectName(QStringLiteral("containersTestConnectionButton"));
    const auto markPageShown = [tabs](int index) {
        e2eMark(QStringLiteral("{\"ev\":\"containers_settings_page_shown\",\"page\":%1}")
                  .arg(e2eJson(tabs->tabText(index))));
    };
    QObject::connect(tabs, &QTabWidget::currentChanged, tabs, markPageShown);

    formPane->setEnabled(false);

    // Every field writes its current row back into the draft as it is
    // edited, guarded by `loading` so `fieldsFromRow`'s own writes (when
    // the selection changes) do not re-save the row that is being loaded
    // into the very fields it just set. `fields` is captured by value —
    // a handful of widget pointers, all parented under `formPane` and
    // therefore alive as long as the page is — never by reference to the
    // local variable, which would dangle once this function returns.
    auto loading = std::make_shared<bool>(false);
    auto writeCurrentRow = [connections, fields, list, loading]() {
        if (*loading) {
            return;
        }
        const int row = list->currentRow();
        if (row < 0 || row >= connections->size()) {
            return;
        }
        const QString id = (*connections)[row].id;
        (*connections)[row] = rowFromFields(fields, id);
        list->item(row)->setText(fields.name->text().isEmpty() ? QObject::tr("(unnamed)")
                                                                 : fields.name->text());
    };
    for (QLineEdit *edit :
         {fields.name, fields.socketPath, fields.tcpUrl, fields.tcpCertDir, fields.pipePath,
          fields.contextName, fields.sshUrl, fields.sshIdentity, fields.wslDistro, fields.machineName,
          fields.executable, fields.composeExecutable}) {
        QObject::connect(edit, &QLineEdit::textChanged, formPane, writeCurrentRow);
    }
    QObject::connect(fields.engine, &QComboBox::currentIndexChanged, formPane, writeCurrentRow);
    QObject::connect(fields.kind, &QComboBox::currentIndexChanged, formPane, writeCurrentRow);

    auto selectRow = [connections, fields, loading, formPane](int row) {
        *loading = true;
        if (row >= 0 && row < connections->size()) {
            fieldsFromRow(fields, (*connections)[row]);
            formPane->setEnabled(true);
        } else {
            formPane->setEnabled(false);
        }
        *loading = false;
    };
    QObject::connect(list, &QListWidget::currentRowChanged, formPane, selectRow);

    QObject::connect(addButton, &QPushButton::clicked, list, [connections, list]() {
        FfiContainerConnection row;
        row.id = QUuid::createUuid().toString(QUuid::WithoutBraces);
        row.name = QObject::tr("New connection");
        row.engine = QStringLiteral("docker");
        row.kind = QStringLiteral("auto");
        connections->append(row);
        list->addItem(row.name);
        list->setCurrentRow(list->count() - 1);
    });

    QObject::connect(removeButton, &QPushButton::clicked, list, [connections, list]() {
        const int row = list->currentRow();
        if (row < 0 || row >= connections->size()) {
            return;
        }
        connections->remove(row);
        delete list->takeItem(row);
    });

    QObject::connect(
      addFromContextsButton, &QPushButton::clicked, addFromContextsButton,
      [appSettings, connections, list, addFromContextsButton]() {
          QMenu menu(addFromContextsButton);
          const auto discovered = appSettings->discoverContainerConnections();
          if (discovered.empty()) {
              QAction *none = menu.addAction(QObject::tr("No connections found"));
              none->setEnabled(false);
          }
          for (const FfiDiscoveredConnection &candidate : discovered) {
              QAction *action = menu.addAction(candidate.label);
              QObject::connect(action, &QAction::triggered, list, [connections, list, candidate]() {
                  FfiContainerConnection row;
                  row.id = QUuid::createUuid().toString(QUuid::WithoutBraces);
                  row.name = candidate.label;
                  row.engine = candidate.engine;
                  row.kind = candidate.kind;
                  row.path = candidate.path;
                  row.url = candidate.url;
                  row.distro = candidate.distro;
                  row.resource_name = candidate.resource_name;
                  connections->append(row);
                  list->addItem(row.name);
                  list->setCurrentRow(list->count() - 1);
              });
          }
          menu.exec(addFromContextsButton->mapToGlobal(QPoint(0, addFromContextsButton->height())));
      });

    QObject::connect(fields.testButton, &QPushButton::clicked, formPane, [fields, appSettings]() {
        fields.testResult->setStyleSheet(QString());
        fields.testResult->setText(QObject::tr("Testing..."));
        appSettings->testContainerConnection(rowFromFields(fields, QString()));
    });
    QObject::connect(appSettings, &AppSettings::containerConnectionTested, formPane,
                      [fields](bool ok, const QString &message) {
                          fields.testResult->setText(message);
                          fields.testResult->setStyleSheet(ok ? QStringLiteral("color: #4caf50;")
                                                              : QStringLiteral("color: #e53935;"));
                          e2eMark(QStringLiteral(
                                    "{\"ev\":\"containers_test_connection_result\",\"ok\":%1}")
                                    .arg(ok ? "true" : "false"));
                      });

    if (!connections->isEmpty()) {
        list->setCurrentRow(0);
    }

    return ContainersPage{
      page,
      [appSettings, connections, showStopped, showUntagged, selinuxRelabel,
       registriesCommit = registriesPage.commit, runTargetsCommit]() {
          appSettings->saveContainerConnections(toRustVec(*connections));
          FfiContainerSettings settings;
          settings.show_stopped_containers = showStopped->isChecked();
          settings.show_untagged_images = showUntagged->isChecked();
          settings.selinux_relabel = selinuxRelabel->isChecked();
          appSettings->saveContainerSettings(settings);
          registriesCommit();
          runTargetsCommit();
      },
    };
}

} // namespace ui_shell
