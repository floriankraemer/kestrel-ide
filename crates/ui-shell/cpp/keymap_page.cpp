#include "keymap_page.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAction>
#include <QFont>
#include <QHash>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QKeySequence>
#include <QKeySequenceEdit>
#include <QLabel>
#include <QMenu>
#include <QMessageBox>
#include <QPushButton>
#include <QStringList>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

namespace {

// Column layout of the action tree, and the role the action id rides in.
constexpr int kActionColumn = 0;
constexpr int kShortcutColumn = 1;
constexpr int kActionIdRole = Qt::UserRole;

// Rebuilds the tree from the draft keymap, restoring the selection on
// `selectedActionId` so an assign/clear doesn't bounce the user back to the
// top of the list.
void populate(QTreeWidget *tree, KeymapEditor *editor, const QString &selectedActionId)
{
    tree->clear();

    QHash<QString, QTreeWidgetItem *> categories;
    const rust::Vec<FfiKeyBinding> bindings = editor->bindings();
    for (const FfiKeyBinding &binding : bindings) {
        QTreeWidgetItem *category = categories.value(binding.category);
        if (!category) {
            category = new QTreeWidgetItem(tree, QStringList{binding.category});
            category->setFlags(Qt::ItemIsEnabled);
            category->setExpanded(true);
            categories.insert(binding.category, category);
        }

        auto *item = new QTreeWidgetItem(category, QStringList{binding.label, binding.shortcut});
        item->setData(kActionColumn, kActionIdRole, binding.action_id);
        if (!binding.is_default) {
            QFont font = item->font(kShortcutColumn);
            font.setBold(true);
            item->setFont(kShortcutColumn, font);
        }
        if (binding.action_id == selectedActionId) {
            tree->setCurrentItem(item);
        }
    }
    tree->expandAll();
}

// The action id of the selected row, empty when a category header (or
// nothing) is selected.
QString selectedActionId(const QTreeWidget *tree)
{
    const QTreeWidgetItem *item = tree->currentItem();
    return item ? item->data(kActionColumn, kActionIdRole).toString() : QString();
}

} // namespace

