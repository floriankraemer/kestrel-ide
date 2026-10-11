#include "syntax_colors_page.h"

#include "theme.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QColor>
#include <QColorDialog>
#include <QComboBox>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QPushButton>
#include <QShortcut>
#include <QSignalBlocker>
#include <QStringList>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

constexpr int kScopeColumn = 0;
constexpr int kSampleColumn = 1;
constexpr int kStyleColumn = 2;
constexpr int kFromColumn = 3;
constexpr int kScopeRole = Qt::UserRole;

// The language id the combo currently selects; empty means the base table,
// which is exactly what the bridge takes for "all languages".
QString selectedLanguage(const QComboBox *combo)
{
    return combo->currentData().toString();
}

// `#rrggbb`, which is what the config stores and what `Rgb::parse` accepts;
// QColor would also take "red", and storing that would silently lose the
// colour on the next load.
bool isHexColor(const QString &text)
{
    return text.size() == 7 && text.startsWith(QLatin1Char('#'))
      && QColor::isValidColorName(text);
}

QString selectedScope(const QTreeWidget *tree)
{
    const QTreeWidgetItem *item = tree->currentItem();
    return item ? item->data(kScopeColumn, kScopeRole).toString() : QString();
}

// "B I U" for whatever the resolved style actually turns on — the Style
// column describes what will be painted, not what this level stores.
QString styleFlags(const FfiSyntaxScopeRow &row)
{
    QStringList flags;
    if (row.sample_bold) {
        flags << QStringLiteral("B");
    }
    if (row.sample_italic) {
        flags << QStringLiteral("I");
    }
    if (row.sample_underline) {
        flags << QStringLiteral("U");
    }
    return flags.join(QLatin1Char(' '));
}

void applySampleAppearance(QTreeWidgetItem *item, const FfiSyntaxScopeRow &row,
                            const QFont &baseFont)
{
    if (row.has_fg) {
        item->setForeground(kSampleColumn, QColor(row.red, row.green, row.blue));
    }
    QFont font = baseFont;
    font.setBold(row.sample_bold);
    font.setItalic(row.sample_italic);
    font.setUnderline(row.sample_underline);
    item->setFont(kSampleColumn, font);
}

// Rebuilds the tree from the draft, restoring the selection on
// `keepScope` so picking a colour doesn't bounce the user to the top.
void populate(QTreeWidget *tree, SyntaxColorEditor *editor, const QString &languageId,
              const QString &languageName, const QFont &sampleFont, const QString &keepScope)
{
    const QSignalBlocker blocker(tree);
    tree->clear();

    const SemanticColors colors = semanticColors();
    QTreeWidgetItem *family = nullptr;
    QString familyName;

    for (const FfiSyntaxScopeRow &row : editor->scopes(languageId)) {
        if (!family || familyName != row.family) {
            familyName = row.family;
            family = new QTreeWidgetItem(tree, QStringList{familyName});
            family->setFlags(Qt::ItemIsEnabled);
            family->setExpanded(true);
        }

        auto *item = new QTreeWidgetItem(
          family, QStringList{row.scope, row.sample, styleFlags(row), QString()});
        item->setData(kScopeColumn, kScopeRole, row.scope);
        applySampleAppearance(item, row, sampleFont);

        switch (row.origin) {
        case FfiColorOrigin::Theme:
            item->setText(kFromColumn, QObject::tr("Theme"));
            item->setForeground(kFromColumn, colors.muted);
            break;
        case FfiColorOrigin::Base:
            item->setText(kFromColumn, QObject::tr("Base"));
            break;
        case FfiColorOrigin::Language: {
            item->setText(kFromColumn, languageName);
            QFont font = item->font(kFromColumn);
            font.setBold(true);
            item->setFont(kFromColumn, font);
            break;
        }
        }

        if (row.scope == keepScope) {
            tree->setCurrentItem(item);
        }
    }
    tree->expandAll();
}

} // namespace

