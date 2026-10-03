#include "language_servers_page.h"

#include "theme.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QHBoxLayout>
#include <QHash>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QSignalBlocker>
#include <QStringList>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QVBoxLayout>
#include <QWidget>

#include <memory>

namespace ui_shell {

namespace {

constexpr int kOnColumn = 0;
constexpr int kLanguageColumn = 1;
constexpr int kCommandColumn = 2;
constexpr int kStatusColumn = 3;
constexpr int kServerIdRole = Qt::UserRole;

// What the manager last said about one language's server. Held per row so a
// state change repaints only that row, and so the status survives a rebuild
// of the tree.
struct LiveState
{
    FfiServerState state = FfiServerState::Failed;
    QString name;
    QString detail;
    quint32 retryMs = 0;
    bool known = false;
};

using LiveStates = QHash<QString, LiveState>;

// What the manager has said so far this session. The settings dialog is
// rebuilt on every open, and a server's state (Phpactor skipped on native
// Windows, Intelephense not found) is announced when a file of its language
// opens, long before the page does; each page therefore starts from this
// record instead of an empty one.
std::shared_ptr<LiveStates> sessionLiveStates()
{
    static const auto states = std::make_shared<LiveStates>();
    return states;
}

QString liveStatusText(const LiveState &live)
{
    switch (live.state) {
    case FfiServerState::Starting:
        return QObject::tr("Starting");
    case FfiServerState::Ready:
        return QObject::tr("Running");
    case FfiServerState::Exited:
        return QObject::tr("Crashed, retrying");
    case FfiServerState::Unavailable:
        return QObject::tr("Unavailable");
    case FfiServerState::NotFound:
        return QObject::tr("Command not found");
    case FfiServerState::Failed:
        return QObject::tr("Stopped");
    }
    return QString();
}

QColor liveStatusColor(const LiveState &live)
{
    const SemanticColors colors = semanticColors();
    switch (live.state) {
    case FfiServerState::Starting:
        return QColor();
    case FfiServerState::Ready:
        return colors.ok;
    case FfiServerState::Exited:
    case FfiServerState::Unavailable:
        return colors.warning;
    case FfiServerState::Failed:
    case FfiServerState::NotFound:
        return colors.error;
    }
    return QColor();
}

// The two-line detail strip: what happened, then the one thing to check.
QString detailLines(const LiveState &live)
{
    const QString name = live.name.isEmpty() ? QObject::tr("The server") : live.name;
    switch (live.state) {
    case FfiServerState::Exited:
        return QObject::tr("%1 exited and will be restarted in %2 seconds.\nIts output is in "
                           "the log.")
          .arg(name)
          .arg((live.retryMs + 999) / 1000);
    case FfiServerState::NotFound:
        return QObject::tr("%1: command not found.\n%2").arg(name, live.detail);
    case FfiServerState::Failed:
        return QObject::tr("%1 stopped: %2\nFix the command, then press Restart Server.")
          .arg(name, live.detail.isEmpty() ? QObject::tr("no further detail.") : live.detail);
    case FfiServerState::Unavailable:
        return live.detail;
    case FfiServerState::Starting:
    case FfiServerState::Ready:
        break;
    }
    // The strip occupies no vertical space when the selected row is healthy.
    return QString();
}

QString staticStatusText(const FfiLanguageServerRow &row)
{
    switch (row.status) {
    case FfiServerRowStatus::NotConfigured:
        return QObject::tr("Not configured");
    case FfiServerRowStatus::Disabled:
        return QObject::tr("Disabled");
    case FfiServerRowStatus::Enabled:
        break;
    }
    return QString();
}

QString selectedServerId(const QTreeWidget *tree)
{
    const QTreeWidgetItem *item = tree->currentItem();
    return item ? item->data(kLanguageColumn, kServerIdRole).toString() : QString();
}

void paintStatus(QTreeWidgetItem *item, const FfiLanguageServerRow &row, const LiveStates &live,
                 bool dirty)
{
    const SemanticColors colors = semanticColors();
    QString text = staticStatusText(row);
    QColor color = colors.muted;

    const auto known = live.constFind(row.id);
    if (row.status != FfiServerRowStatus::NotConfigured && known != live.constEnd()
        && known->known) {
        text = liveStatusText(*known);
        color = liveStatusColor(*known);
    }
    if (!text.isEmpty() && dirty) {
        // The Status column reports the running world, not the draft; the
        // suffix is what keeps the user from reading it as their new command.
        text = QObject::tr("%1 (pending)").arg(text);
    }
    item->setText(kStatusColumn, text);
    if (color.isValid()) {
        item->setForeground(kStatusColumn, color);
    }
}

} // namespace

void trackLanguageServerStates(LanguageService *languageService)
{
    QObject::connect(languageService, &LanguageService::serverStateChanged, languageService,
                     [live = sessionLiveStates()](const QString &serverId, const QString &name,
                                                  FfiServerState state, const QString &detail,
                                                  quint32 retryMs) {
                         (*live)[serverId] = LiveState{state, name, detail, retryMs, true};
                     });
}

QWidget *buildLanguageServersPage(QWidget *parent,
                                  LanguageServerEditor *editor,
                                  LanguageService *languageService)
{
    auto *page = new QWidget(parent);
    page->setMinimumSize(560, 460);
    auto *layout = new QVBoxLayout(page);

    auto *tree = new QTreeWidget(page);
    tree->setColumnCount(4);
    tree->setHeaderLabels(
      {QObject::tr("On"), QObject::tr("Language"), QObject::tr("Command"), QObject::tr("Status")});
    tree->header()->setSectionResizeMode(kOnColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kLanguageColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kCommandColumn, QHeaderView::Stretch);
    tree->header()->setSectionResizeMode(kStatusColumn, QHeaderView::ResizeToContents);
    // Flat: the natural grouping key would be status, and status changes
    // while the user watches, which would make rows jump between groups.
    tree->setRootIsDecorated(false);
    tree->setIndentation(0);
    layout->addWidget(tree, 1);

    auto *commandEdit = new QLineEdit(page);
    auto *argsEdit = new QLineEdit(page);
    auto *enabledCheck = new QCheckBox(QObject::tr("Enabled"), page);
    auto *restartButton = new QPushButton(QObject::tr("Restart Server"), page);

    auto *commandRow = new QHBoxLayout();
    commandRow->addWidget(new QLabel(QObject::tr("Command:"), page));
    commandRow->addWidget(commandEdit, 1);
    auto *argsRow = new QHBoxLayout();
    argsRow->addWidget(new QLabel(QObject::tr("Arguments:"), page));
    argsRow->addWidget(argsEdit, 1);
    auto *buttonRow = new QHBoxLayout();
    buttonRow->addWidget(enabledCheck);
    buttonRow->addStretch(1);
    buttonRow->addWidget(restartButton);
    layout->addLayout(commandRow);
    layout->addLayout(argsRow);
    layout->addLayout(buttonRow);

    auto *detailLabel = new QLabel(page);
    detailLabel->setWordWrap(true);
    detailLabel->setVisible(false);
    layout->addWidget(detailLabel);

    auto live = sessionLiveStates();

    auto rowFor = [editor](const QString &serverId) {
        for (const FfiLanguageServerRow &row : editor->rows()) {
            if (row.id == serverId) {
                return row;
            }
        }
        return FfiLanguageServerRow{};
    };

    auto repaint = [=](const QString &keepId) {
        const QSignalBlocker blocker(tree);
        tree->clear();
        for (const FfiLanguageServerRow &row : editor->rows()) {
            auto *item = new QTreeWidgetItem(
              tree, QStringList{QString(), row.label, row.command, QString()});
            item->setData(kLanguageColumn, kServerIdRole, row.id);
            // A row with no command has no checkbox at all rather than an
            // unchecked one: there is nothing to switch on yet.
            if (row.status != FfiServerRowStatus::NotConfigured) {
                item->setCheckState(kOnColumn, row.enabled ? Qt::Checked : Qt::Unchecked);
            }
            paintStatus(item, row, *live, editor->isDirty(row.id));
            if (row.id == keepId) {
                tree->setCurrentItem(item);
            }
        }
    };

    auto showDetail = [=](const QString &serverId) {
        const auto known = live->constFind(serverId);
        const QString text =
          known == live->constEnd() || !known->known ? QString() : detailLines(*known);
        detailLabel->setText(text);
        detailLabel->setStyleSheet(
          QStringLiteral("color: %1;").arg(semanticColors().error.name()));
        detailLabel->setVisible(!text.isEmpty());
    };

    // Writing the form into the draft before anything else can read it is
    // what keeps navigating away from silently discarding typing.
    auto commitForm = [=](const QString &serverId) {
        if (serverId.isEmpty()) {
            return;
        }
        editor->setCommand(serverId, commandEdit->text());
        editor->setArgs(serverId, argsEdit->text());
    };

    auto loadForm = [=](const QString &serverId) {
        const FfiLanguageServerRow row = rowFor(serverId);
        const QSignalBlocker commandBlocker(commandEdit);
        const QSignalBlocker argsBlocker(argsEdit);
        const QSignalBlocker enabledBlocker(enabledCheck);
        commandEdit->setText(row.command);
        argsEdit->setText(row.args);
        enabledCheck->setChecked(row.enabled);
        const bool selected = !serverId.isEmpty();
        commandEdit->setEnabled(selected);
        argsEdit->setEnabled(selected);
        enabledCheck->setEnabled(selected && !row.command.isEmpty());
        // An action, not a setting: it restarts what is running, so it
        // refuses while the row holds an edit that has not been committed.
        restartButton->setEnabled(selected && !row.command.isEmpty()
                                  && !editor->isDirty(serverId));
        showDetail(serverId);
    };

    // The selected row is the page's only state; `previous` is what the form
    // is currently editing, so moving away can flush it first.
    auto previous = std::make_shared<QString>();

    QObject::connect(tree, &QTreeWidget::currentItemChanged, page, [=]() {
        const QString serverId = selectedServerId(tree);
        if (*previous != serverId) {
            commitForm(*previous);
            *previous = serverId;
        }
        loadForm(serverId);
    });

    QObject::connect(tree, &QTreeWidget::itemChanged, page, [=](QTreeWidgetItem *item) {
        const QString serverId = item->data(kLanguageColumn, kServerIdRole).toString();
        if (serverId.isEmpty()) {
            return;
        }
        editor->setEnabled(serverId, item->checkState(kOnColumn) == Qt::Checked);
        repaint(selectedServerId(tree));
        loadForm(selectedServerId(tree));
    });

    auto applyForm = [=]() {
        const QString serverId = selectedServerId(tree);
        commitForm(serverId);
        repaint(serverId);
        loadForm(serverId);
    };
    // Enter commits the field and hands focus back to the tree, the fast
    // path for configuring several servers in a row.
    QObject::connect(commandEdit, &QLineEdit::returnPressed, page, [=]() {
        applyForm();
        tree->setFocus();
    });
    QObject::connect(argsEdit, &QLineEdit::returnPressed, page, [=]() {
        applyForm();
        tree->setFocus();
    });
    QObject::connect(commandEdit, &QLineEdit::editingFinished, page, applyForm);
    QObject::connect(argsEdit, &QLineEdit::editingFinished, page, applyForm);
    QObject::connect(enabledCheck, &QCheckBox::toggled, page, [=](bool checked) {
        const QString serverId = selectedServerId(tree);
        if (serverId.isEmpty()) {
            return;
        }
        editor->setEnabled(serverId, checked);
        repaint(serverId);
    });

    QObject::connect(restartButton, &QPushButton::clicked, page, [=]() {
        const QString serverId = selectedServerId(tree);
        if (!serverId.isEmpty()) {
            languageService->restartServer(serverId);
        }
    });

    // Status is live even while the draft is dirty, because it reports the
    // running world rather than the draft.
    QObject::connect(languageService, &LanguageService::serverStateChanged, page,
                      [=](const QString &serverId, const QString &name, FfiServerState state,
                          const QString &detail, quint32 retryMs) {
                          (*live)[serverId] =
                            LiveState{state, name, detail, retryMs, true};
                          repaint(selectedServerId(tree));
                          showDetail(selectedServerId(tree));
                      });

    repaint(QString());
    loadForm(QString());
    return page;
}

} // namespace ui_shell
