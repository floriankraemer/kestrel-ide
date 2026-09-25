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

// Neither list should grow the dialog around it, and an empty Excluded list
// (the common case — most projects exclude nothing) should not leave a
// stretched, mostly-void box either. Both lists share one cap so the two
// group boxes read as a matched pair rather than one dwarfing the other.
constexpr int kListMaxHeight = 140;

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
    // T5 review follow-up: the page edits an in-memory draft
    // (`ProjectTreeModel::beginScopeEdit`/`*Draft` invokables/
    // `commitScopeEdit`, the same begin/edit/commit shape Editing/Language
    // Servers already use in this dialog) rather than writing through on
    // every click — Cancel has to actually discard what was typed, and
    // nothing here may rescope until the dialog's OK handler commits.
    treeModel->beginScopeEdit();

    auto *page = new QWidget(parent);
    auto *layout = new QVBoxLayout(page);
    layout->setContentsMargins(0, 0, 0, 0);

    const bool projectOpen = !treeModel->rootPath().isEmpty();

    // Excluded folders — always the project layer, regardless of which
    // layer the dialog's own scope selector is currently showing (ADR-0064:
    // `excluded` has no global counterpart to switch to).
    auto *excludedGroup = new QGroupBox(QObject::tr("Excluded Folders — this project"), page);
    auto *excludedLayout = new QVBoxLayout(excludedGroup);
    auto *excludedList = new QListWidget(excludedGroup);
    excludedList->setMaximumHeight(kListMaxHeight);
    excludedLayout->addWidget(excludedList);

    auto *excludedEmptyHint = new QLabel(
      QObject::tr("No excluded folders. Use Add… or right-click a folder in the Project "
                  "tree → Mark Directory as Excluded."),
      excludedGroup);
    excludedEmptyHint->setWordWrap(true);
    excludedEmptyHint->setEnabled(false);
    excludedLayout->addWidget(excludedEmptyHint);

    const auto reloadExcluded = [treeModel, excludedList, excludedEmptyHint]() {
        reloadStringList(excludedList, treeModel->excludedDraft());
        excludedEmptyHint->setVisible(excludedList->count() == 0);
    };
    reloadExcluded();

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
    layout->addWidget(excludedGroup);

    QObject::connect(addExcludedButton, &QPushButton::clicked, page,
                     [page, treeModel, reloadExcluded]() {
                         const QString dir = QFileDialog::getExistingDirectory(
                           page, QObject::tr("Choose a Folder to Exclude"), treeModel->rootPath());
                         if (dir.isEmpty()) {
                             return;
                         }
                         // Validated (and, on success, added to the draft)
                         // immediately — an absolute-path/root-escape
                         // refusal has to surface at Add time, not wait for
                         // OK, or the user has no idea which entry it was.
                         const FfiResult result = treeModel->addExcludedDraft(dir);
                         warnOnFailure(page, result);
                         reloadExcluded();
                     });
    QObject::connect(removeExcludedButton, &QPushButton::clicked, page,
                     [treeModel, excludedList, reloadExcluded]() {
                         const QString selected = selectedText(excludedList);
                         if (selected.isEmpty()) {
                             return;
                         }
                         treeModel->removeExcludedDraft(selected);
                         reloadExcluded();
                     });

    // Ignored names — always the global layer, for the same reason
    // Excluded Folders above is pinned to the project one.
    auto *ignoredGroup = new QGroupBox(QObject::tr("Ignored Names — all projects"), page);
    auto *ignoredLayout = new QVBoxLayout(ignoredGroup);
    auto *ignoredList = new QListWidget(ignoredGroup);
    ignoredList->setMaximumHeight(kListMaxHeight);
    reloadStringList(ignoredList, treeModel->ignoredNamesDraft());
    ignoredLayout->addWidget(ignoredList);

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
    layout->addWidget(ignoredGroup);

    const auto reloadIgnored = [treeModel, ignoredList]() {
        reloadStringList(ignoredList, treeModel->ignoredNamesDraft());
    };
    const auto addIgnored = [treeModel, ignoredInput, reloadIgnored]() {
        const QString pattern = ignoredInput->text();
        if (pattern.trimmed().isEmpty()) {
            return;
        }
        treeModel->addIgnoredNameDraft(pattern);
        reloadIgnored();
        ignoredInput->clear();
    };
    QObject::connect(addIgnoredButton, &QPushButton::clicked, page, addIgnored);
    QObject::connect(ignoredInput, &QLineEdit::returnPressed, page, addIgnored);
    QObject::connect(removeIgnoredButton, &QPushButton::clicked, page,
                     [treeModel, ignoredList, reloadIgnored]() {
                         const QString selected = selectedText(ignoredList);
                         if (selected.isEmpty()) {
                             return;
                         }
                         treeModel->removeIgnoredNameDraft(selected);
                         reloadIgnored();
                     });
    QObject::connect(resetIgnoredButton, &QPushButton::clicked, page,
                     [page, treeModel, reloadIgnored]() {
                         const auto answer = QMessageBox::question(
                           page, QObject::tr("Reset Ignored Names"),
                           QObject::tr("Restore the default ignored-names list? Any names you "
                                       "added or removed are lost."),
                           QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel);
                         if (answer != QMessageBox::Yes) {
                             return;
                         }
                         treeModel->resetIgnoredNamesDraft();
                         reloadIgnored();
                     });

    // T5 E2E: the ignored-names input, its Add/Remove buttons and each
    // current row's rect — enough for a flow to add a pattern or remove an
    // existing one and confirm OK commits it. See `plugins_page.cpp`'s own
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
    layout->addStretch(1);

    return page;
}

} // namespace ui_shell
