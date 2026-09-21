#include "driver_install_dialog.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QDialog>
#include <QDialogButtonBox>
#include <QLabel>
#include <QMessageBox>
#include <QProgressBar>
#include <QPushButton>
#include <QVBoxLayout>
#include <QWidget>

namespace ui_shell {

void showDriverInstallDialog(QWidget *parent, DriverInstallService *service,
                              const QString &driverId, const QString &driverName)
{
    const FfiDriverConsent consent = service->consent(driverId);

    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Install %1 Driver").arg(driverName));

    auto *layout = new QVBoxLayout(&dialog);

    auto *summary = new QLabel(&dialog);
    summary->setWordWrap(true);
    summary->setText(QObject::tr("This downloads a driver published by %1:\n%2\n\n"
                                  "sha256: %3\n\n"
                                  "The download is verified against this checksum before it is "
                                  "unpacked or used.")
                        .arg(QString(consent.publisher), QString(consent.url), QString(consent.sha256)));
    layout->addWidget(summary);

    auto *progress = new QProgressBar(&dialog);
    progress->setRange(0, 0); // indeterminate: install() reports only success/failure, not a percentage.
    progress->setVisible(false);
    layout->addWidget(progress);

    auto *resultLabel = new QLabel(&dialog);
    resultLabel->setWordWrap(true);
    layout->addWidget(resultLabel);

    auto *buttons = new QDialogButtonBox(QDialogButtonBox::Cancel, &dialog);
    auto *installButton = buttons->addButton(QObject::tr("Install"), QDialogButtonBox::AcceptRole);
    installButton->setEnabled(!consent.url.isEmpty());
    layout->addWidget(buttons);

    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    QObject::connect(installButton, &QPushButton::clicked, &dialog,
                     [service, driverId, installButton, progress, resultLabel]() {
                         installButton->setEnabled(false);
                         progress->setVisible(true);
                         resultLabel->clear();
                         const FfiResult result = service->install(driverId);
                         if (result.code != 0) {
                             progress->setVisible(false);
                             installButton->setEnabled(true);
                             QMessageBox::warning(installButton->parentWidget(),
                                                   QObject::tr("Could not install"),
                                                   QString(result.message));
                         }
                     });
    QObject::connect(service, &DriverInstallService::installFinished, &dialog,
                     [&dialog, progress, resultLabel, installButton, driverId](const QString &finishedId,
                                                                                bool ok,
                                                                                const QString &message) {
                         if (finishedId != driverId) {
                             return;
                         }
                         progress->setVisible(false);
                         installButton->setEnabled(!ok);
                         resultLabel->setStyleSheet(ok ? QStringLiteral("color: #4caf50;")
                                                        : QStringLiteral("color: #e53935;"));
                         resultLabel->setText(message);
                         if (ok) {
                             dialog.accept();
                         }
                     });

    dialog.exec();
}

} // namespace ui_shell
