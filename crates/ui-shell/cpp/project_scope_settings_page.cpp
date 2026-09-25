#include "project_scope_settings_page.h"

#include "e2e_mark.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QFileDialog>
#include <QGroupBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMessageBox>
#include <QPushButton>
#include <QString>
#include <QStringList>
#include <QTimer>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

void reloadStringList(QListWidget *list, const QStringList &entries)
{
    list->clear();
    for (const QString &entry : entries) {
        new QListWidgetItem(entry, list);
    }
}

void warnOnFailure(QWidget *page, const FfiResult &result)
{
    if (result.code != 0) {
        QMessageBox::warning(page, QObject::tr("Project Scope"), result.message);
    }
}

QString selectedText(QListWidget *list)
{
    QListWidgetItem *item = list->currentItem();
    return item ? item->text() : QString();
}

} // namespace

QWidget *buildProjectScopeSettingsPage(QWidget *parent, ProjectTreeModel *treeModel)
{
    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 0, 0, 0);

    const bool projectOpen = !treeModel->rootPath().isEmpty();

    // Excluded folders (per project).
    auto *excludedGroup = new QGroupBox(QObject::tr("Excluded Folders"), page);
    auto *excludedLayout = new QVBoxLayout(excludedGroup);
    auto *excludedList = new QListWidget(excludedGroup);
    reloadStringList(excludedList, treeModel->excludedList());
    excludedLayout->addWidget(excludedList, 1);

    if (!projectOpen) {
        auto *noProjectLabel =
          new QLabel(QObject::tr("Open a project to edit its excluded folders."), excludedGroup);
        noProjectLabel->setEnabled(false);
        noProjectLabel->setWordWrap(true);
        excludedLayout->addWidget(noProjectLabel);
    }

    auto *excludedButtons = new QHBoxLayout();
    auto *addExcludedButton = new QPushButton(QObject::tr("Add..."), excludedGroup);
    auto *removeExcludedButton = new QPushButton(QObject::tr("Remove"), excludedGroup);
    excludedButtons->addWidget(addExcludedButton);
    excludedButtons->addWidget(removeExcludedButton);
    excludedButtons->addStretch(1);
    excludedLayout->addLayout(excludedButtons);
    excludedGroup->setEnabled(projectOpen);
    layout->addWidget(excludedGroup, 1);

    QObject::connect(addExcludedButton, &QPushButton::clicked, page, [page, treeModel, excludedList]() {
        const QString dir = QFileDialog::getExistingDirectory(
          page, QObject::tr("Choose a Folder to Exclude"), treeModel->rootPath());
        if (dir.isEmpty()) {
            return;
        }
        const FfiResult result = treeModel->addExcluded(dir);
        warnOnFailure(page, result);
        reloadStringList(excludedList, treeModel->excludedList());
    });
    QObject::connect(removeExcludedButton, &QPushButton::clicked, page,
                     [page, treeModel, excludedList]() {
                         const QString selected = selectedText(excludedList);
                         if (selected.isEmpty()) {
                             return;
                         }
                         const FfiResult result = treeModel->removeExcluded(selected);
                         warnOnFailure(page, result);
                         reloadStringList(excludedList, treeModel->excludedList());
                     });

    // Ignored names (global).
    auto *ignoredGroup = new QGroupBox(QObject::tr("Ignored Names"), page);
    auto *ignoredLayout = new QVBoxLayout(ignoredGroup);
    auto *ignoredList = new QListWidget(ignoredGroup);
    reloadStringList(ignoredList, treeModel->ignoredNamesList());
    ignoredLayout->addWidget(ignoredList, 1);

    auto *ignoredInputRow = new QHBoxLayout();
    auto *ignoredInput = new QLineEdit(ignoredGroup);
    ignoredInput->setPlaceholderText(QObject::tr("Name or glob, e.g. *.bak"));
    auto *addIgnoredButton = new QPushButton(QObject::tr("Add"), ignoredGroup);
    ignoredInputRow->addWidget(ignoredInput, 1);
    ignoredInputRow->addWidget(addIgnoredButton);
    ignoredLayout->addLayout(ignoredInputRow);

    auto *ignoredButtons = new QHBoxLayout();
    auto *removeIgnoredButton = new QPushButton(QObject::tr("Remove"), ignoredGroup);
    auto *resetIgnoredButton = new QPushButton(QObject::tr("Reset to Defaults"), ignoredGroup);
    ignoredButtons->addWidget(removeIgnoredButton);
    ignoredButtons->addStretch(1);
    ignoredButtons->addWidget(resetIgnoredButton);
    ignoredLayout->addLayout(ignoredButtons);
    layout->addWidget(ignoredGroup, 1);

    const auto addIgnored = [page, treeModel, ignoredList, ignoredInput]() {
        const QString pattern = ignoredInput->text();
        if (pattern.trimmed().isEmpty()) {
            return;
        }
        const FfiResult result = treeModel->addIgnoredName(pattern);
        warnOnFailure(page, result);
        reloadStringList(ignoredList, treeModel->ignoredNamesList());
        ignoredInput->clear();
    };
    QObject::connect(addIgnoredButton, &QPushButton::clicked, page, addIgnored);
    QObject::connect(ignoredInput, &QLineEdit::returnPressed, page, addIgnored);
    QObject::connect(removeIgnoredButton, &QPushButton::clicked, page,
                     [page, treeModel, ignoredList]() {
                         const QString selected = selectedText(ignoredList);
                         if (selected.isEmpty()) {
                             return;
                         }
                         const FfiResult result = treeModel->removeIgnoredName(selected);
                         warnOnFailure(page, result);
                         reloadStringList(ignoredList, treeModel->ignoredNamesList());
                     });
    QObject::connect(resetIgnoredButton, &QPushButton::clicked, page, [page, treeModel, ignoredList]() {
        const auto answer = QMessageBox::question(
          page, QObject::tr("Reset Ignored Names"),
          QObject::tr("Restore the default ignored-names list? Any names you added or removed "
                      "are lost."),
          QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel);
        if (answer != QMessageBox::Yes) {
            return;
        }
        const FfiResult result = treeModel->resetIgnoredNames();
        warnOnFailure(page, result);
        reloadStringList(ignoredList, treeModel->ignoredNamesList());
    });

    // T5 E2E: the ignored-names input, its Add/Remove buttons and each
    // current row's rect — enough for a flow to add a pattern or remove an
    // existing one and confirm the rescope. See `plugins_page.cpp`'s own
    // `plugins_page_rows` marker for why this waits a turn: a page built by
    // `deferPage` has no real layout until the event loop runs once more
    // after the category switch that built it.
    QTimer::singleShot(0, page, [ignoredInput, addIgnoredButton, removeIgnoredButton, ignoredList]() {
        const auto rectOf = [](QWidget *widget) {
            const QPoint origin = widget->mapToGlobal(QPoint(0, 0));
            return QStringLiteral("[%1,%2,%3,%4]")
              .arg(origin.x())
              .arg(origin.y())
              .arg(widget->width())
              .arg(widget->height());
        };
        QStringList rows;
        for (int i = 0; i < ignoredList->count(); ++i) {
            QListWidgetItem *item = ignoredList->item(i);
            const QRect itemRect = ignoredList->visualItemRect(item);
            const QPoint origin = ignoredList->viewport()->mapToGlobal(itemRect.topLeft());
            rows << QStringLiteral("{\"name\":%1,\"rect\":[%2,%3,%4,%5]}")
                      .arg(e2eJson(item->text()))
                      .arg(origin.x())
                      .arg(origin.y())
                      .arg(itemRect.width())
                      .arg(itemRect.height());
        }
        e2eMark(QStringLiteral("{\"ev\":\"project_scope_page_shown\","
                                "\"ignored_input_rect\":%1,\"add_ignored_rect\":%2,"
                                "\"remove_ignored_rect\":%3,\"ignored_rows\":[%4]}")
                  .arg(rectOf(ignoredInput), rectOf(addIgnoredButton), rectOf(removeIgnoredButton))
                  .arg(rows.join(QLatin1Char(','))));
    });

    // Content rules the index applies on top of both lists (ADR-0064) —
    // stated, not configurable.
    auto *note = new QLabel(
      QObject::tr("Files larger than %1 MiB are found by name only; binary files (a NUL byte in "
                  "the first %2 KiB) and non-UTF-8 files are not indexed at all.")
        .arg(treeModel->maxIndexedFileSizeMib())
        .arg(treeModel->binarySniffKib()),
      page);
    note->setWordWrap(true);
    note->setEnabled(false);
    layout->addWidget(note);

    return page;
}

} // namespace ui_shell
