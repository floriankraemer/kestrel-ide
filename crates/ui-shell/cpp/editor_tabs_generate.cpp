// Alt+Insert (ADR-0072): the Generate menu and its member picker. What can be
// generated, for which properties and as what code is `EditorOps`'; the
// servers' source/refactor actions are the intentions `LanguageService`
// already merges across servers.

#include "editor_tabs.h"
#include "code_editor.h"
#include "e2e_mark.h"

#include <QAction>
#include <QDialog>
#include <QDialogButtonBox>
#include <QHash>
#include <QListWidget>
#include <QMainWindow>
#include <QMenu>
#include <QStatusBar>
#include <QTimer>
#include <QVBoxLayout>
#include <QVariant>

namespace ui_shell {

namespace {

// How long Alt+Insert waits for the language servers before showing the local
// generators alone — a server that is slow or absent must not hold them back.
constexpr int kServerWaitMs = 300;

} // namespace

void EditorTabs::showStatusNotice(const QString &message)
{
    if (message.isEmpty()) {
        return;
    }
    if (auto *main = qobject_cast<QMainWindow *>(window_)) {
        main->statusBar()->showMessage(message, 10000);
    }
}

void EditorTabs::showGenerateNow()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    generatePending_ = true;
    const quint64 token = ++generateToken_;
    requestIntentionsFor(editor, false);
    QTimer::singleShot(kServerWaitMs, window_, [this, token]() {
        if (generatePending_ && token == generateToken_) {
            generatePending_ = false;
            showGenerateMenu(false);
        }
    });
}

void EditorTabs::showGenerateMenu(bool withServerActions)
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    QMenu menu(window_);
    menu.setObjectName(QStringLiteral("generateMenu"));

    QHash<QAction *, FfiGenerateKind> generators;
    QHash<QAction *, QString> titles;
    for (const FfiGenerateOption &option : editorOps_->generateOptions(tabId, editor->toPlainText())) {
        const QString title = option.title;
        const QString reason = option.reason;
        QAction *entry =
          menu.addAction(reason.isEmpty() ? title : tr("%1 — %2").arg(title, reason));
        entry->setEnabled(option.enabled);
        generators.insert(entry, option.kind);
        titles.insert(entry, title);
    }

    QHash<QAction *, quint32> serverActions;
    if (withServerActions) {
        const ::rust::Vec<FfiIntention> items = languageService_->intentions();
        bool separated = menu.isEmpty();
        for (std::size_t i = 0; i < items.size(); ++i) {
            if (items[i].group != FfiIntentionGroup::Source
                && items[i].group != FfiIntentionGroup::Refactor) {
                continue;
            }
            if (!separated) {
                menu.addSeparator();
                separated = true;
            }
            const QString reason = items[i].disabled_reason;
            QAction *entry = menu.addAction(
              reason.isEmpty() ? QString(items[i].title)
                               : tr("%1 — %2").arg(QString(items[i].title), reason));
            entry->setEnabled(reason.isEmpty());
            serverActions.insert(entry, static_cast<quint32>(i));
        }
    }

    if (menu.isEmpty()) {
        showStatusNotice(tr("Nothing to generate here."));
        return;
    }
    // A popup menu holds a keyboard grab, so these marks are the only way a
    // flow can tell it is up and where its entries are.
    e2eMarkMenuActions(&menu, "generate_menu_action");
    e2eMark("{\"ev\":\"dialog_shown\",\"name\":\"generate_menu\"}");
    QAction *chosen = menu.exec(editor->mapToGlobal(editor->cursorRect().bottomLeft()));
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"generate_menu\","
                            "\"accepted\":%1}")
              .arg(chosen != nullptr ? QLatin1String("true") : QLatin1String("false")));
    if (chosen == nullptr) {
        return;
    }
    if (generators.contains(chosen)) {
        runGenerator(editor, generators.value(chosen), titles.value(chosen));
    } else if (serverActions.contains(chosen)) {
        languageService_->applyIntention(serverActions.value(chosen), documentRevision());
    }
}

void EditorTabs::runGenerator(CodeEditor *editor, FfiGenerateKind kind, const QString &title)
{
    const quint64 tabId = editor->property("tabId").toULongLong();
    const QString text = editor->toPlainText();
    const ::rust::Vec<FfiGenerateMember> members = editorOps_->generateMembers(tabId, text, kind);
    if (members.empty()) {
        return;
    }

    QDialog dialog(window_);
    dialog.setWindowTitle(title);
    auto *layout = new QVBoxLayout(&dialog);
    auto *list = new QListWidget(&dialog);
    for (const FfiGenerateMember &member : members) {
        auto *item = new QListWidgetItem(QString(member.label), list);
        item->setData(Qt::UserRole, QString(member.name));
        item->setFlags(item->flags() | Qt::ItemIsUserCheckable);
        item->setCheckState(Qt::Checked);
    }
    layout->addWidget(list);
    auto *buttons =
      new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    layout->addWidget(buttons);
    // `window` is the X window id: with no window manager under Xvfb a new
    // toplevel is not focused, so a flow focuses it before sending a key.
    QStringList labels;
    for (int row = 0; row < list->count(); ++row) {
        labels << e2eJson(list->item(row)->text());
    }
    QTimer::singleShot(0, &dialog, [&dialog, labels]() {
        e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"generate_members\","
                                "\"window\":\"%1\",\"items\":[%2]}")
                  .arg(dialog.winId())
                  .arg(labels.join(QLatin1Char(','))));
    });
    const bool accepted = dialog.exec() == QDialog::Accepted;
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"generate_members\","
                            "\"accepted\":%1}")
              .arg(accepted ? QLatin1String("true") : QLatin1String("false")));
    if (!accepted) {
        return;
    }

    ::rust::Vec<FfiGenerateMember> selected;
    for (int row = 0; row < list->count(); ++row) {
        const QListWidgetItem *item = list->item(row);
        if (item->checkState() == Qt::Checked) {
            selected.push_back(FfiGenerateMember{item->data(Qt::UserRole).toString(), QString()});
        }
    }
    const ::rust::Vec<FfiTextEdit> edits =
      editorOps_->generateCode(tabId, text, kind, std::move(selected));
    if (!edits.empty()) {
        applyEditsTo(editor, edits);
    }
    refreshCarets(editor);
}

} // namespace ui_shell
