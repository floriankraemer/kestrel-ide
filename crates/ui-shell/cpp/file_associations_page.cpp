#include "file_associations_page.h"

#include <QAbstractItemView>
#include <QCheckBox>
#include <QComboBox>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QItemSelectionModel>
#include <QPushButton>
#include <QStringList>
#include <QTableWidget>
#include <QTableWidgetItem>
#include <QVBoxLayout>
#include <QVector>
#include <QWidget>

#include <algorithm>
#include <functional>

namespace ui_shell {

namespace {

constexpr int kPatternColumn = 0;
constexpr int kHandlerColumn = 1;

// One rule table plus its Add/Remove strip. Every edit — a keystroke on a
// pattern cell, a combo change, an add, a remove — reads the whole table
// back and hands it to `onChange`; the page has no partial update, the same
// as the Terminal page's environment text area.
struct RuleTable
{
    QTableWidget *table;
};

QComboBox *handlerCombo(QWidget *parent, const QStringList &handlerNames, const QString &current)
{
    auto *combo = new QComboBox(parent);
    combo->addItems(handlerNames);
    const int index = combo->findText(current);
    combo->setCurrentIndex(index >= 0 ? index : 0);
    return combo;
}

void appendRow(QTableWidget *table, const QStringList &handlerNames, const QString &pattern,
              const QString &handler)
{
    const int row = table->rowCount();
    table->insertRow(row);
    table->setItem(row, kPatternColumn, new QTableWidgetItem(pattern));
    table->setCellWidget(row, kHandlerColumn, handlerCombo(table, handlerNames, handler));
}

::rust::Vec<FfiFileAssociationRule> collectRows(QTableWidget *table)
{
    ::rust::Vec<FfiFileAssociationRule> rows;
    for (int row = 0; row < table->rowCount(); ++row) {
        const auto *patternItem = table->item(row, kPatternColumn);
        const auto *combo = qobject_cast<QComboBox *>(table->cellWidget(row, kHandlerColumn));
        if (patternItem == nullptr || combo == nullptr) {
            continue;
        }
        rows.push_back(FfiFileAssociationRule{ patternItem->text(), combo->currentText() });
    }
    return rows;
}

// Builds one table (with its Add/Remove strip) inside `box`, seeded from
// `rows`, calling `onChange` with the whole table's current content after
// every add, remove or in-place edit.
RuleTable buildRuleTable(QGroupBox *box, const QStringList &handlerNames,
                         const ::rust::Vec<FfiFileAssociationRule> &rows,
                         const std::function<void(::rust::Vec<FfiFileAssociationRule>)> &onChange)
{
    auto *layout = new QVBoxLayout(box);

    auto *table = new QTableWidget(0, 2, box);
    table->setHorizontalHeaderLabels({ QObject::tr("Pattern"), QObject::tr("Handler") });
    table->horizontalHeader()->setSectionResizeMode(kPatternColumn, QHeaderView::Stretch);
    table->horizontalHeader()->setSectionResizeMode(kHandlerColumn, QHeaderView::ResizeToContents);
    table->verticalHeader()->setVisible(false);
    table->setSelectionBehavior(QAbstractItemView::SelectRows);
    layout->addWidget(table);

    for (const FfiFileAssociationRule &rule : rows) {
        appendRow(table, handlerNames, rule.pattern, rule.handler);
    }

    auto *buttonRow = new QHBoxLayout();
    auto *addButton = new QPushButton(QObject::tr("Add"), box);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), box);
    buttonRow->addWidget(addButton);
    buttonRow->addWidget(removeButton);
    buttonRow->addStretch(1);
    layout->addLayout(buttonRow);

    QObject::connect(table, &QTableWidget::itemChanged, box,
                     [table, onChange]() { onChange(collectRows(table)); });
    QObject::connect(addButton, &QPushButton::clicked, box, [table, handlerNames, onChange]() {
        appendRow(table, handlerNames, QString(), handlerNames.isEmpty() ? QString() : handlerNames.first());
        onChange(collectRows(table));
    });
    QObject::connect(removeButton, &QPushButton::clicked, box, [table, onChange]() {
        const auto selected = table->selectionModel()->selectedRows();
        QVector<int> rowsToRemove;
        rowsToRemove.reserve(selected.size());
        for (const QModelIndex &index : selected) {
            rowsToRemove.push_back(index.row());
        }
        std::sort(rowsToRemove.begin(), rowsToRemove.end(), std::greater<int>());
        for (const int row : rowsToRemove) {
            table->removeRow(row);
        }
        onChange(collectRows(table));
    });
    // The combo's own change never touches the pattern cell, so
    // `itemChanged` above never fires for it — hooked per row instead.
    for (int row = 0; row < table->rowCount(); ++row) {
        auto *combo = qobject_cast<QComboBox *>(table->cellWidget(row, kHandlerColumn));
        QObject::connect(combo, &QComboBox::currentIndexChanged, box,
                         [table, onChange]() { onChange(collectRows(table)); });
    }

    return RuleTable{ table };
}

} // namespace

QWidget *buildFileAssociationsPage(QWidget *parent, FileAssociationsEditor *editor)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);

    const QStringList handlerNames =
      editor->handlerNames().split(QLatin1Char('\n'), Qt::SkipEmptyParts);

    auto *globalBox = new QGroupBox(QObject::tr("Global"), page);
    buildRuleTable(globalBox, handlerNames, editor->globalRules(),
                  [editor](::rust::Vec<FfiFileAssociationRule> rows) {
                      editor->setGlobalRules(std::move(rows));
                  });
    layout->addWidget(globalBox);

    auto *projectBox = new QGroupBox(QObject::tr("This project"), page);
    auto *projectLayout = new QVBoxLayout(projectBox);
    auto *overrideCheck =
      new QCheckBox(QObject::tr("Override the global associations for this project"), projectBox);
    projectLayout->addWidget(overrideCheck);

    auto *projectTableBox = new QGroupBox(projectBox);
    projectTableBox->setFlat(true);
    projectLayout->addWidget(projectTableBox);
    buildRuleTable(projectTableBox, handlerNames, editor->projectRules(),
                  [editor](::rust::Vec<FfiFileAssociationRule> rows) {
                      editor->setProjectRules(std::move(rows));
                  });

    const bool hasProject = editor->hasProject();
    overrideCheck->setChecked(hasProject && editor->projectOverrides());
    overrideCheck->setEnabled(hasProject);
    projectTableBox->setEnabled(overrideCheck->isChecked());
    projectBox->setEnabled(hasProject);
    if (!hasProject) {
        projectBox->setToolTip(QObject::tr("Open a project to override its file associations."));
    }

    QObject::connect(overrideCheck, &QCheckBox::toggled, projectBox,
                     [editor, projectTableBox](bool checked) {
                         editor->setProjectOverrides(checked);
                         projectTableBox->setEnabled(checked);
                     });

    layout->addWidget(projectBox);
    layout->addStretch(1);

    return page;
}

} // namespace ui_shell
