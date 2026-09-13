#include "editing_page.h"

#include <QCheckBox>
#include <QComboBox>
#include <QFormLayout>
#include <QGroupBox>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QSignalBlocker>
#include <QSpinBox>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

constexpr int kNoOverrideTabWidth = 0; // mirrors app_config::editing's own zero-is-unset idiom
constexpr int kMaxTabWidth = 16;
constexpr int kMaxWrapColumn = 500;

// A tri-state checkbox reads as "inherit" (PartiallyChecked), "on" or "off" —
// the same three answers `Option<bool>` has, which is why one widget covers
// every boolean field on both the global row and a language's override.
QCheckBox *addTriState(QFormLayout *form, const QString &label)
{
    auto *box = new QCheckBox(form->parentWidget());
    box->setTristate(true);
    form->addRow(label, box);
    return box;
}

void setTriState(QCheckBox *box, bool has, bool value)
{
    const QSignalBlocker blocker(box);
    box->setCheckState(!has          ? Qt::PartiallyChecked
                        : value      ? Qt::Checked
                                     : Qt::Unchecked);
}

// One set of the fields `EditingSettings` carries — shared between the
// global row and whichever language's row is selected, since they are the
// same shape by design (`app_config::editing`'s own module doc).
struct RowFields
{
    QSpinBox *tabWidth;
    QCheckBox *useSpaces;
    QCheckBox *trimTrailingWhitespace;
    QCheckBox *insertFinalNewline;
    QSpinBox *wrapColumn;
    QCheckBox *softWrap;
};

RowFields addRowFields(QFormLayout *form)
{
    auto *tabWidth = new QSpinBox(form->parentWidget());
    tabWidth->setRange(0, kMaxTabWidth);
    tabWidth->setSpecialValueText(QObject::tr("Default (4)"));
    form->addRow(QObject::tr("Tab width:"), tabWidth);

    QCheckBox *useSpaces = addTriState(form, QObject::tr("Indent with spaces:"));
    QCheckBox *trim = addTriState(form, QObject::tr("Trim trailing whitespace on save:"));
    QCheckBox *finalNewline = addTriState(form, QObject::tr("Insert final newline on save:"));

    auto *wrapColumn = new QSpinBox(form->parentWidget());
    wrapColumn->setRange(0, kMaxWrapColumn);
    wrapColumn->setSpecialValueText(QObject::tr("Never wrap"));
    form->addRow(QObject::tr("Wrap column:"), wrapColumn);

    QCheckBox *softWrap = addTriState(form, QObject::tr("Wrap lines at the guide (soft wrap):"));

    return RowFields{tabWidth, useSpaces, trim, finalNewline, wrapColumn, softWrap};
}

void setRowFields(const RowFields &fields, const FfiEditingRow &row)
{
    const QSignalBlocker tabBlocker(fields.tabWidth);
    fields.tabWidth->setValue(static_cast<int>(row.tab_width));
    setTriState(fields.useSpaces, row.has_use_spaces, row.use_spaces);
    setTriState(fields.trimTrailingWhitespace, row.has_trim_trailing_whitespace,
               row.trim_trailing_whitespace);
    setTriState(fields.insertFinalNewline, row.has_insert_final_newline,
               row.insert_final_newline);
    const QSignalBlocker wrapBlocker(fields.wrapColumn);
    fields.wrapColumn->setValue(static_cast<int>(row.wrap_column));
    setTriState(fields.softWrap, row.has_soft_wrap, row.soft_wrap);
}

