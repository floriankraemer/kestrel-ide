#include "run_config_php_page.h"

#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QSignalBlocker>
#include <QSpinBox>
#include <QVBoxLayout>

namespace ui_shell {

PhpServerOptionsPage::PhpServerOptionsPage(QWidget *parent)
  : QWidget(parent)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);

    const auto addRow = [this, layout](const QString &label, QWidget *field) {
        auto *row = new QHBoxLayout();
        auto *labelWidget = new QLabel(label, this);
        labelWidget->setMinimumWidth(90);
        row->addWidget(labelWidget);
        row->addWidget(field, 1);
        layout->addLayout(row);
    };

    hostEdit_ = new QLineEdit(this);
    hostEdit_->setPlaceholderText(tr("localhost"));
    connect(hostEdit_, &QLineEdit::textChanged, this, &PhpServerOptionsPage::changed);
    addRow(tr("Host:"), hostEdit_);

    // 0 is "the default port"; the special-value text says so.
    portSpin_ = new QSpinBox(this);
    portSpin_->setRange(0, 65535);
    portSpin_->setSpecialValueText(tr("Default (8000)"));
    connect(portSpin_, QOverload<int>::of(&QSpinBox::valueChanged), this,
            &PhpServerOptionsPage::changed);
    addRow(tr("Port:"), portSpin_);

    documentRootEdit_ = new QLineEdit(this);
    documentRootEdit_->setPlaceholderText(tr("Project root"));
    connect(documentRootEdit_, &QLineEdit::textChanged, this, &PhpServerOptionsPage::changed);
    addRow(tr("Document root:"), documentRootEdit_);

    routerEdit_ = new QLineEdit(this);
    routerEdit_->setPlaceholderText(tr("None (serve files directly)"));
    connect(routerEdit_, &QLineEdit::textChanged, this, &PhpServerOptionsPage::changed);
    addRow(tr("Router script:"), routerEdit_);
}

void PhpServerOptionsPage::setOptions(const FfiPhpServerOptions &options)
{
    const QSignalBlocker hostBlocker(hostEdit_);
    const QSignalBlocker portBlocker(portSpin_);
    const QSignalBlocker rootBlocker(documentRootEdit_);
    const QSignalBlocker routerBlocker(routerEdit_);
    hostEdit_->setText(QString(options.host));
    portSpin_->setValue(static_cast<int>(options.port));
    documentRootEdit_->setText(QString(options.document_root));
    routerEdit_->setText(QString(options.router));
}

FfiPhpServerOptions PhpServerOptionsPage::options() const
{
    FfiPhpServerOptions options{};
    options.host = hostEdit_->text();
    options.port = static_cast<quint32>(portSpin_->value());
    options.document_root = documentRootEdit_->text();
    options.router = routerEdit_->text();
    return options;
}

} // namespace ui_shell
