#include "containers_registries_page.h"

#include <QComboBox>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QHash>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QSplitter>
#include <QUuid>
#include <QVBoxLayout>
#include <QVector>
#include <QWidget>

namespace ui_shell {

namespace {

enum KindIndex {
    kHub = 0,
    kGitLab,
    kDockerV2,
    kGeneric,
};

const QStringList &kindIds()
{
    static const QStringList ids = {
        QStringLiteral("hub"),
        QStringLiteral("gitlab"),
        QStringLiteral("v2"),
        QStringLiteral("generic"),
    };
    return ids;
}

QString kindLabel(int index)
{
    switch (index) {
    case kGitLab:
        return QObject::tr("GitLab");
    case kDockerV2:
        return QObject::tr("Docker V2");
    case kGeneric:
        return QObject::tr("Generic (push-only)");
    default:
        return QObject::tr("Docker Hub");
    }
}

struct RegistryFields
{
    QLineEdit *name = nullptr;
    QComboBox *kind = nullptr;
    QLineEdit *address = nullptr;
    QPushButton *ghcrPreset = nullptr;
    QPushButton *quayPreset = nullptr;
    QLineEdit *username = nullptr;
    QLineEdit *password = nullptr;
    QLabel *keychainHint = nullptr;
    QLabel *gitlabProjectLabel = nullptr;
    QLineEdit *gitlabProject = nullptr;
    QPushButton *testButton = nullptr;
    QLabel *testResult = nullptr;
};

RegistryFields buildRegistryFields(QWidget *parent, QFormLayout *form)
{
    RegistryFields fields;
    fields.name = new QLineEdit(parent);
    form->addRow(QObject::tr("Name:"), fields.name);

    fields.kind = new QComboBox(parent);
    for (int i = 0; i < kindIds().size(); ++i) {
        fields.kind->addItem(kindLabel(i), kindIds().at(i));
    }
    form->addRow(QObject::tr("Kind:"), fields.kind);

    auto *addressRow = new QHBoxLayout();
    fields.address = new QLineEdit(parent);
    fields.address->setPlaceholderText(QObject::tr("host[:port], e.g. ghcr.io"));
    fields.ghcrPreset = new QPushButton(QObject::tr("GHCR"), parent);
    fields.quayPreset = new QPushButton(QObject::tr("Quay"), parent);
    addressRow->addWidget(fields.address, 1);
    addressRow->addWidget(fields.ghcrPreset);
    addressRow->addWidget(fields.quayPreset);
    form->addRow(QObject::tr("Address:"), addressRow);

    fields.username = new QLineEdit(parent);
    form->addRow(QObject::tr("Username:"), fields.username);

    fields.password = new QLineEdit(parent);
    fields.password->setEchoMode(QLineEdit::Password);
    fields.password->setPlaceholderText(QObject::tr("Leave blank to keep the stored password/token"));
    form->addRow(QObject::tr("Password/Token:"), fields.password);

    fields.keychainHint = new QLabel(
      QObject::tr("Stored in the OS keychain, never in settings.toml. If no OS keychain is "
                  "available here, leave this blank and run `docker login <address>` instead —"
                  " push/pull still work through the CLI's own stored credentials."),
      parent);
    fields.keychainHint->setWordWrap(true);
    fields.keychainHint->setStyleSheet(QStringLiteral("color: palette(mid);"));
    form->addRow(QString(), fields.keychainHint);

    fields.gitlabProjectLabel = new QLabel(QObject::tr("GitLab project:"), parent);
    fields.gitlabProject = new QLineEdit(parent);
    fields.gitlabProject->setPlaceholderText(QObject::tr("group/project or a numeric project id"));
    form->addRow(fields.gitlabProjectLabel, fields.gitlabProject);

    auto *testRow = new QHBoxLayout();
    fields.testButton = new QPushButton(QObject::tr("Test connection"), parent);
    fields.testResult = new QLabel(parent);
    fields.testResult->setWordWrap(true);
    testRow->addWidget(fields.testButton);
    testRow->addWidget(fields.testResult, 1);
    form->addRow(testRow);

    auto updateKindVisibility = [fields]() {
        const bool isGitLab = fields.kind->currentData().toString() == QStringLiteral("gitlab");
        fields.gitlabProjectLabel->setVisible(isGitLab);
        fields.gitlabProject->setVisible(isGitLab);
    };
    QObject::connect(fields.kind, &QComboBox::currentIndexChanged, parent, updateKindVisibility);
    updateKindVisibility();

    QObject::connect(fields.ghcrPreset, &QPushButton::clicked, parent, [fields]() {
        fields.kind->setCurrentIndex(kDockerV2);
        fields.address->setText(QStringLiteral("ghcr.io"));
    });
    QObject::connect(fields.quayPreset, &QPushButton::clicked, parent, [fields]() {
        fields.kind->setCurrentIndex(kDockerV2);
        fields.address->setText(QStringLiteral("quay.io"));
    });

    return fields;
}

FfiRegistrySetting rowFromFields(const RegistryFields &fields, const QString &id, bool tokenAuth)
{
    FfiRegistrySetting row;
    row.id = id;
    row.name = fields.name->text();
    row.kind = fields.kind->currentData().toString();
    row.address = fields.address->text();
    row.username = fields.username->text();
    row.gitlab_project = fields.gitlabProject->text();
    row.token_auth = tokenAuth;
    return row;
}

void fieldsFromRow(const RegistryFields &fields, const FfiRegistrySetting &row, AppSettings *appSettings)
{
    fields.name->setText(row.name);
    const int kindIndex = kindIds().indexOf(row.kind);
    fields.kind->setCurrentIndex(kindIndex >= 0 ? kindIndex : kHub);
    fields.address->setText(row.address);
    fields.username->setText(row.username);
    fields.gitlabProject->setText(row.gitlab_project);
    fields.password->clear();
    fields.password->setPlaceholderText(appSettings->hasRegistrySecret(row.id)
                                          ? QObject::tr("A password/token is stored — leave blank to keep it")
                                          : QObject::tr("Leave blank to keep the stored password/token"));
    fields.testResult->clear();
}

::rust::Vec<FfiRegistrySetting> toRustVec(const QVector<FfiRegistrySetting> &rows)
{
    ::rust::Vec<FfiRegistrySetting> converted;
    for (const FfiRegistrySetting &row : rows) {
        converted.push_back(row);
    }
    return converted;
}

} // namespace

RegistriesPage buildRegistriesPage(QWidget *parent, AppSettings *appSettings)
{
    auto *page = new QWidget(parent);
    auto *pageLayout = new QVBoxLayout(page);
    pageLayout->setContentsMargins(0, 0, 0, 0);

    auto registries = std::make_shared<QVector<FfiRegistrySetting>>();
    // Passwords typed into the form this session, by registry id — kept
    // apart from `registries` (which never carries a secret field, same as
    // `FfiRegistrySetting` itself) so `commit()` knows exactly which rows
    // actually need a `storeRegistrySecret` call rather than re-storing
    // every row's blank password over whatever was already there.
    auto pendingSecrets = std::make_shared<QHash<QString, QString>>();
    for (const FfiRegistrySetting &row : appSettings->registries()) {
        registries->append(row);
    }

    auto *splitter = new QSplitter(page);

    auto *listPane = new QWidget(splitter);
    auto *listLayout = new QVBoxLayout(listPane);
    listLayout->setContentsMargins(0, 0, 0, 0);
    auto *list = new QListWidget(listPane);
    for (const FfiRegistrySetting &row : *registries) {
        list->addItem(row.name.isEmpty() ? QObject::tr("(unnamed)") : row.name);
    }
    listLayout->addWidget(list, 1);

    auto *listButtons = new QHBoxLayout();
    auto *addButton = new QPushButton(QObject::tr("Add"), listPane);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), listPane);
    listButtons->addWidget(addButton);
    listButtons->addWidget(removeButton);
    listLayout->addLayout(listButtons);
    splitter->addWidget(listPane);

    auto *formPane = new QWidget(splitter);
    auto *form = new QFormLayout(formPane);
    const RegistryFields fields = buildRegistryFields(formPane, form);
    splitter->addWidget(formPane);
    splitter->setStretchFactor(0, 1);
    splitter->setStretchFactor(1, 2);
    pageLayout->addWidget(splitter, 1);
    formPane->setEnabled(false);

    auto loading = std::make_shared<bool>(false);
    auto writeCurrentRow = [registries, fields, list, pendingSecrets, loading]() {
        if (*loading) {
            return;
        }
        const int row = list->currentRow();
        if (row < 0 || row >= registries->size()) {
            return;
        }
        const QString id = (*registries)[row].id;
        const bool tokenAuth = (*registries)[row].token_auth;
        (*registries)[row] = rowFromFields(fields, id, tokenAuth);
        list->item(row)->setText(fields.name->text().isEmpty() ? QObject::tr("(unnamed)")
                                                                 : fields.name->text());
        if (!fields.password->text().isEmpty()) {
            (*pendingSecrets)[id] = fields.password->text();
        }
    };
    for (QLineEdit *edit : {fields.name, fields.address, fields.username, fields.gitlabProject}) {
        QObject::connect(edit, &QLineEdit::textChanged, formPane, writeCurrentRow);
    }
    QObject::connect(fields.password, &QLineEdit::textChanged, formPane, writeCurrentRow);
    QObject::connect(fields.kind, &QComboBox::currentIndexChanged, formPane, writeCurrentRow);

    auto selectRow = [registries, fields, loading, formPane, appSettings](int row) {
        *loading = true;
        if (row >= 0 && row < registries->size()) {
            fieldsFromRow(fields, (*registries)[row], appSettings);
            formPane->setEnabled(true);
        } else {
            formPane->setEnabled(false);
        }
        *loading = false;
    };
    QObject::connect(list, &QListWidget::currentRowChanged, formPane, selectRow);

    QObject::connect(addButton, &QPushButton::clicked, list, [registries, list]() {
        FfiRegistrySetting row;
        row.id = QUuid::createUuid().toString(QUuid::WithoutBraces);
        row.name = QObject::tr("New registry");
        row.kind = QStringLiteral("hub");
        registries->append(row);
        list->addItem(row.name);
        list->setCurrentRow(list->count() - 1);
    });

    QObject::connect(removeButton, &QPushButton::clicked, list, [registries, list]() {
        const int row = list->currentRow();
        if (row < 0 || row >= registries->size()) {
            return;
        }
        registries->remove(row);
        delete list->takeItem(row);
    });

    QObject::connect(fields.testButton, &QPushButton::clicked, formPane, [fields, appSettings]() {
        fields.testResult->setStyleSheet(QString());
        fields.testResult->setText(QObject::tr("Testing..."));
        appSettings->testRegistryConnection(rowFromFields(fields, QString(), false), fields.password->text());
    });
    QObject::connect(appSettings, &AppSettings::registryTested, formPane,
                      [fields](bool ok, const QString &message) {
                          fields.testResult->setText(message);
                          fields.testResult->setStyleSheet(ok ? QStringLiteral("color: #4caf50;")
                                                              : QStringLiteral("color: #e53935;"));
                      });

    if (!registries->isEmpty()) {
        list->setCurrentRow(0);
    }

    return RegistriesPage{
      page,
      [appSettings, registries, pendingSecrets]() {
          for (auto it = pendingSecrets->constBegin(); it != pendingSecrets->constEnd(); ++it) {
              appSettings->storeRegistrySecret(it.key(), it.value());
          }
          appSettings->saveRegistries(toRustVec(*registries));
      },
    };
}

} // namespace ui_shell
