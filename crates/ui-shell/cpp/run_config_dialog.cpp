#include "run_config_dialog.h"

#include "e2e_mark.h"
#include "run_config_container_pages.h"

#include <QAction>
#include <QCheckBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QListWidget>
#include <QMenu>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QPoint>
#include <QPushButton>
#include <QRect>
#include <QSignalBlocker>
#include <QTimer>
#include <QToolButton>
#include <QVBoxLayout>

#include <memory>

namespace ui_shell {

namespace {

int configCount(RunConfigEditor *editor)
{
    return static_cast<int>(editor->configurations().size());
}

FfiRunConfig configAt(RunConfigEditor *editor, int index)
{
    int i = 0;
    for (const FfiRunConfig &config : editor->configurations()) {
        if (i++ == index) {
            return config;
        }
    }
    return FfiRunConfig{};
}

int indexOfId(RunConfigEditor *editor, const QString &id)
{
    int i = 0;
    for (const FfiRunConfig &config : editor->configurations()) {
        if (QString(config.id) == id) {
            return i;
        }
        ++i;
    }
    return -1;
}

// The starter options a freshly added container-kind configuration gets —
// just enough for `container_core::run_config` to compile something
// sensible; the rest is left to the user to fill in through the page's own
// fields (C5, ADR-0056).
FfiContainerOptions defaultContainerOptions(const QString &kind)
{
    FfiContainerOptions options;
    if (kind == QLatin1String("containerfile")) {
        options.dockerfile = QStringLiteral("Dockerfile");
        options.context_dir = QStringLiteral("$PROJECT_DIR$");
        options.run_built_image = true;
    } else if (kind == QLatin1String("compose")) {
        options.compose_files = QStringLiteral("docker-compose.yml");
    }
    return options;
}

void repaintList(QListWidget *list, RunConfigEditor *editor, int keepIndex)
{
    const QSignalBlocker blocker(list);
    list->clear();
    for (const FfiRunConfig &config : editor->configurations()) {
        list->addItem(config.name.isEmpty() ? QObject::tr("(unnamed)") : config.name);
    }
    if (keepIndex >= 0 && keepIndex < list->count()) {
        list->setCurrentRow(keepIndex);
    }
}

} // namespace

void showRunConfigDialog(QWidget *parent, RunConfigEditor *editor,
                         ContainerService *containerService, const QString &selectConfigId)
{
    editor->beginEdit();

    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Run Configurations"));
    dialog.resize(760, 520);

    auto *list = new QListWidget(&dialog);
    list->setMaximumWidth(200);
    // A plain "Add" button plus a Containers submenu (C5, ADR-0056): the
    // default action still adds a plain process configuration with one
    // click, exactly as before.
    auto *addButton = new QToolButton(&dialog);
    addButton->setText(QObject::tr("Add"));
    addButton->setPopupMode(QToolButton::MenuButtonPopup);
    auto *addMenu = new QMenu(addButton);
    QAction *addProcessAction = addMenu->addAction(QObject::tr("Process"));
    QMenu *containersMenu = addMenu->addMenu(QObject::tr("Containers"));
    QAction *addImageAction = containersMenu->addAction(QObject::tr("Container Image"));
    QAction *addContainerfileAction = containersMenu->addAction(QObject::tr("Containerfile"));
    QAction *addComposeAction = containersMenu->addAction(QObject::tr("Compose"));
    addButton->setMenu(addMenu);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), &dialog);
    auto *listButtons = new QHBoxLayout();
    listButtons->addWidget(addButton);
    listButtons->addWidget(removeButton);
    auto *listColumn = new QVBoxLayout();
    listColumn->addWidget(list, 1);
    listColumn->addLayout(listButtons);

    auto *nameEdit = new QLineEdit(&dialog);
    auto *programEdit = new QLineEdit(&dialog);
    auto *argsEdit = new QLineEdit(&dialog);
    auto *cwdEdit = new QLineEdit(&dialog);
    auto *envEdit = new QPlainTextEdit(&dialog);
    envEdit->setPlaceholderText(QObject::tr("KEY=VALUE, one per line"));
    auto *beforeLaunchEdit = new QPlainTextEdit(&dialog);
    beforeLaunchEdit->setPlaceholderText(
      QObject::tr("One per line: build | run <configuration id> | tool <program> [args]"));
    beforeLaunchEdit->setMaximumHeight(70);
    auto *parallelCheck =
      new QCheckBox(QObject::tr("Allow parallel run"), &dialog);
    parallelCheck->setToolTip(
      QObject::tr("Run this configuration again without stopping the running one"));

    auto *containerPage = new ContainerOptionsPage(containerService, editor, &dialog);

    auto *commandPreviewEdit = new QPlainTextEdit(&dialog);
    commandPreviewEdit->setReadOnly(true);
    commandPreviewEdit->setMaximumHeight(50);

    auto *form = new QVBoxLayout();
    const auto addRow = [form, &dialog](const QString &label, QWidget *field) {
        auto *row = new QHBoxLayout();
        auto *labelWidget = new QLabel(label, &dialog);
        labelWidget->setMinimumWidth(90);
        row->addWidget(labelWidget);
        row->addWidget(field, 1);
        form->addLayout(row);
    };
    addRow(QObject::tr("Name:"), nameEdit);
    addRow(QObject::tr("Program:"), programEdit);
    addRow(QObject::tr("Arguments:"), argsEdit);
    addRow(QObject::tr("Working dir:"), cwdEdit);
    form->addWidget(new QLabel(QObject::tr("Before launch:"), &dialog));
    form->addWidget(beforeLaunchEdit);
    form->addWidget(parallelCheck);
    form->addWidget(new QLabel(QObject::tr("Environment:"), &dialog));
    form->addWidget(envEdit, 1);
    form->addWidget(containerPage, 1);
    form->addWidget(new QLabel(QObject::tr("Command preview:"), &dialog));
    form->addWidget(commandPreviewEdit);

    auto *columns = new QHBoxLayout();
    columns->addLayout(listColumn);
    columns->addLayout(form, 1);

    auto *buttons =
      new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel, &dialog);

    auto *layout = new QVBoxLayout(&dialog);
    layout->addLayout(columns, 1);
    layout->addWidget(buttons);

    // The selected row is the only state; `previousIndex` is what the form
    // is currently editing, so switching rows (or Save) flushes it into the
    // draft first — same discipline `language_servers_page.cpp`'s
    // `commitForm`/`loadForm` pair uses.
    auto previousIndex = std::make_shared<int>(-1);

    // What kind the currently-loaded row is — set only by `loadForm`/the Add
    // menu, read by `commitForm` so it never has to re-derive it from a
    // combo box that does not exist (kind is fixed at creation).
    auto currentKind = std::make_shared<QString>();

    const auto commitForm = [=]() {
        const int index = *previousIndex;
        if (index < 0) {
            return;
        }
        FfiRunConfig form{};
        form.name = nameEdit->text();
        form.program = programEdit->text();
        form.args = argsEdit->text();
        form.cwd = cwdEdit->text();
        form.env = envEdit->toPlainText();
        form.allow_parallel = parallelCheck->isChecked();
        form.before_launch = beforeLaunchEdit->toPlainText();
        form.kind = *currentKind;
        form.container = containerPage->options();
        editor->updateConfiguration(static_cast<quint32>(index), form);
    };

    const auto refreshPreview = [=]() {
        FfiRunConfig form{};
        form.name = nameEdit->text();
        form.program = programEdit->text();
        form.args = argsEdit->text();
        form.kind = *currentKind;
        form.container = containerPage->options();
        commandPreviewEdit->setPlainText(editor->commandPreview(form));
    };

    const auto loadForm = [=](int index) {
        const bool has = index >= 0;
        nameEdit->setEnabled(has);
        cwdEdit->setEnabled(has);
        envEdit->setEnabled(has);
        beforeLaunchEdit->setEnabled(has);
        parallelCheck->setEnabled(has);
        removeButton->setEnabled(has);
        const FfiRunConfig config = has ? configAt(editor, index) : FfiRunConfig{};
        nameEdit->setText(config.name);
        programEdit->setText(config.program);
        argsEdit->setText(config.args);
        cwdEdit->setText(config.cwd);
        envEdit->setPlainText(config.env);
        beforeLaunchEdit->setPlainText(config.before_launch);
        parallelCheck->setChecked(config.allow_parallel);
        *currentKind = config.kind;
        const bool isContainer = has && !config.kind.isEmpty();
        // Program/Arguments/Working dir are meaningless for a container-kind
        // configuration (its argv is compiled from the container page's own
        // fields, not these) — disabled rather than left editable and
        // ignored.
        programEdit->setEnabled(has && !isContainer);
        argsEdit->setEnabled(has && !isContainer);
        cwdEdit->setEnabled(has && !isContainer);
        containerPage->setKind(config.kind);
        containerPage->setOptions(config.container);
        refreshPreview();
    };

    QObject::connect(containerPage, &ContainerOptionsPage::changed, &dialog, refreshPreview);
    QObject::connect(programEdit, &QLineEdit::textChanged, &dialog, refreshPreview);
    QObject::connect(argsEdit, &QLineEdit::textChanged, &dialog, refreshPreview);

    QObject::connect(list, &QListWidget::currentRowChanged, &dialog, [=](int row) {
        commitForm();
        *previousIndex = row;
        loadForm(row);
    });

    const auto addWithKind = [=](const QString &kind) {
        commitForm();
        if (kind.isEmpty()) {
            editor->addConfiguration();
        } else {
            editor->addContainerConfiguration(QObject::tr("New Configuration"), kind,
                                              defaultContainerOptions(kind));
        }
        const int index = configCount(editor) - 1;
        repaintList(list, editor, index);
        // `repaintList`'s `QSignalBlocker` means its own `setCurrentRow`
        // above never fires `currentRowChanged` — so, unlike a row the user
        // clicks themselves, the form has to be pointed at the new row
        // explicitly, the same way the dialog's own initial setup below
        // does after its first `repaintList`. Without this the fields stay
        // on whatever they last showed (or blank and disabled, on the
        // dialog's first Add), which is what an E2E flow driving Add then
        // typing into Program actually caught.
        *previousIndex = list->currentRow();
        loadForm(list->currentRow());
        e2eMark(QStringLiteral("{\"ev\":\"run_config_added\",\"count\":%1}")
                  .arg(configCount(editor)));
    };
    QObject::connect(addButton, &QToolButton::clicked, &dialog,
                      [=]() { addWithKind(QString()); });
    QObject::connect(addProcessAction, &QAction::triggered, &dialog,
                      [=]() { addWithKind(QString()); });
    QObject::connect(addImageAction, &QAction::triggered, &dialog,
                      [=]() { addWithKind(QStringLiteral("container-image")); });
    QObject::connect(addContainerfileAction, &QAction::triggered, &dialog,
                      [=]() { addWithKind(QStringLiteral("containerfile")); });
    QObject::connect(addComposeAction, &QAction::triggered, &dialog,
                      [=]() { addWithKind(QStringLiteral("compose")); });

    QObject::connect(removeButton, &QPushButton::clicked, &dialog, [=]() {
        const int index = list->currentRow();
        if (index < 0) {
            return;
        }
        editor->removeConfiguration(static_cast<quint32>(index));
        // The row this index now names is a different configuration (or
        // none): committing the still-open form into it would overwrite the
        // wrong entry.
        *previousIndex = -1;
        repaintList(list, editor, qMin(index, configCount(editor) - 1));
        // Same reasoning as `addButton`'s handler: `repaintList` never fires
        // `currentRowChanged` on its own.
        *previousIndex = list->currentRow();
        loadForm(list->currentRow());
    });

    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, [=, &dialog]() {
        commitForm();
        const FfiResult refusal = editor->validate();
        if (refusal.code != 0) {
            QMessageBox::warning(&dialog, QObject::tr("Run Configurations"), refusal.message);
            return;
        }
        const FfiResult result = editor->commit();
        if (result.code != 0) {
            QMessageBox::warning(&dialog, QObject::tr("Run Configurations"), result.message);
            return;
        }
        dialog.accept();
    });
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, [editor, &dialog]() {
        editor->revert();
        dialog.reject();
    });

    const int initialIndex = selectConfigId.isEmpty()
      ? (configCount(editor) > 0 ? 0 : -1)
      : indexOfId(editor, selectConfigId);
    repaintList(list, editor, initialIndex);
    *previousIndex = list->currentRow();
    loadForm(list->currentRow());

    // A modal `exec()` blocks here until the dialog closes, so — unlike a
    // menu bar's `aboutToShow`/`aboutToHide` — `dialog_shown` has to be
    // marked once the dialog is actually up and `dialog_closed` from
    // `finished`, which fires whichever button (or Escape) closed it. Same
    // convention `refactor_preview_dialog.cpp` and
    // `search_everywhere_dialog.cpp` use.
    QObject::connect(&dialog, &QDialog::finished, &dialog, [](int result) {
        e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"run_config_dialog\","
                                "\"accepted\":%1}")
                  .arg(result == QDialog::Accepted ? QLatin1String("true")
                                                    : QLatin1String("false")));
    });

    // A zero-delay timer fired from inside `exec()`'s own modal loop, not a
    // `show()` called ahead of it: `exec()` is what actually grants this
    // dialog the (window-manager-free) X input focus an E2E flow's clicks
    // and typing depend on, and doing that ourselves first — tried and
    // reverted — left the dialog shown but never focused. The rects give
    // `dialog_shown` the same kind of ready-to-click geometry
    // `changes_panel_shown` carries — an E2E flow drives this dialog by
    // clicking Add and Save and typing into Program, not by guessing a
    // keyboard tab order through a form whose focus chain is an
    // implementation detail.
    QTimer::singleShot(0, &dialog, [=]() {
        const auto rectJson = [](const QRect &rect) {
            return QStringLiteral("[%1,%2,%3,%4]")
              .arg(rect.x())
              .arg(rect.y())
              .arg(rect.width())
              .arg(rect.height());
        };
        const QRect addRect(addButton->mapToGlobal(QPoint(0, 0)), addButton->size());
        const QRect programRect(programEdit->mapToGlobal(QPoint(0, 0)), programEdit->size());
        QPushButton *saveButton = buttons->button(QDialogButtonBox::Save);
        const QRect saveRect(saveButton->mapToGlobal(QPoint(0, 0)), saveButton->size());
        e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"run_config_dialog\","
                                "\"add_rect\":%1,\"program_rect\":%2,\"save_rect\":%3}")
                  .arg(rectJson(addRect))
                  .arg(rectJson(programRect))
                  .arg(rectJson(saveRect)));
    });
    dialog.exec();
}

} // namespace ui_shell