QWidget *buildSyntaxColorsPage(QWidget *parent,
                               SyntaxColorEditor *editor,
                               const QFont &sampleFont,
                               std::function<void()> onChanged)
{
    auto *page = new QWidget(parent);
    page->setMinimumSize(560, 460);
    auto *layout = new QVBoxLayout(page);

    auto *languageCombo = new QComboBox(page);
    languageCombo->addItem(QObject::tr("(Base — all languages)"), QString());
    languageCombo->insertSeparator(languageCombo->count());
    for (const FfiLanguageOption &option : editor->languages()) {
        languageCombo->addItem(option.name, option.id);
    }

    // A scope name the file names but this build does not know has no row to
    // show itself in, so it is said in words, above the table and for as long
    // as the page is open. The sentence is `settings-model`'s.
    auto *unknownScopeLabel = new QLabel(editor->unknownScopeWarning(), page);
    unknownScopeLabel->setWordWrap(true);
    unknownScopeLabel->setStyleSheet(
      QStringLiteral("color: %1;").arg(semanticColors().warning.name()));
    unknownScopeLabel->setVisible(!unknownScopeLabel->text().isEmpty());
    layout->addWidget(unknownScopeLabel);

    auto *resetLevelButton = new QPushButton(page);
    auto *topRow = new QHBoxLayout();
    topRow->addWidget(new QLabel(QObject::tr("Language:"), page));
    topRow->addWidget(languageCombo, 1);
    topRow->addWidget(resetLevelButton);
    layout->addLayout(topRow);

    auto *tree = new QTreeWidget(page);
    tree->setColumnCount(4);
    tree->setHeaderLabels({QObject::tr("Scope"), QObject::tr("Sample"), QObject::tr("Style"),
                           QObject::tr("From")});
    tree->header()->setSectionResizeMode(kScopeColumn, QHeaderView::Stretch);
    tree->header()->setSectionResizeMode(kSampleColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kStyleColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kFromColumn, QHeaderView::ResizeToContents);
    tree->setRootIsDecorated(false);
    tree->setIndentation(12);
    layout->addWidget(tree, 1);

    auto *hexEdit = new QLineEdit(page);
    hexEdit->setPlaceholderText(QStringLiteral("#rrggbb"));
    hexEdit->setMaximumWidth(100);
    auto *chooseButton = new QPushButton(QObject::tr("Choose…"), page);
    auto *boldCheck = new QCheckBox(QObject::tr("Bold"), page);
    auto *italicCheck = new QCheckBox(QObject::tr("Italic"), page);
    auto *underlineCheck = new QCheckBox(QObject::tr("Underline"), page);
    auto *resetScopeButton = new QPushButton(QObject::tr("Reset Scope"), page);
    resetScopeButton->setToolTip(
      QObject::tr("Remove this language's override and inherit the base style."));

    auto *controls = new QHBoxLayout();
    controls->addWidget(new QLabel(QObject::tr("Color:"), page));
    controls->addWidget(hexEdit);
    controls->addWidget(chooseButton);
    controls->addWidget(boldCheck);
    controls->addWidget(italicCheck);
    controls->addWidget(underlineCheck);
    controls->addStretch(1);
    controls->addWidget(resetScopeButton);
    layout->addLayout(controls);

    // Never a modal for a mistyped colour: the previous value stays applied
    // and the message sits under the strip until the field is valid again.
    auto *errorLabel = new QLabel(page);
    errorLabel->setVisible(false);
    layout->addWidget(errorLabel);

    // The page's own state is exactly two strings: which language is
    // selected, and which row. Everything else is asked for again.
    auto languageName = std::make_shared<QString>(languageCombo->currentText());

    auto refresh = [tree, editor, languageCombo, languageName, sampleFont](
                     const QString &keepScope) {
        populate(tree, editor, selectedLanguage(languageCombo), *languageName, sampleFont,
                 keepScope);
    };

    auto syncLevelButton = [editor, languageCombo, resetLevelButton]() {
        const QString languageId = selectedLanguage(languageCombo);
        resetLevelButton->setText(languageId.isEmpty() ? QObject::tr("Reset Base…")
                                                       : QObject::tr("Reset Language…"));
        resetLevelButton->setEnabled(editor->canResetLevel(languageId));
    };

    // Seeds the strip from a row, so touching nothing and then toggling one
    // flag cannot silently rewrite the colour — and so `Reset Scope` is
    // disabled again the moment the row has nothing left to reset.
    auto loadStrip = [=](const QString &scope) {
        const bool isScope = !scope.isEmpty();
        hexEdit->setEnabled(isScope);
        chooseButton->setEnabled(isScope);
        boldCheck->setEnabled(isScope);
        italicCheck->setEnabled(isScope);
        underlineCheck->setEnabled(isScope);
        errorLabel->setVisible(false);
        if (!isScope) {
            resetScopeButton->setEnabled(false);
            return;
        }

        for (const FfiSyntaxScopeRow &row : editor->scopes(selectedLanguage(languageCombo))) {
            if (row.scope != scope) {
                continue;
            }
            const QSignalBlocker hexBlocker(hexEdit);
            const QSignalBlocker boldBlocker(boldCheck);
            const QSignalBlocker italicBlocker(italicCheck);
            const QSignalBlocker underlineBlocker(underlineCheck);
            hexEdit->setText(row.hex);
            boldCheck->setChecked(row.bold);
            italicCheck->setChecked(row.italic);
            underlineCheck->setChecked(row.underline);
            resetScopeButton->setEnabled(row.can_reset);
            break;
        }
    };

    // Reads the strip and writes it to the draft. One place, so the colour
    // dialog, the hex field and the three checkboxes cannot disagree.
    auto applyStrip = [=]() {
        const QString scope = selectedScope(tree);
        if (scope.isEmpty()) {
            return;
        }
        const QString hex = hexEdit->text().trimmed();
        if (!hex.isEmpty() && !isHexColor(hex)) {
            errorLabel->setText(QObject::tr("%1 is not a colour. Enter #rrggbb.").arg(hex));
            errorLabel->setStyleSheet(
              QStringLiteral("color: %1;").arg(semanticColors().error.name()));
            errorLabel->setVisible(true);
            return;
        }
        errorLabel->setVisible(false);
        editor->setStyle(selectedLanguage(languageCombo), scope, hex, boldCheck->isChecked(),
                         italicCheck->isChecked(), underlineCheck->isChecked());
        refresh(scope);
        loadStrip(scope);
        syncLevelButton();
        if (onChanged) {
            onChanged();
        }
    };

    QObject::connect(tree, &QTreeWidget::currentItemChanged, page,
                      [=](QTreeWidgetItem *current) {
                          loadStrip(current ? current->data(kScopeColumn, kScopeRole).toString()
                                            : QString());
                      });

    QObject::connect(languageCombo, &QComboBox::currentIndexChanged, page,
                      [=]() {
                          // A separator is not a language; step past it
                          // rather than showing an empty table.
                          if (languageCombo->currentText().isEmpty()) {
                              languageCombo->setCurrentIndex(0);
                              return;
                          }
                          *languageName = languageCombo->currentText();
                          refresh(QString());
                          loadStrip(selectedScope(tree));
                          syncLevelButton();
                      });

    QObject::connect(chooseButton, &QPushButton::clicked, page, [=]() {
        const QString current = hexEdit->text().trimmed();
        const QColor initial = isHexColor(current) ? QColor(current)
                                                  : page->palette().text().color();
        const QColor chosen = QColorDialog::getColor(initial, page, QObject::tr("Scope Color"));
        if (!chosen.isValid()) {
            return;
        }
        hexEdit->setText(chosen.name());
        applyStrip();
    });

    QObject::connect(hexEdit, &QLineEdit::editingFinished, page, applyStrip);
    QObject::connect(boldCheck, &QCheckBox::toggled, page, applyStrip);
    QObject::connect(italicCheck, &QCheckBox::toggled, page, applyStrip);
    QObject::connect(underlineCheck, &QCheckBox::toggled, page, applyStrip);

    // Enter on a row opens the picker, so a keyboard user never has to tab
    // to `Choose...`; Ctrl+B/I/U toggle the flags without leaving the tree.
    QObject::connect(tree, &QTreeWidget::itemActivated, page,
                      [chooseButton]() { chooseButton->click(); });
    for (const auto &binding : {std::pair{QStringLiteral("Ctrl+B"), boldCheck},
                                std::pair{QStringLiteral("Ctrl+I"), italicCheck},
                                std::pair{QStringLiteral("Ctrl+U"), underlineCheck}}) {
        auto *shortcut = new QShortcut(QKeySequence(binding.first), tree);
        shortcut->setContext(Qt::WidgetShortcut);
        QCheckBox *check = binding.second;
        QObject::connect(shortcut, &QShortcut::activated, page, [check]() {
            if (check->isEnabled()) {
                check->toggle();
            }
        });
    }

    QObject::connect(resetScopeButton, &QPushButton::clicked, page, [=]() {
        const QString scope = selectedScope(tree);
        if (scope.isEmpty()) {
            return;
        }
        editor->resetScope(selectedLanguage(languageCombo), scope);
        refresh(scope);
        loadStrip(scope);
        syncLevelButton();
        if (onChanged) {
            onChanged();
        }
    });

    QObject::connect(resetLevelButton, &QPushButton::clicked, page, [=]() {
        const QString languageId = selectedLanguage(languageCombo);
        const QString question =
          languageId.isEmpty()
            ? QObject::tr("Remove every colour customisation and inherit the theme's styles?")
            : QObject::tr("Remove all %1 colour overrides and inherit the base styles?")
                .arg(*languageName);
        const auto answer =
          QMessageBox::question(page, QObject::tr("Reset Colors"), question,
                                QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel);
        if (answer != QMessageBox::Yes) {
            return;
        }
        editor->resetLevel(languageId);
        refresh(selectedScope(tree));
        loadStrip(selectedScope(tree));
        syncLevelButton();
        if (onChanged) {
            onChanged();
        }
    });

    refresh(QString());
    loadStrip(QString());
    syncLevelButton();
    return page;
}

} // namespace ui_shell
