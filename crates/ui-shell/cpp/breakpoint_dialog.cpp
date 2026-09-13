#include "breakpoint_dialog.h"

#include "e2e_mark.h"
#include <QObject>

#include <QCheckBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFormLayout>
#include <QLineEdit>
#include <QPoint>
#include <QPushButton>
#include <QRect>
#include <QTimer>
#include <QVBoxLayout>

namespace ui_shell {

void showBreakpointDialog(QWidget *parent, DebugService *debugService, const QString &path,
                           quint32 line)
{
    const FfiBreakpoint current = debugService->breakpointAt(path, line);

    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Edit Breakpoint"));
    QObject::connect(&dialog, &QDialog::finished, &dialog, [](int) {
        e2eMark("{\"ev\":\"dialog_closed\",\"name\":\"breakpoint_dialog\"}");
    });

    auto *enabledCheck = new QCheckBox(QObject::tr("Enabled"), &dialog);
    enabledCheck->setChecked(current.enabled);
    auto *temporaryCheck =
      new QCheckBox(QObject::tr("Remove once hit (temporary)"), &dialog);
    temporaryCheck->setChecked(current.temporary);
    auto *conditionEdit = new QLineEdit(current.condition, &dialog);
    conditionEdit->setPlaceholderText(QObject::tr("e.g. i > 2"));
    auto *hitConditionEdit = new QLineEdit(current.hit_condition, &dialog);
    hitConditionEdit->setPlaceholderText(QObject::tr("e.g. 5, or %3 == 0"));
    auto *logMessageEdit = new QLineEdit(current.log_message, &dialog);
    logMessageEdit->setPlaceholderText(
      QObject::tr("Log this instead of suspending, e.g. i is {i}"));

    auto *form = new QFormLayout();
    form->addRow(enabledCheck);
    form->addRow(QObject::tr("Condition:"), conditionEdit);
    form->addRow(QObject::tr("Hit count:"), hitConditionEdit);
    form->addRow(QObject::tr("Log message:"), logMessageEdit);
    form->addRow(temporaryCheck);

    auto *buttons =
      new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    auto *layout = new QVBoxLayout(&dialog);
    layout->addLayout(form);
    layout->addWidget(buttons);

    conditionEdit->setFocus();
    // Same reasoning as `showRunConfigDialog`'s own timer: `exec()` is what
    // actually grants this dialog X input focus, so the mark — and the
    // rects an E2E flow clicks rather than guesses a tab order through —
    // fire from inside the modal loop, not before it.
    QTimer::singleShot(0, &dialog, [conditionEdit, buttons]() {
        const auto rectJson = [](const QRect &rect) {
            return QStringLiteral("[%1,%2,%3,%4]")
              .arg(rect.x())
              .arg(rect.y())
              .arg(rect.width())
              .arg(rect.height());
        };
        const QRect conditionRect(conditionEdit->mapToGlobal(QPoint(0, 0)),
                                   conditionEdit->size());
        QPushButton *okButton = buttons->button(QDialogButtonBox::Ok);
        const QRect okRect(okButton->mapToGlobal(QPoint(0, 0)), okButton->size());
        e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"breakpoint_dialog\","
                                "\"condition_rect\":%1,\"ok_rect\":%2}")
                  .arg(rectJson(conditionRect))
                  .arg(rectJson(okRect)));
    });
    if (dialog.exec() != QDialog::Accepted) {
        return;
    }

    FfiBreakpoint updated{};
    updated.path = path;
    updated.line = line;
    updated.enabled = enabledCheck->isChecked();
    updated.condition = conditionEdit->text();
    updated.hit_condition = hitConditionEdit->text();
    updated.log_message = logMessageEdit->text();
    updated.temporary = temporaryCheck->isChecked();
    debugService->configureBreakpoint(updated);
}

} // namespace ui_shell
