#include "run_config_php_page.h"

#include <QComboBox>
#include <QFileDialog>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QPushButton>
#include <QSignalBlocker>
#include <QSpinBox>
#include <QTimer>
#include <QVBoxLayout>

namespace {
// The pause after the last keystroke in the configuration file field before
// the file is read again.
constexpr int kModelRequestDelayMs = 400;
// Rows the group lists show before they scroll.
constexpr int kGroupListRows = 4;

QStringList splitNames(const QString &text)
{
    QStringList names;
    for (const QString &part : text.split(QLatin1Char(','), Qt::SkipEmptyParts)) {
        names << part.trimmed();
    }
    names.removeAll(QString());
    return names;
}
} // namespace

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

GroupPicker::GroupPicker(QWidget *parent)
  : QWidget(parent)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(2);
    list_ = new QListWidget(this);
    list_->setFixedHeight(list_->fontMetrics().lineSpacing() * kGroupListRows + 12);
    layout->addWidget(list_);
    auto *addEdit = new QLineEdit(this);
    addEdit->setPlaceholderText(tr("Add a group and press Enter"));
    layout->addWidget(addEdit);
    connect(addEdit, &QLineEdit::returnPressed, this, [this, addEdit]() {
        const QString name = addEdit->text().trimmed();
        if (!name.isEmpty()) {
            addChecked(name, true);
            emit changed();
        }
        addEdit->clear();
    });
    connect(list_, &QListWidget::itemChanged, this, &GroupPicker::changed);
}

void GroupPicker::addChecked(const QString &name, bool checked)
{
    const QList<QListWidgetItem *> found = list_->findItems(name, Qt::MatchExactly);
    QListWidgetItem *item = found.isEmpty() ? new QListWidgetItem(name, list_) : found.first();
    item->setFlags(item->flags() | Qt::ItemIsUserCheckable);
    item->setCheckState(checked ? Qt::Checked : Qt::Unchecked);
}

QStringList GroupPicker::checked() const
{
    QStringList names;
    for (int row = 0; row < list_->count(); ++row) {
        if (list_->item(row)->checkState() == Qt::Checked) {
            names << list_->item(row)->text();
        }
    }
    return names;
}

void GroupPicker::setChecked(const QStringList &names)
{
    const QSignalBlocker blocker(list_);
    for (int row = 0; row < list_->count(); ++row) {
        list_->item(row)->setCheckState(Qt::Unchecked);
    }
    for (const QString &name : names) {
        addChecked(name, true);
    }
}

void GroupPicker::setChoices(const QStringList &names)
{
    const QStringList keep = checked();
    const QSignalBlocker blocker(list_);
    list_->clear();
    for (const QString &name : names) {
        addChecked(name, false);
    }
    for (const QString &name : keep) {
        addChecked(name, true);
    }
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
    // Editable: a suite the file does not (yet) declare can still be typed.
    testsuiteCombo_ = new QComboBox(this);
    testsuiteCombo_->setEditable(true);
    testsuiteCombo_->lineEdit()->setPlaceholderText(tr("All test suites"));
    testsuiteCombo_->setInsertPolicy(QComboBox::NoInsert);
    connect(testsuiteCombo_, &QComboBox::currentTextChanged, this, &PhpUnitOptionsPage::changed);
    addRow(tr("Test suite:"), testsuiteCombo_);
    configurationEdit_ = addField(tr("Configuration file:"), false, true);
    configurationEdit_->setPlaceholderText(tr("phpunit.xml"));
    configurationProblem_ = new QLabel(this);
    configurationProblem_->setWordWrap(true);
    configurationProblem_->hide();
    layout_->addWidget(configurationProblem_);
    groupsPicker_ = new GroupPicker(this);
    connect(groupsPicker_, &GroupPicker::changed, this, &PhpUnitOptionsPage::changed);
    addRow(tr("Groups:"), groupsPicker_);
    excludeGroupsPicker_ = new GroupPicker(this);
    connect(excludeGroupsPicker_, &GroupPicker::changed, this, &PhpUnitOptionsPage::changed);
    addRow(tr("Exclude groups:"), excludeGroupsPicker_);
    runnerOptionsEdit_ = addField(tr("Runner options:"), false, false);
    runnerOptionsEdit_->setPlaceholderText(tr("e.g. --stop-on-failure"));
    interpreterEdit_ = addField(tr("Interpreter:"), false, false);
    interpreterEdit_->setPlaceholderText(tr("Project PHP interpreter"));
    showFieldsForScope();

    modelTimer_ = new QTimer(this);
    modelTimer_->setSingleShot(true);
    modelTimer_->setInterval(kModelRequestDelayMs);
    connect(modelTimer_, &QTimer::timeout, this, [this]() {
        if (isVisible()) {
            editor_->requestPhpUnitModel(configurationEdit_->text());
        }
    });
    connect(configurationEdit_, &QLineEdit::textChanged, this,
            &PhpUnitOptionsPage::scheduleModelRequest);
    connect(editor_, &RunConfigEditor::phpUnitModelReady, this,
            [this](const FfiPhpUnitModel &model) { applyModel(model); });
}

