#include "run_config_php_page.h"

#include <QComboBox>
#include <QFileDialog>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QSignalBlocker>
#include <QSpinBox>
#include <QVBoxLayout>

namespace ui_shell {

PhpServerOptionsPage::PhpServerOptionsPage(QWidget *parent)
  : QWidget(parent)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);

    const auto addRow = [this, layout](const QString &label, QWidget *field) {
        auto *row = new QHBoxLayout();
        auto *labelWidget = new QLabel(label, this);
        labelWidget->setMinimumWidth(90);
        row->addWidget(labelWidget);
        row->addWidget(field, 1);
        layout->addLayout(row);
    };

    hostEdit_ = new QLineEdit(this);
    hostEdit_->setPlaceholderText(tr("localhost"));
    connect(hostEdit_, &QLineEdit::textChanged, this, &PhpServerOptionsPage::changed);
    addRow(tr("Host:"), hostEdit_);

    // 0 is "the default port"; the special-value text says so.
    portSpin_ = new QSpinBox(this);
    portSpin_->setRange(0, 65535);
    portSpin_->setSpecialValueText(tr("Default (8000)"));
    connect(portSpin_, QOverload<int>::of(&QSpinBox::valueChanged), this,
            &PhpServerOptionsPage::changed);
    addRow(tr("Port:"), portSpin_);

    documentRootEdit_ = new QLineEdit(this);
    documentRootEdit_->setPlaceholderText(tr("Project root"));
    connect(documentRootEdit_, &QLineEdit::textChanged, this, &PhpServerOptionsPage::changed);
    addRow(tr("Document root:"), documentRootEdit_);

    routerEdit_ = new QLineEdit(this);
    routerEdit_->setPlaceholderText(tr("None (serve files directly)"));
    connect(routerEdit_, &QLineEdit::textChanged, this, &PhpServerOptionsPage::changed);
    addRow(tr("Router script:"), routerEdit_);
}

void PhpServerOptionsPage::setOptions(const FfiPhpServerOptions &options)
{
    const QSignalBlocker hostBlocker(hostEdit_);
    const QSignalBlocker portBlocker(portSpin_);
    const QSignalBlocker rootBlocker(documentRootEdit_);
    const QSignalBlocker routerBlocker(routerEdit_);
    hostEdit_->setText(QString(options.host));
    portSpin_->setValue(static_cast<int>(options.port));
    documentRootEdit_->setText(QString(options.document_root));
    routerEdit_->setText(QString(options.router));
}

FfiPhpServerOptions PhpServerOptionsPage::options() const
{
    FfiPhpServerOptions options{};
    options.host = hostEdit_->text();
    options.port = static_cast<quint32>(portSpin_->value());
    options.document_root = documentRootEdit_->text();
    options.router = routerEdit_->text();
    return options;
}

PhpUnitOptionsPage::PhpUnitOptionsPage(RunConfigEditor *editor, QWidget *parent)
  : QWidget(parent)
  , editor_(editor)
{
    layout_ = new QVBoxLayout(this);
    layout_->setContentsMargins(0, 0, 0, 0);

    auto *scopeRow = new QHBoxLayout();
    auto *scopeLabel = new QLabel(tr("Test scope:"), this);
    scopeLabel->setMinimumWidth(90);
    scopeRow->addWidget(scopeLabel);
    scopeCombo_ = new QComboBox(this);
    // The data is `PhpUnitRunSetting::scope`'s spelling.
    scopeCombo_->addItem(tr("Defined in the configuration file"), QStringLiteral("config"));
    scopeCombo_->addItem(tr("Directory"), QStringLiteral("directory"));
    scopeCombo_->addItem(tr("File"), QStringLiteral("file"));
    scopeCombo_->addItem(tr("Class"), QStringLiteral("class"));
    scopeCombo_->addItem(tr("Method"), QStringLiteral("method"));
    scopeCombo_->addItem(tr("Data set"), QStringLiteral("data_set"));
    scopeRow->addWidget(scopeCombo_, 1);
    layout_->addLayout(scopeRow);
    connect(scopeCombo_, QOverload<int>::of(&QComboBox::currentIndexChanged), this, [this]() {
        showFieldsForScope();
        emit changed();
    });

    pathEdit_ = addField(tr("Path:"), true, true);
    pathEdit_->setPlaceholderText(tr("Relative to the project"));
    classEdit_ = addField(tr("Class:"), false, false);
    classEdit_->setPlaceholderText(tr("Fully qualified, e.g. App\\Tests\\GreeterTest"));
    methodEdit_ = addField(tr("Method:"), false, false);
    dataSetEdit_ = addField(tr("Data set:"), false, false);
    dataSetEdit_->setPlaceholderText(tr("\"name\" or #0"));
    filterEdit_ = addField(tr("Filter:"), false, false);
    filterEdit_->setPlaceholderText(tr("Regular expression for --filter"));
    testsuiteEdit_ = addField(tr("Test suite:"), false, false);
    testsuiteEdit_->setPlaceholderText(tr("All test suites"));
    configurationEdit_ = addField(tr("Configuration file:"), false, true);
    configurationEdit_->setPlaceholderText(tr("phpunit.xml"));
    groupsEdit_ = addField(tr("Groups:"), false, false);
    groupsEdit_->setPlaceholderText(tr("Comma-separated"));
    excludeGroupsEdit_ = addField(tr("Exclude groups:"), false, false);
    excludeGroupsEdit_->setPlaceholderText(tr("Comma-separated"));
    runnerOptionsEdit_ = addField(tr("Runner options:"), false, false);
    runnerOptionsEdit_->setPlaceholderText(tr("e.g. --stop-on-failure"));
    interpreterEdit_ = addField(tr("Interpreter:"), false, false);
    interpreterEdit_->setPlaceholderText(tr("Project PHP interpreter"));
    showFieldsForScope();
}

