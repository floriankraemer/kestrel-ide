#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QLineEdit;
class QSpinBox;

namespace ui_shell {

// The `php-builtin-server` run configuration's own page (PHP parity plan,
// I6): listen host and port, document root and router script —
// `FfiPhpServerOptions`'s exact fields.
//
// Humble view: it only collects what the user typed into `options()` and
// reads `setOptions()` back. What blank/zero mean (localhost, 8000, the
// project root) is `run-core`'s rule, shown in the placeholders only.
class PhpServerOptionsPage : public QWidget
{
    Q_OBJECT
public:
    explicit PhpServerOptionsPage(QWidget *parent);

    void setOptions(const FfiPhpServerOptions &options);
    FfiPhpServerOptions options() const;

signals:
    void changed();

private:
    QLineEdit *hostEdit_;
    QSpinBox *portSpin_;
    QLineEdit *documentRootEdit_;
    QLineEdit *routerEdit_;
};

} // namespace ui_shell