// The inverse of `setRowFields`: what the widgets say right now, folded back
// into the wire shape `setGlobalRow`/`setLanguageRow` take. `languageId` and
// the encoding/line-ending pair are threaded through unchanged by the
// caller, since neither panel edits both.
FfiEditingRow readRowFields(const RowFields &fields, const QString &languageId,
                           const QString &languageName, const QString &defaultEncoding,
                           const QString &lineEndings)
{
    FfiEditingRow row{};
    row.language_id = languageId;
    row.language_name = languageName;
    row.tab_width = static_cast<quint32>(fields.tabWidth->value());
    row.has_use_spaces = fields.useSpaces->checkState() != Qt::PartiallyChecked;
    row.use_spaces = fields.useSpaces->checkState() == Qt::Checked;
    row.has_trim_trailing_whitespace =
      fields.trimTrailingWhitespace->checkState() != Qt::PartiallyChecked;
    row.trim_trailing_whitespace =
      fields.trimTrailingWhitespace->checkState() == Qt::Checked;
    row.has_insert_final_newline =
      fields.insertFinalNewline->checkState() != Qt::PartiallyChecked;
    row.insert_final_newline = fields.insertFinalNewline->checkState() == Qt::Checked;
    // `Some(0)` and unset are the same "never wrap" answer
    // (`EditingSettings::wrap_column_or_default`), so the widget need not
    // distinguish them — zero always reads back as unset.
    row.has_wrap_column = fields.wrapColumn->value() > 0;
    row.wrap_column = static_cast<quint32>(fields.wrapColumn->value());
    row.has_soft_wrap = fields.softWrap->checkState() != Qt::PartiallyChecked;
    row.soft_wrap = fields.softWrap->checkState() == Qt::Checked;
    row.default_encoding = defaultEncoding;
    row.line_endings = lineEndings;
    return row;
}

} // namespace

