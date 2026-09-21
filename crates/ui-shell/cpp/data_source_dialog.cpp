#include "data_source_dialog.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QPushButton>
#include <QSpinBox>
#include <QStringList>
#include <QTabWidget>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

void selectData(QComboBox *combo, const QString &data)
{
    const int index = combo->findData(data);
    combo->setCurrentIndex(index >= 0 ? index : 0);
}

QSpinBox *portSpin(QWidget *parent, const QString &text)
{
    auto *spin = new QSpinBox(parent);
    spin->setRange(0, 65535);
    spin->setSpecialValueText(QObject::tr("(default)"));
    bool ok = false;
    const int value = text.toInt(&ok);
    spin->setValue(ok ? value : 0);
    return spin;
}

QLabel *colorSwatch(QWidget *parent, const QString &hex)
{
    auto *swatch = new QLabel(parent);
    swatch->setFixedSize(20, 20);
    swatch->setAutoFillBackground(true);
    swatch->setStyleSheet(QStringLiteral("background-color: %1; border: 1px solid palette(mid);")
                             .arg(hex.isEmpty() ? QStringLiteral("transparent") : hex));
    return swatch;
}

} // namespace

void showDataSourceDialog(QWidget *parent, AppSettings *appSettings, DataSourceEditor *editor,
                           const QString &id, const QString &scope)
{
    editor->beginEdit(id, scope);
    const FfiDataSourceFields fields = editor->fields();

    QDialog dialog(parent);
    dialog.setWindowTitle(id.isEmpty() ? QObject::tr("Add Data Source")
                                        : QObject::tr("Edit Data Source"));
    dialog.resize(480, 420);

    auto *layout = new QVBoxLayout(&dialog);
    auto *problemsLabel = new QLabel(&dialog);
    problemsLabel->setWordWrap(true);
    problemsLabel->setStyleSheet(QStringLiteral("color: #c0392b;"));
    problemsLabel->setVisible(false);
    layout->addWidget(problemsLabel);

    auto *tabs = new QTabWidget(&dialog);
    layout->addWidget(tabs, 1);

    // General.
    auto *general = new QWidget(tabs);
    auto *generalForm = new QFormLayout(general);
    auto *name = new QLineEdit(fields.name, general);
    auto *driver = new QComboBox(general);
    for (const FfiDriverOption &option : appSettings->databaseDrivers()) {
        driver->addItem(option.name, option.id);
    }
    selectData(driver, fields.driver);
    auto *groupRow = new QHBoxLayout();
    auto *group = new QLineEdit(fields.group, general);
    auto *color = new QLineEdit(fields.color, general);
    color->setPlaceholderText(QStringLiteral("#rrggbb"));
    auto *swatch = colorSwatch(general, fields.color);
    groupRow->addWidget(color);
    groupRow->addWidget(swatch);
    auto *host = new QLineEdit(fields.host, general);
    auto *port = portSpin(general, fields.port);
    auto *database = new QLineEdit(fields.database, general);
    auto *user = new QLineEdit(fields.user, general);
    auto *auth = new QComboBox(general);
    auth->addItem(QObject::tr("None"), QStringLiteral("none"));
    auth->addItem(QObject::tr("Password"), QStringLiteral("password"));
    auth->addItem(QObject::tr("Agent"), QStringLiteral("agent"));
    selectData(auth, fields.auth);
    auto *password = new QLineEdit(general);
    password->setEchoMode(QLineEdit::Password);
    password->setPlaceholderText(editor->hasPassword()
                                    ? QObject::tr("Stored — leave blank to keep it")
                                    : QObject::tr("Not set"));
    auto *passwordHint = new QLabel(editor->passwordHint(), general);
    passwordHint->setWordWrap(true);
    passwordHint->setStyleSheet(QStringLiteral("color: palette(mid);"));
    passwordHint->setVisible(!passwordHint->text().isEmpty());
    auto *checksRow = new QHBoxLayout();
    auto *readOnly = new QCheckBox(QObject::tr("Read-only"), general);
    readOnly->setChecked(fields.readOnly);
    auto *history = new QCheckBox(QObject::tr("History"), general);
    history->setChecked(fields.history);
    checksRow->addWidget(readOnly);
    checksRow->addWidget(history);
    auto *testRow = new QHBoxLayout();
    auto *testButton = new QPushButton(QObject::tr("Test connection"), general);
    auto *testResult = new QLabel(general);
    testResult->setWordWrap(true);
    testRow->addWidget(testButton);
    testRow->addWidget(testResult, 1);

    generalForm->addRow(QObject::tr("Name:"), name);
    generalForm->addRow(QObject::tr("Driver:"), driver);
    generalForm->addRow(QObject::tr("Group:"), group);
    generalForm->addRow(QObject::tr("Colour:"), groupRow);
    generalForm->addRow(QObject::tr("Host:"), host);
    generalForm->addRow(QObject::tr("Port:"), port);
    generalForm->addRow(QObject::tr("Database:"), database);
    generalForm->addRow(QObject::tr("User:"), user);
    generalForm->addRow(QObject::tr("Authentication:"), auth);
    generalForm->addRow(QObject::tr("Password:"), password);
    generalForm->addRow(QString(), passwordHint);
    generalForm->addRow(checksRow);
    generalForm->addRow(testRow);
    tabs->addTab(general, QObject::tr("General"));

    // SSH.
    auto *ssh = new QWidget(tabs);
    auto *sshForm = new QFormLayout(ssh);
    auto *sshHost = new QLineEdit(fields.sshHost, ssh);
    auto *sshPort = portSpin(ssh, fields.sshPort);
    auto *sshUser = new QLineEdit(fields.sshUser, ssh);
    auto *sshAuth = new QComboBox(ssh);
    sshAuth->addItem(QObject::tr("Agent"), QStringLiteral("agent"));
    sshAuth->addItem(QObject::tr("Password"), QStringLiteral("password"));
    sshAuth->addItem(QObject::tr("Key file"), QStringLiteral("key"));
    selectData(sshAuth, fields.sshAuth);
    auto *sshKeyFile = new QLineEdit(fields.sshKeyFile, ssh);
    sshForm->addRow(QObject::tr("Bastion host:"), sshHost);
    sshForm->addRow(QObject::tr("Port:"), sshPort);
    sshForm->addRow(QObject::tr("User:"), sshUser);
    sshForm->addRow(QObject::tr("Authentication:"), sshAuth);
    sshForm->addRow(QObject::tr("Key file:"), sshKeyFile);
    tabs->addTab(ssh, QObject::tr("SSH"));

    // SSL.
    auto *ssl = new QWidget(tabs);
    auto *sslForm = new QFormLayout(ssl);
    auto *sslMode = new QComboBox(ssl);
    sslMode->addItem(QObject::tr("Disable"), QStringLiteral("disable"));
    sslMode->addItem(QObject::tr("Prefer"), QStringLiteral("prefer"));
    sslMode->addItem(QObject::tr("Require"), QStringLiteral("require"));
    sslMode->addItem(QObject::tr("Verify CA"), QStringLiteral("verify-ca"));
    sslMode->addItem(QObject::tr("Verify full"), QStringLiteral("verify-full"));
    selectData(sslMode, fields.sslMode);
    auto *sslCaFile = new QLineEdit(fields.sslCaFile, ssl);
    sslForm->addRow(QObject::tr("Mode:"), sslMode);
    sslForm->addRow(QObject::tr("CA file:"), sslCaFile);
    tabs->addTab(ssl, QObject::tr("SSL"));

    // Options.
    auto *options = new QWidget(tabs);
    auto *optionsForm = new QFormLayout(options);
    auto *url = new QLineEdit(fields.url, options);
    url->setPlaceholderText(QObject::tr("A full connection URL, if this driver takes one"));
    optionsForm->addRow(QObject::tr("URL:"), url);
    tabs->addTab(options, QObject::tr("Options"));

    auto *buttons =
      new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    layout->addWidget(buttons);

    const auto refreshProblems = [editor, problemsLabel]() {
        const ::rust::Vec<FfiDataSourceProblem> problems = editor->problems();
        if (problems.empty()) {
            problemsLabel->setVisible(false);
            return;
        }
        QStringList sentences;
        for (const FfiDataSourceProblem &problem : problems) {
            sentences << QString(problem.sentence);
        }
        problemsLabel->setText(sentences.join(QStringLiteral("\n")));
        problemsLabel->setVisible(true);
    };

    QObject::connect(name, &QLineEdit::textChanged, editor, [editor, refreshProblems](const QString &text) {
        editor->setName(text);
        refreshProblems();
    });
    QObject::connect(driver, &QComboBox::currentIndexChanged, editor,
                     [editor, driver](int) { editor->setDriver(driver->currentData().toString()); });
    QObject::connect(group, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setGroup(text); });
    QObject::connect(color, &QLineEdit::textChanged, editor,
                     [editor, swatch, refreshProblems](const QString &text) {
                         editor->setColor(text);
                         swatch->setStyleSheet(
                           QStringLiteral("background-color: %1; border: 1px solid palette(mid);")
                             .arg(text.isEmpty() ? QStringLiteral("transparent") : text));
                         refreshProblems();
                     });
    QObject::connect(host, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setHost(text); });
    QObject::connect(port, &QSpinBox::valueChanged, editor, [editor, refreshProblems](int value) {
        editor->setPort(value == 0 ? QString() : QString::number(value));
        refreshProblems();
    });
    QObject::connect(database, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setDatabase(text); });
    QObject::connect(user, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setUser(text); });
    QObject::connect(auth, &QComboBox::currentIndexChanged, editor,
                     [editor, auth](int) { editor->setAuth(auth->currentData().toString()); });
    QObject::connect(password, &QLineEdit::textChanged, editor,
                     [editor, passwordHint](const QString &text) {
                         if (text.isEmpty()) {
                             return;
                         }
                         editor->setPassword(text);
                         const QString hint = editor->passwordHint();
                         passwordHint->setText(hint);
                         passwordHint->setVisible(!hint.isEmpty());
                     });
    QObject::connect(readOnly, &QCheckBox::toggled, editor,
                     [editor](bool on) { editor->setReadOnly(on); });
    QObject::connect(history, &QCheckBox::toggled, editor,
                     [editor](bool on) { editor->setHistory(on); });
    QObject::connect(url, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setUrl(text); });

    QObject::connect(sshHost, &QLineEdit::textChanged, editor,
                     [editor, refreshProblems](const QString &text) {
                         editor->setSshHost(text);
                         refreshProblems();
                     });
    QObject::connect(sshPort, &QSpinBox::valueChanged, editor, [editor](int value) {
        editor->setSshPort(value == 0 ? QString() : QString::number(value));
    });
    QObject::connect(sshUser, &QLineEdit::textChanged, editor,
                     [editor, refreshProblems](const QString &text) {
                         editor->setSshUser(text);
                         refreshProblems();
                     });
    QObject::connect(sshAuth, &QComboBox::currentIndexChanged, editor,
                     [editor, sshAuth](int) { editor->setSshAuth(sshAuth->currentData().toString()); });
    QObject::connect(sshKeyFile, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setSshKeyFile(text); });

    QObject::connect(sslMode, &QComboBox::currentIndexChanged, editor,
                     [editor, sslMode](int) { editor->setSslMode(sslMode->currentData().toString()); });
    QObject::connect(sslCaFile, &QLineEdit::textChanged, editor,
                     [editor](const QString &text) { editor->setSslCaFile(text); });

    QObject::connect(testButton, &QPushButton::clicked, editor, [editor, testResult]() {
        testResult->setStyleSheet(QString());
        testResult->setText(QObject::tr("Testing..."));
        editor->testConnection();
    });
    QObject::connect(editor, &DataSourceEditor::testConnectionFinished, &dialog,
                     [testResult](bool ok, const QString &message) {
                         testResult->setText(message);
                         testResult->setStyleSheet(ok ? QStringLiteral("color: #4caf50;")
                                                       : QStringLiteral("color: #e53935;"));
                     });

    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, [&dialog, editor]() {
        const FfiResult result = editor->commit();
        if (result.code == 0) {
            dialog.accept();
            return;
        }
        QMessageBox::warning(&dialog, QObject::tr("Could not save"), QString(result.message));
    });

    refreshProblems();
    name->setFocus();
    dialog.exec();
}

} // namespace ui_shell