QLineEdit *PhpUnitOptionsPage::addField(const QString &label, bool browseDirectory, bool browseFile)
{
    auto *row = new QWidget(this);
    auto *rowLayout = new QHBoxLayout(row);
    rowLayout->setContentsMargins(0, 0, 0, 0);
    auto *labelWidget = new QLabel(label, row);
    labelWidget->setMinimumWidth(90);
    auto *edit = new QLineEdit(row);
    rowLayout->addWidget(labelWidget);
    rowLayout->addWidget(edit, 1);
    connect(edit, &QLineEdit::textChanged, this, &PhpUnitOptionsPage::changed);
    if (browseDirectory) {
        auto *button = new QPushButton(tr("Directory..."), row);
        connect(button, &QPushButton::clicked, this, [this, edit]() { browse(edit, true); });
        rowLayout->addWidget(button);
    }
    if (browseFile) {
        auto *button = new QPushButton(tr("File..."), row);
        connect(button, &QPushButton::clicked, this, [this, edit]() { browse(edit, false); });
        rowLayout->addWidget(button);
    }
    layout_->addWidget(row);
    rows_.insert(edit, row);
    return edit;
}

void PhpUnitOptionsPage::browse(QLineEdit *edit, bool directory)
{
    const QString root = editor_->projectRoot();
    const QString picked =
      directory ? QFileDialog::getExistingDirectory(this, tr("Select Directory"), root)
                : QFileDialog::getOpenFileName(this, tr("Select File"), root);
    if (picked.isEmpty()) {
        return;
    }
    const QString relative = editor_->projectRelativePath(picked);
    // Outside the project there is nothing portable to store.
    edit->setText(relative.isEmpty() ? picked : relative);
}

void PhpUnitOptionsPage::showFieldsForScope()
{
    const QString scope = scopeCombo_->currentData().toString();
    const bool path = scope == QLatin1String("directory") || scope == QLatin1String("file") ||
                      scope == QLatin1String("class") || scope == QLatin1String("method") ||
                      scope == QLatin1String("data_set");
    const bool cls = scope == QLatin1String("class") || scope == QLatin1String("method") ||
                     scope == QLatin1String("data_set");
    const bool method = scope == QLatin1String("method") || scope == QLatin1String("data_set");
    rows_.value(pathEdit_)->setVisible(path);
    rows_.value(classEdit_)->setVisible(cls);
    rows_.value(methodEdit_)->setVisible(method);
    rows_.value(dataSetEdit_)->setVisible(scope == QLatin1String("data_set"));
    rows_.value(filterEdit_)->setVisible(scope == QLatin1String("file"));
    rows_.value(testsuiteEdit_)->setVisible(scope == QLatin1String("config"));
}

void PhpUnitOptionsPage::setOptions(const FfiPhpUnitOptions &options)
{
    const QSignalBlocker scopeBlocker(scopeCombo_);
    const QList<QLineEdit *> edits = rows_.keys();
    for (QLineEdit *edit : edits) {
        edit->blockSignals(true);
    }
    // A blank or unknown scope reads as the configuration file's.
    const int scopeIndex = scopeCombo_->findData(QString(options.scope));
    scopeCombo_->setCurrentIndex(scopeIndex >= 0 ? scopeIndex : 0);
    pathEdit_->setText(QString(options.path));
    classEdit_->setText(QString(options.class_name));
    methodEdit_->setText(QString(options.method));
    dataSetEdit_->setText(QString(options.data_set));
    filterEdit_->setText(QString(options.filter));
    testsuiteEdit_->setText(QString(options.testsuite));
    configurationEdit_->setText(QString(options.configuration));
    groupsEdit_->setText(QString(options.groups));
    excludeGroupsEdit_->setText(QString(options.exclude_groups));
    runnerOptionsEdit_->setText(QString(options.runner_options));
    interpreterEdit_->setText(QString(options.interpreter));
    for (QLineEdit *edit : edits) {
        edit->blockSignals(false);
    }
    showFieldsForScope();
}

FfiPhpUnitOptions PhpUnitOptionsPage::options() const
{
    FfiPhpUnitOptions options{};
    options.scope = scopeCombo_->currentData().toString();
    options.path = pathEdit_->text();
    options.class_name = classEdit_->text();
    options.method = methodEdit_->text();
    options.data_set = dataSetEdit_->text();
    options.filter = filterEdit_->text();
    options.testsuite = testsuiteEdit_->text();
    options.configuration = configurationEdit_->text();
    options.groups = groupsEdit_->text();
    options.exclude_groups = excludeGroupsEdit_->text();
    options.runner_options = runnerOptionsEdit_->text();
    options.interpreter = interpreterEdit_->text();
    return options;
}

} // namespace ui_shell