QWidget *buildEditingPage(QWidget *parent, EditingEditor *editor)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);

    auto *globalBox = new QGroupBox(QObject::tr("All languages"), page);
    auto *globalForm = new QFormLayout(globalBox);
    const RowFields globalFields = addRowFields(globalForm);
    // #143: the one widget an E2E flow needs a stable handle to — found by
    // `settings_dialog.cpp` via `findChild` for the `dialog_shown` mark's
    // rect, the same convention `run_config_dialog.cpp` uses for its own
    // Add/Program/Save widgets. Only the "All languages" row needs a name:
    // the "Language override" row's own `tabWidth` (same `RowFields`
    // shape, `languageFields` below) is never the flow's target.
    globalFields.tabWidth->setObjectName(QStringLiteral("editingTabWidth"));

    auto *encodingEdit = new QLineEdit(globalBox);
    encodingEdit->setPlaceholderText(QObject::tr("utf-8"));
    globalForm->addRow(QObject::tr("Default encoding:"), encodingEdit);

    auto *lineEndingCombo = new QComboBox(globalBox);
    lineEndingCombo->addItem(QObject::tr("Preserve"), QStringLiteral("preserve"));
    lineEndingCombo->addItem(QStringLiteral("LF"), QStringLiteral("lf"));
    lineEndingCombo->addItem(QStringLiteral("CRLF"), QStringLiteral("crlf"));
    lineEndingCombo->addItem(QObject::tr("Platform"), QStringLiteral("platform"));
    globalForm->addRow(QObject::tr("Line endings:"), lineEndingCombo);

    layout->addWidget(globalBox);

    const FfiEditingRow globalRow = editor->globalRow();
    setRowFields(globalFields, globalRow);
    encodingEdit->setText(globalRow.default_encoding);
    const int lineEndingIndex =
      lineEndingCombo->findData(globalRow.line_endings.isEmpty() ? QStringLiteral("preserve")
                                                                  : globalRow.line_endings);
    lineEndingCombo->setCurrentIndex(qMax(0, lineEndingIndex));

    auto pushGlobalRow = [editor, globalFields, encodingEdit, lineEndingCombo]() {
        editor->setGlobalRow(readRowFields(globalFields, QString(), QString(),
                                           encodingEdit->text(),
                                           lineEndingCombo->currentData().toString()));
    };
    QObject::connect(globalFields.tabWidth, &QSpinBox::valueChanged, page, pushGlobalRow);
    QObject::connect(globalFields.useSpaces, &QCheckBox::stateChanged, page, pushGlobalRow);
    QObject::connect(globalFields.trimTrailingWhitespace, &QCheckBox::stateChanged, page,
                     pushGlobalRow);
    QObject::connect(globalFields.insertFinalNewline, &QCheckBox::stateChanged, page,
                     pushGlobalRow);
    QObject::connect(globalFields.wrapColumn, &QSpinBox::valueChanged, page, pushGlobalRow);
    QObject::connect(globalFields.softWrap, &QCheckBox::stateChanged, page, pushGlobalRow);
    QObject::connect(encodingEdit, &QLineEdit::textChanged, page, pushGlobalRow);
    QObject::connect(lineEndingCombo, &QComboBox::currentIndexChanged, page, pushGlobalRow);

    // Per-language: every field starts at "inherit" (`EditingSettings`'s own
    // default), which is exactly what a language with no override already
    // has — so switching the combo to a language nobody has touched shows
    // three dashes, not a copy of the global row.
    auto *languageBox = new QGroupBox(QObject::tr("Language override"), page);
    auto *languageLayout = new QVBoxLayout(languageBox);
    auto *languageCombo = new QComboBox(languageBox);
    languageLayout->addWidget(languageCombo);
    auto *languageForm = new QFormLayout();
    const RowFields languageFields = addRowFields(languageForm);
    languageLayout->addLayout(languageForm);

    auto *resolvedLabel = new QLabel(languageBox);
    languageLayout->addWidget(resolvedLabel);

    layout->addWidget(languageBox);

    const ::rust::Vec<FfiEditingRow> languageRows = editor->languageRows();
    for (const FfiEditingRow &row : languageRows) {
        languageCombo->addItem(row.language_name, row.language_id);
    }

    auto refreshResolvedLabel = [editor, languageCombo, resolvedLabel]() {
        const QString id = languageCombo->currentData().toString();
        resolvedLabel->setText(
          QObject::tr("Effective tab width for %1: %2")
            .arg(languageCombo->currentText())
            .arg(editor->resolvedTabWidth(id)));
    };

    auto showLanguageRow = [editor, languageCombo, languageFields, refreshResolvedLabel]() {
        const QString id = languageCombo->currentData().toString();
        const ::rust::Vec<FfiEditingRow> rows = editor->languageRows();
        for (const FfiEditingRow &row : rows) {
            if (row.language_id == id) {
                setRowFields(languageFields, row);
                break;
            }
        }
        refreshResolvedLabel();
    };

    auto pushLanguageRow = [editor, languageCombo, languageFields, refreshResolvedLabel]() {
        const QString id = languageCombo->currentData().toString();
        editor->setLanguageRow(
          readRowFields(languageFields, id, languageCombo->currentText(), QString(), QString()));
        refreshResolvedLabel();
    };
    QObject::connect(languageCombo, &QComboBox::currentIndexChanged, page, showLanguageRow);
    QObject::connect(languageFields.tabWidth, &QSpinBox::valueChanged, page, pushLanguageRow);
    QObject::connect(languageFields.useSpaces, &QCheckBox::stateChanged, page, pushLanguageRow);
    QObject::connect(languageFields.trimTrailingWhitespace, &QCheckBox::stateChanged, page,
                     pushLanguageRow);
    QObject::connect(languageFields.insertFinalNewline, &QCheckBox::stateChanged, page,
                     pushLanguageRow);
    QObject::connect(languageFields.wrapColumn, &QSpinBox::valueChanged, page, pushLanguageRow);
    QObject::connect(languageFields.softWrap, &QCheckBox::stateChanged, page, pushLanguageRow);

    if (!languageRows.empty()) {
        showLanguageRow();
    }

    return page;
}

bool commitEditingPage(QWidget *parent, EditingEditor *editor)
{
    const ::rust::Vec<FfiEditingProblem> problems = editor->problems();
    if (!problems.empty()) {
        QMessageBox::warning(parent, QObject::tr("Editing"), problems.front().sentence);
        return false;
    }
    const FfiResult saved = editor->commit();
    if (saved.code != 0) {
        QMessageBox::critical(parent, QObject::tr("Editing"), saved.message);
        return false;
    }
    return true;
}

} // namespace ui_shell
