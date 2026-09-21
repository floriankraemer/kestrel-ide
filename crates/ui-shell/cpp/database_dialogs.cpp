#include "database_dialogs.h"

#include <QMessageBox>
#include <QObject>
#include <QPushButton>
#include <QWidget>

namespace ui_shell {

void showAskContinueDialog(ConsoleService *consoleService, quint64 resultId,
                           const QString &message, QWidget *parent)
{
    QMessageBox box(parent);
    box.setIcon(QMessageBox::Warning);
    box.setWindowTitle(QObject::tr("Statement Failed"));
    box.setText(QObject::tr("A statement in this script failed:\n\n%1").arg(message));
    box.setInformativeText(QObject::tr("Continue running the rest of the script?"));
    QPushButton *continueButton =
      box.addButton(QObject::tr("Continue"), QMessageBox::AcceptRole);
    box.addButton(QObject::tr("Stop"), QMessageBox::RejectRole);
    box.setDefaultButton(continueButton);
    box.exec();
    consoleService->resume(resultId, box.clickedButton() == continueButton);
}

} // namespace ui_shell
