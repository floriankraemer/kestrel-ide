#include "run_config_php_page.h"

#include "e2e_mark.h"

#include <QAbstractSpinBox>
#include <QComboBox>
#include <QFileDialog>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMessageBox>
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

namespace {
// Marks a label as one of the form's, for `alignFormLabels`.
const char *const kFormLabel = "runConfigFormLabel";

QString rectJson(const QWidget *widget)
{
    const QPoint origin = widget->mapToGlobal(QPoint(0, 0));
    return QStringLiteral("[%1,%2,%3,%4]")
      .arg(origin.x())
      .arg(origin.y())
      .arg(widget->width())
      .arg(widget->height());
}
} // namespace

QWidget *addFormRow(QVBoxLayout *layout, QWidget *owner, const QString &label, QWidget *field)
{
    auto *row = new QWidget(owner);
    auto *rowLayout = new QHBoxLayout(row);
    rowLayout->setContentsMargins(0, 0, 0, 0);
    auto *labelWidget = new QLabel(label, row);
    labelWidget->setProperty(kFormLabel, true);
    field->setParent(row);
    // The label centres its text in one input line's height at the top of
    // the row: beside a single-line field it shares the field's text line,
    // beside a list it sits on the first row, never above either (#460).
    const bool singleLine = qobject_cast<QLineEdit *>(field) != nullptr
      || qobject_cast<QComboBox *>(field) != nullptr
      || qobject_cast<QAbstractSpinBox *>(field) != nullptr;
    QLineEdit probe(row);
    QWidget *lineField = singleLine ? field : &probe;
    lineField->ensurePolished();
    labelWidget->setMinimumHeight(lineField->sizeHint().height());
    labelWidget->setAlignment(Qt::AlignLeft | Qt::AlignVCenter);
    rowLayout->addWidget(labelWidget, 0, Qt::AlignTop);
    rowLayout->addWidget(field, 1);
    layout->addWidget(row);
    return row;
}

void alignFormLabels(QWidget *root)
{
    const QList<QLabel *> labels = root->findChildren<QLabel *>();
    int width = 0;
    for (const QLabel *label : labels) {
        if (label->property(kFormLabel).toBool()) {
            width = qMax(width, label->sizeHint().width());
        }
    }
    for (QLabel *label : labels) {
        if (label->property(kFormLabel).toBool()) {
            label->setFixedWidth(width);
        }
    }
}

QString formRowsJson(const QWidget *root)
{
    QStringList rows;
    for (const QLabel *label : root->findChildren<QLabel *>()) {
        if (!label->property(kFormLabel).toBool() || !label->isVisible()) {
            continue;
        }
        const QLayout *rowLayout = label->parentWidget()->layout();
        const QLayoutItem *fieldItem = rowLayout != nullptr ? rowLayout->itemAt(1) : nullptr;
        const QWidget *field = fieldItem != nullptr ? fieldItem->widget() : nullptr;
        if (field == nullptr) {
            continue;
        }
        rows << QStringLiteral("{\"label\":%1,\"field_x\":%2}")
                  .arg(e2eJson(label->text()))
                  .arg(field->mapToGlobal(QPoint(0, 0)).x());
    }
    return QStringLiteral("[%1]").arg(rows.join(QLatin1Char(',')));
}

PhpServerOptionsPage::PhpServerOptionsPage(QWidget *parent)
  : QWidget(parent)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);

    const auto addRow = [this, layout](const QString &label, QWidget *field) {
        addFormRow(layout, this, label, field);
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
    addEdit_ = new QLineEdit(this);
    addEdit_->setPlaceholderText(tr("Add a group and press Enter"));
    layout->addWidget(addEdit_);
    // Enter adds the group here; left to propagate, it would also press the
    // dialog's default button and save and close the dialog.
    addEdit_->installEventFilter(this);
    connect(list_, &QListWidget::itemChanged, this, &GroupPicker::changed);
}

