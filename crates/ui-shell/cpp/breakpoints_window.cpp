#include "breakpoints_window.h"

#include "e2e_mark.h"
#include <QObject>

#include <QCheckBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QLineEdit>
#include <QListWidget>
#include <QPoint>
#include <QPushButton>
#include <QRect>
#include <QSignalBlocker>
#include <QSplitter>
#include <QTimer>
#include <QVBoxLayout>

#include <memory>

namespace ui_shell {

namespace {

QString rowLabel(const FfiBreakpoint &breakpoint)
{
    return QStringLiteral("%1:%2").arg(QString(breakpoint.path)).arg(breakpoint.line);
}

} // namespace

void showBreakpointsWindow(QWidget *parent, DebugService *debugService,
                            std::function<void(const QString &, int, int)> openAt)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Breakpoints"));
    dialog.resize(640, 380);
    QObject::connect(&dialog, &QDialog::finished, &dialog, [](int) {
        e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"breakpoints_window\"}");
    });

    // The model this window edits: reloaded after every change, so Remove
    // and every field edit stay consistent with what `DebugService` now
    // holds rather than a copy that could drift from it.
    auto rows = std::make_shared<::rust::Vec<FfiBreakpoint>>();

    auto *list = new QListWidget(&dialog);
    auto *removeButton = new QPushButton(QObject::tr("Remove"), &dialog);
    removeButton->setEnabled(false);
    auto *listColumn = new QVBoxLayout();
    listColumn->addWidget(list, 1);
    listColumn->addWidget(removeButton);
    auto *listWidget = new QWidget(&dialog);
    listWidget->setLayout(listColumn);

    auto *enabledCheck = new QCheckBox(QObject::tr("Enabled"), &dialog);
    auto *temporaryCheck = new QCheckBox(QObject::tr("Remove once hit (temporary)"), &dialog);
    auto *conditionEdit = new QLineEdit(&dialog);
    auto *hitConditionEdit = new QLineEdit(&dialog);
    auto *logMessageEdit = new QLineEdit(&dialog);
    for (QWidget *field : {static_cast<QWidget *>(enabledCheck),
                            static_cast<QWidget *>(temporaryCheck),
                            static_cast<QWidget *>(conditionEdit),
                            static_cast<QWidget *>(hitConditionEdit),
                            static_cast<QWidget *>(logMessageEdit)}) {
        field->setEnabled(false);
    }
    auto *form = new QFormLayout();
    form->addRow(enabledCheck);
    form->addRow(QObject::tr("Condition:"), conditionEdit);
    form->addRow(QObject::tr("Hit count:"), hitConditionEdit);
    form->addRow(QObject::tr("Log message:"), logMessageEdit);
    form->addRow(temporaryCheck);
    auto *detailWidget = new QWidget(&dialog);
    detailWidget->setLayout(form);

    auto *splitter = new QSplitter(Qt::Horizontal, &dialog);
    splitter->addWidget(listWidget);
    splitter->addWidget(detailWidget);
    splitter->setStretchFactor(0, 1);
    splitter->setStretchFactor(1, 2);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Close, &dialog);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);

    auto *layout = new QVBoxLayout(&dialog);
    layout->addWidget(splitter, 1);
    layout->addWidget(buttons);

    // Reload the list from `DebugService`, keeping `row` current if it is
    // still in range — Remove and an edit both call this rather than
    // patching the widgets by hand.
    auto reload = [debugService, rows, list](int keepRow) {
        *rows = debugService->allBreakpoints();
        const QSignalBlocker blocker(list);
        list->clear();
        for (const FfiBreakpoint &breakpoint : *rows) {
            list->addItem(rowLabel(breakpoint));
        }
        if (keepRow >= 0 && keepRow < list->count()) {
            list->setCurrentRow(keepRow);
        }
    };

    auto currentBreakpoint = [rows, list]() -> const FfiBreakpoint * {
        const int row = list->currentRow();
        if (row < 0 || static_cast<size_t>(row) >= rows->size()) {
            return nullptr;
        }
        return &(*rows)[static_cast<size_t>(row)];
    };

    auto applyCurrent = [debugService, currentBreakpoint, enabledCheck, conditionEdit,
                          hitConditionEdit, logMessageEdit, temporaryCheck]() {
        const FfiBreakpoint *breakpoint = currentBreakpoint();
        if (!breakpoint) {
            return;
        }
        FfiBreakpoint updated{};
        updated.path = breakpoint->path;
        updated.line = breakpoint->line;
        updated.enabled = enabledCheck->isChecked();
        updated.condition = conditionEdit->text();
        updated.hit_condition = hitConditionEdit->text();
        updated.log_message = logMessageEdit->text();
        updated.temporary = temporaryCheck->isChecked();
        debugService->configureBreakpoint(updated);
    };

    QObject::connect(list, &QListWidget::currentRowChanged, &dialog,
            [currentBreakpoint, removeButton, enabledCheck, temporaryCheck, conditionEdit,
             hitConditionEdit, logMessageEdit](int) {
                const FfiBreakpoint *breakpoint = currentBreakpoint();
                const bool has = breakpoint != nullptr;
                removeButton->setEnabled(has);
                for (QWidget *field : {static_cast<QWidget *>(enabledCheck),
                                        static_cast<QWidget *>(temporaryCheck),
                                        static_cast<QWidget *>(conditionEdit),
                                        static_cast<QWidget *>(hitConditionEdit),
                                        static_cast<QWidget *>(logMessageEdit)}) {
                    field->setEnabled(has);
                }
                if (!has) {
                    return;
                }
                const QSignalBlocker a(enabledCheck);
                const QSignalBlocker b(temporaryCheck);
                const QSignalBlocker c(conditionEdit);
                const QSignalBlocker d(hitConditionEdit);
                const QSignalBlocker e(logMessageEdit);
                enabledCheck->setChecked(breakpoint->enabled);
                temporaryCheck->setChecked(breakpoint->temporary);
                conditionEdit->setText(breakpoint->condition);
                hitConditionEdit->setText(breakpoint->hit_condition);
                logMessageEdit->setText(breakpoint->log_message);
            });

    QObject::connect(enabledCheck, &QCheckBox::toggled, &dialog, applyCurrent);
    QObject::connect(temporaryCheck, &QCheckBox::toggled, &dialog, applyCurrent);
    QObject::connect(conditionEdit, &QLineEdit::editingFinished, &dialog, applyCurrent);
    QObject::connect(hitConditionEdit, &QLineEdit::editingFinished, &dialog, applyCurrent);
    QObject::connect(logMessageEdit, &QLineEdit::editingFinished, &dialog, applyCurrent);

    QObject::connect(removeButton, &QPushButton::clicked, &dialog, [debugService, currentBreakpoint,
                                                             reload]() {
        const FfiBreakpoint *breakpoint = currentBreakpoint();
        if (!breakpoint) {
            return;
        }
        // A breakpoint always exists at this point, so toggling it is
        // exactly a removal — the same call the gutter's own toggle makes.
        debugService->toggleBreakpoint(breakpoint->path, breakpoint->line);
        reload(-1);
    });

    QObject::connect(list, &QListWidget::itemDoubleClicked, &dialog, [currentBreakpoint, openAt]() {
        if (const FfiBreakpoint *breakpoint = currentBreakpoint()) {
            openAt(breakpoint->path, static_cast<int>(breakpoint->line), 1);
        }
    });

    reload(0);
    // Same reasoning as `showRunConfigDialog`'s own timer: `exec()` is what
    // actually grants this dialog X input focus, so the mark — and the
    // rects an E2E flow clicks rather than guesses a tab order through —
    // fire from inside the modal loop, not before it.
    QTimer::singleShot(0, &dialog, [conditionEdit]() {
        const auto rectJson = [](const QRect &rect) {
            return QStringLiteral("[%1,%2,%3,%4]")
              .arg(rect.x())
              .arg(rect.y())
              .arg(rect.width())
              .arg(rect.height());
        };
        const QRect conditionRect(conditionEdit->mapToGlobal(QPoint(0, 0)),
                                   conditionEdit->size());
        e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"breakpoints_window\","
                                "\"condition_rect\":%1}")
                  .arg(rectJson(conditionRect)));
    });
    dialog.exec();
}

} // namespace ui_shell
