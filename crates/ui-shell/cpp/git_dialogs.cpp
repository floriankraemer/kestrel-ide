#include "git_dialogs.h"

#include "e2e_mark.h"

#include <QAbstractButton>
#include <QMessageBox>
#include <QPushButton>

namespace ui_shell {

bool confirmDiscardChanges(QWidget *parent, const QString &fileName, const char *markName)
{
    QMessageBox confirm(QMessageBox::Warning, QObject::tr("Revert File Changes"),
                         QObject::tr("Discard all changes to \"%1\"?").arg(fileName),
                         QMessageBox::Cancel, parent);
    confirm.setInformativeText(
      QObject::tr("The file goes back to its last committed state. This cannot be undone."));
    QAbstractButton *revert =
      confirm.addButton(QObject::tr("Revert"), QMessageBox::DestructiveRole);
    confirm.setDefaultButton(QMessageBox::Cancel);
    e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"%1\"}").arg(markName));
    confirm.exec();
    const bool accepted = confirm.clickedButton() == revert;
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"%1\",\"accepted\":%2}")
              .arg(markName, accepted ? "true" : "false"));
    return accepted;
}

} // namespace ui_shell