bool GroupPicker::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == addEdit_ && event->type() == QEvent::KeyPress) {
        const int key = static_cast<QKeyEvent *>(event)->key();
        if (key == Qt::Key_Return || key == Qt::Key_Enter) {
            const QString name = addEdit_->text().trimmed();
            if (!name.isEmpty()) {
                addChecked(name, true);
                emit changed();
            }
            addEdit_->clear();
            return true;
        }
    }
    return QWidget::eventFilter(watched, event);
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

    scopeCombo_ = new QComboBox(this);
    // The data is `PhpUnitRunSetting::scope`'s spelling.
    scopeCombo_->addItem(tr("Defined in the configuration file"), QStringLiteral("config"));
    scopeCombo_->addItem(tr("Directory"), QStringLiteral("directory"));
    scopeCombo_->addItem(tr("File"), QStringLiteral("file"));
    scopeCombo_->addItem(tr("Class"), QStringLiteral("class"));
    scopeCombo_->addItem(tr("Method"), QStringLiteral("method"));
    scopeCombo_->addItem(tr("Data set"), QStringLiteral("data_set"));
    addRow(tr("Test scope:"), scopeCombo_);
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
    interpreterOptionsEdit_ = addField(tr("Interpreter options:"), false, false);
    interpreterOptionsEdit_->setPlaceholderText(tr("e.g. -d memory_limit=-1"));
    workingDirectoryEdit_ = addField(tr("Custom working directory:"), true, false);
    workingDirectoryEdit_->setPlaceholderText(tr("Project root"));
    showFieldsForScope();

    modelTimer_ = new QTimer(this);
    modelTimer_->setSingleShot(true);
    modelTimer_->setInterval(kModelRequestDelayMs);
    connect(modelTimer_, &QTimer::timeout, this, [this]() {
        if (isVisible()) {
            editor_->requestPhpUnitModel(configurationEdit_->text(),
                                         workingDirectoryEdit_->text());
        }
    });
    connect(configurationEdit_, &QLineEdit::textChanged, this,
            &PhpUnitOptionsPage::scheduleModelRequest);
    connect(workingDirectoryEdit_, &QLineEdit::textChanged, this,
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
    // An answer for a file name or directory the user has since changed.
    if (QString(model.configuration) != configurationEdit_->text()
        || QString(model.working_directory) != workingDirectoryEdit_->text()) {
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
    const bool hasProblem = model.problem.code != 0;
    configurationProblem_->setText(hasProblem ? QString(model.problem.message) : QString());
    configurationProblem_->setVisible(hasProblem);
    // What the pickers now offer, as `phpunit.xml` declares it.
    QStringList suites;
    for (int i = 0; i < testsuiteCombo_->count(); ++i) {
        suites << e2eJson(testsuiteCombo_->itemText(i));
    }
    QStringList offered;
    for (const QString &group : groups) {
        offered << e2eJson(group);
    }
    e2eMark(QStringLiteral("{\"ev\":\"phpunit_page_model\",\"file\":%1,\"suites\":[%2],"
                           "\"groups\":[%3],\"problem\":%4}")
              .arg(e2eJson(file), suites.join(QLatin1Char(',')), offered.join(QLatin1Char(',')),
                   e2eJson(configurationProblem_->text())));
}

void PhpUnitOptionsPage::addRow(const QString &label, QWidget *field)
{
    rows_.insert(field, addFormRow(layout_, this, label, field));
}

QString PhpUnitOptionsPage::e2eFieldsJson() const
{
    return QStringLiteral("{\"configuration_rect\":%1,\"configuration\":%2,"
                          "\"group_add_rect\":%3,\"groups\":%4,"
                          "\"working_directory\":%5}")
      .arg(rectJson(configurationEdit_), e2eJson(configurationEdit_->text()),
           rectJson(groupsPicker_->addEdit()), e2eJson(groupsPicker_->checked().join(QLatin1Char(','))),
           e2eJson(workingDirectoryEdit_->text()));
}

QLineEdit *PhpUnitOptionsPage::addField(const QString &label, bool browseDirectory, bool browseFile)
{
    auto *edit = new QLineEdit(this);
    addRow(label, edit);
    QWidget *row = rows_.value(edit);
    auto *rowLayout = static_cast<QHBoxLayout *>(row->layout());
    connect(edit, &QLineEdit::textChanged, this, &PhpUnitOptionsPage::changed);
    if (browseDirectory) {
        auto *button = new QPushButton(tr("Directory…"), row);
        connect(button, &QPushButton::clicked, this, [this, edit]() { browse(edit, true); });
        rowLayout->addWidget(button);
    }
    if (browseFile) {
        auto *button = new QPushButton(tr("File…"), row);
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
    // Outside the project there is nothing the run could pass: paths are
    // passed relative to a working directory inside the project.
    if (relative.isEmpty()) {
        QMessageBox::warning(this, tr("Outside the Project"),
                             tr("Choose a file or directory inside the project."));
        return;
    }
    edit->setText(relative);
}

void PhpUnitOptionsPage::showFieldsForScope()
{
    const FfiPhpUnitScopeFields fields =
      editor_->phpUnitScopeFields(scopeCombo_->currentData().toString());
    rows_.value(pathEdit_)->setVisible(fields.path);
    rows_.value(classEdit_)->setVisible(fields.class_name);
    rows_.value(methodEdit_)->setVisible(fields.method);
    rows_.value(dataSetEdit_)->setVisible(fields.data_set);
    rows_.value(filterEdit_)->setVisible(fields.filter);
    rows_.value(testsuiteCombo_)->setVisible(fields.testsuite);
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
    interpreterOptionsEdit_->setText(QString(options.interpreter_options));
    workingDirectoryEdit_->setText(QString(options.working_directory));
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
    options.interpreter_options = interpreterOptionsEdit_->text();
    options.working_directory = workingDirectoryEdit_->text();
    return options;
}

} // namespace ui_shell