QWidget *buildKeymapPage(QWidget *parent, KeymapEditor *editor)
{
    auto *page = new QWidget(parent);
    // Tall enough for the whole catalog: a keymap table you have to scroll to
    // see is the one place a settings dialog shouldn't save space.
    page->setMinimumSize(520, 420);
    auto *layout = new QVBoxLayout(page);

    auto *tree = new QTreeWidget(page);
    tree->setColumnCount(2);
    tree->setHeaderLabels({QObject::tr("Action"), QObject::tr("Shortcut")});
    tree->header()->setSectionResizeMode(kActionColumn, QHeaderView::Stretch);
    tree->header()->setSectionResizeMode(kShortcutColumn, QHeaderView::ResizeToContents);
    tree->setRootIsDecorated(false);
    tree->setIndentation(12);
    layout->addWidget(tree, 1);

    // Nothing is selected until the user picks a row, so there is nothing to
    // assign to yet.
    auto *shortcutEdit = new QKeySequenceEdit(page);
    shortcutEdit->setEnabled(false);

    // A menu accelerator is a single combination, but QKeySequenceEdit keeps
    // recording up to four of them ("Ctrl+Shift+F, ..."). Cutting it back to
    // the first stroke as soon as one arrives also ends the recording, so the
    // field shows the final binding immediately.
    QObject::connect(shortcutEdit, &QKeySequenceEdit::keySequenceChanged, page,
                      [shortcutEdit](const QKeySequence &sequence) {
                          if (sequence.count() > 1) {
                              shortcutEdit->setKeySequence(QKeySequence(sequence[0]));
                          }
                      });

    auto *assignButton = new QPushButton(QObject::tr("Assign"), page);
    auto *clearButton = new QPushButton(QObject::tr("Clear"), page);
    auto *resetButton = new QPushButton(QObject::tr("Reset All"), page);

    auto *controls = new QHBoxLayout();
    controls->addWidget(new QLabel(QObject::tr("Shortcut:"), page));
    controls->addWidget(shortcutEdit, 1);
    controls->addWidget(assignButton);
    controls->addWidget(clearButton);
    controls->addWidget(resetButton);
    layout->addLayout(controls);

    populate(tree, editor, QString());

    // Selecting a row seeds the editor with that action's current binding,
    // so "Assign" without touching the key editor is a no-op rather than an
    // accidental unbind.
    QObject::connect(tree, &QTreeWidget::currentItemChanged, page,
                      [tree, shortcutEdit](QTreeWidgetItem *current) {
                          const bool isAction =
                            current && !current->data(kActionColumn, kActionIdRole).toString().isEmpty();
                          shortcutEdit->setEnabled(isAction);
                          shortcutEdit->setKeySequence(
                            isAction ? QKeySequence(current->text(kShortcutColumn),
                                                     QKeySequence::PortableText)
                                     : QKeySequence());
                      });

    QObject::connect(assignButton, &QPushButton::clicked, page,
                      [page, tree, shortcutEdit, editor]() {
                          const QString actionId = selectedActionId(tree);
                          if (actionId.isEmpty()) {
                              return;
                          }
                          const QString shortcut =
                            shortcutEdit->keySequence().toString(QKeySequence::PortableText);
                          const QStringList conflicts = editor->conflicts(actionId, shortcut);
                          if (!conflicts.isEmpty()) {
                              const auto answer = QMessageBox::warning(
                                page, QObject::tr("Shortcut Already Assigned"),
                                QObject::tr("%1 is already assigned to:\n\n%2\n\nAssign it here "
                                            "and leave those unbound?")
                                  .arg(shortcut, conflicts.join(QStringLiteral("\n"))),
                                QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel);
                              if (answer != QMessageBox::Yes) {
                                  return;
                              }
                          }
                          editor->assign(actionId, shortcut);
                          populate(tree, editor, actionId);
                      });

    QObject::connect(clearButton, &QPushButton::clicked, page, [tree, editor]() {
        const QString actionId = selectedActionId(tree);
        if (actionId.isEmpty()) {
            return;
        }
        editor->assign(actionId, QString());
        populate(tree, editor, actionId);
    });

    QObject::connect(resetButton, &QPushButton::clicked, page, [page, tree, editor]() {
        const auto answer = QMessageBox::question(
          page, QObject::tr("Reset Keymap"),
          QObject::tr("Restore every action's default shortcut?"),
          QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel);
        if (answer != QMessageBox::Yes) {
            return;
        }
        editor->resetDefaults();
        populate(tree, editor, selectedActionId(tree));
    });

    return page;
}

QAction *registerAction(QMenu *menu, const QString &id, const QString &text,
                        AppSettings *appSettings, QHash<QString, QAction *> &actions)
{
    QAction *action = menu->addAction(text);
    action->setShortcut(QKeySequence(appSettings->shortcutFor(id), QKeySequence::PortableText));
    actions.insert(id, action);
    return action;
}

QAction *scopeShortcut(QAction *action, QWidget *owner, const QString &id,
                       AppSettings *appSettings, ShortcutReach reach)
{
    action->setShortcut(scopedShortcutKey(id, appSettings));
    action->setShortcutContext(reach == ShortcutReach::Owner ? Qt::WidgetShortcut
                                                              : Qt::WidgetWithChildrenShortcut);
    owner->addAction(action);
    return action;
}

QKeySequence scopedShortcutKey(const QString &id, AppSettings *appSettings)
{
    return QKeySequence(appSettings->shortcutFor(id), QKeySequence::PortableText);
}

void applyKeymap(const QHash<QString, QAction *> &actions, AppSettings *appSettings)
{
    for (auto it = actions.constBegin(); it != actions.constEnd(); ++it) {
        it.value()->setShortcut(
          QKeySequence(appSettings->shortcutFor(it.key()), QKeySequence::PortableText));
    }
}

} // namespace ui_shell
