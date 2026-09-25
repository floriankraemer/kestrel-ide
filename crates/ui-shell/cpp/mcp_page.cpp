#include "mcp_page.h"

#include <QCheckBox>
#include <QFormLayout>
#include <QLabel>
#include <QLineEdit>
#include <QObject>
#include <QSpinBox>
#include <QString>
#include <QWidget>

#include <cstdint>

namespace ui_shell {

McpPage buildMcpPage(QWidget *parent, AppSettings *appSettings, DocumentManager *docManager,
                     const QString &status)
{
    auto *mcpPage = new QWidget(parent);
    auto *mcpForm = new QFormLayout(mcpPage);
    auto *mcpEnabledCheck = new QCheckBox(QObject::tr("Enable MCP server"), mcpPage);
    mcpEnabledCheck->setChecked(appSettings->mcpEnabled());
    mcpForm->addRow(mcpEnabledCheck);

    auto *mcpPortSpin = new QSpinBox(mcpPage);
    mcpPortSpin->setRange(0, 65535);
    mcpPortSpin->setSpecialValueText(QObject::tr("Automatic"));
    mcpPortSpin->setValue(static_cast<int>(appSettings->mcpPort()));
    mcpPortSpin->setEnabled(mcpEnabledCheck->isChecked());
    mcpForm->addRow(QObject::tr("Port:"), mcpPortSpin);
    QObject::connect(mcpEnabledCheck, &QCheckBox::toggled, mcpPortSpin, &QSpinBox::setEnabled);

    auto *mcpStatusLabel = new QLabel(status, mcpPage);
    mcpStatusLabel->setWordWrap(true);
    mcpForm->addRow(QObject::tr("Status:"), mcpStatusLabel);
    // Live only while the dialog is open, so a failed restart on OK is
    // visible without reopening Settings.
    QObject::connect(docManager, &DocumentManager::mcpStarted, mcpPage,
                      [mcpStatusLabel](std::uint16_t port) {
                          mcpStatusLabel->setText(
                            QObject::tr("Listening on 127.0.0.1:%1").arg(port));
                      });
    QObject::connect(docManager, &DocumentManager::mcpStopped, mcpPage, [mcpStatusLabel]() {
        mcpStatusLabel->setText(QObject::tr("Disabled"));
    });
    QObject::connect(docManager, &DocumentManager::mcpFailed, mcpPage,
                      [mcpStatusLabel](const QString &message) {
                          mcpStatusLabel->setText(message);
                      });

    // The port and token an agent needs are written here on every start,
    // so the useful thing to show is where to read them from.
    auto *mcpDiscoveryEdit = new QLineEdit(appSettings->mcpDiscoveryFilePath(), mcpPage);
    mcpDiscoveryEdit->setReadOnly(true);
    mcpForm->addRow(QObject::tr("Discovery file:"), mcpDiscoveryEdit);

    return McpPage{
      mcpPage,
      [appSettings, docManager, mcpEnabledCheck, mcpPortSpin]() {
          appSettings->saveMcpSettings(mcpEnabledCheck->isChecked(),
                                        static_cast<quint16>(mcpPortSpin->value()));
          // Unconditional: applyMcpSettings leaves a server already running
          // for these settings alone (a restart would re-bind and re-token,
          // cutting off connected clients), and working out whether
          // anything changed here would be the view deciding it instead.
          docManager->applyMcpSettings();
      },
    };
}

std::shared_ptr<QString> wireMcpStatus(DocumentManager *docManager, QObject *context)
{
    auto status = std::make_shared<QString>(QObject::tr("Starting..."));
    QObject::connect(docManager, &DocumentManager::mcpStarted, context,
                      [status](std::uint16_t port) {
                          *status = QObject::tr("Listening on 127.0.0.1:%1").arg(port);
                      });
    QObject::connect(docManager, &DocumentManager::mcpStopped, context,
                      [status]() { *status = QObject::tr("Disabled"); });
    QObject::connect(docManager, &DocumentManager::mcpFailed, context,
                      [status](const QString &message) { *status = message; });
    return status;
}

} // namespace ui_shell