void PhpUnitOptionsPage::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    scheduleModelRequest();
}

void PhpUnitOptionsPage::scheduleModelRequest()
{
    modelTimer_->start();
}

void PhpUnitOptionsPage::applyModel(const FfiPhpUnitModel &model)
{
    // An answer for a file name the user has since changed.
    if (QString(model.configuration) != configurationEdit_->text()) {
        return;
    }
    const QString suite = testsuiteCombo_->currentText();
    {
        const QSignalBlocker blocker(testsuiteCombo_);
        testsuiteCombo_->clear();
        testsuiteCombo_->addItems(QString(model.suites).split(QLatin1Char('\n'), Qt::SkipEmptyParts));
        testsuiteCombo_->setEditText(suite);
    }
    const QStringList groups = QString(model.groups).split(QLatin1Char('\n'), Qt::SkipEmptyParts);
    groupsPicker_->setChoices(groups);
    excludeGroupsPicker_->setChoices(groups);
    const QString file = QString(model.file);
    configurationEdit_->setPlaceholderText(file.isEmpty() ? tr("phpunit.xml") : file);
    const QString problem = QString(model.problem);
    configurationProblem_->setText(problem);
    configurationProblem_->setVisible(!problem.isEmpty());
}

void PhpUnitOptionsPage::addRow(const QString &label, QWidget *field)
{
    auto *row = new QWidget(this);
    auto *rowLayout = new QHBoxLayout(row);
    rowLayout->setContentsMargins(0, 0, 0, 0);
    auto *labelWidget = new QLabel(label, row);
    labelWidget->setMinimumWidth(90);
    labelWidget->setAlignment(Qt::AlignLeft | Qt::AlignTop);
    field->setParent(row);
    rowLayout->addWidget(labelWidget);
    rowLayout->addWidget(field, 1);
    layout_->addWidget(row);
    rows_.insert(field, row);
}

QLineEdit *PhpUnitOptionsPage::addField(const QString &label, bool browseDirectory, bool browseFile)
{
    auto *edit = new QLineEdit(this);
    addRow(label, edit);
    QWidget *row = rows_.value(edit);
    auto *rowLayout = static_cast<QHBoxLayout *>(row->layout());
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
    rows_.value(testsuiteCombo_)->setVisible(scope == QLatin1String("config"));
}

void PhpUnitOptionsPage::setOptions(const FfiPhpUnitOptions &options)
{
    const QSignalBlocker scopeBlocker(scopeCombo_);
    const QSignalBlocker suiteBlocker(testsuiteCombo_);
    const QList<QLineEdit *> edits = findChildren<QLineEdit *>();
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
    testsuiteCombo_->setEditText(QString(options.testsuite));
    configurationEdit_->setText(QString(options.configuration));
    groupsPicker_->setChecked(splitNames(QString(options.groups)));
    excludeGroupsPicker_->setChecked(splitNames(QString(options.exclude_groups)));
    runnerOptionsEdit_->setText(QString(options.runner_options));
    interpreterEdit_->setText(QString(options.interpreter));
    for (QLineEdit *edit : edits) {
        edit->blockSignals(false);
    }
    showFieldsForScope();
    scheduleModelRequest();
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
    options.testsuite = testsuiteCombo_->currentText();
    options.configuration = configurationEdit_->text();
    options.groups = groupsPicker_->checked().join(QStringLiteral(", "));
    options.exclude_groups = excludeGroupsPicker_->checked().join(QStringLiteral(", "));
    options.runner_options = runnerOptionsEdit_->text();
    options.interpreter = interpreterEdit_->text();
    return options;
}

} // namespace ui_shell
