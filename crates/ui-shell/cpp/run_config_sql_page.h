#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QCheckBox;
class QComboBox;
class QLineEdit;
class QToolButton;

namespace ui_shell {

// The `sql-script` run configuration's own page (database-tools-plan
// F3.6): which data source, which `.sql` file, and its error policy —
// `FfiSqlScriptOptions`'s exact fields, the same "structured, not JSON"
// shape `ContainerOptionsPage` gives its own three kinds.
//
// Humble view: this widget only collects what the user picked/typed into
// `options()`/reads `setOptions()` back — `app_config::SqlScriptRunSetting`
// (Rust) is what those fields become, and `RunService::launch` (also
// Rust) is what decides how to run them; this class encodes no rule of
// its own.
class SqlScriptOptionsPage : public QWidget
{
    Q_OBJECT
public:
    explicit SqlScriptOptionsPage(ConsoleService *consoleService, QWidget *parent);

    void setOptions(const FfiSqlScriptOptions &options);
    FfiSqlScriptOptions options() const;

signals:
    void changed();

private:
    void browseForFile();

    ConsoleService *consoleService_;
    QComboBox *sourceCombo_;
    QLineEdit *fileEdit_;
    QToolButton *browseButton_;
    QComboBox *txModeCombo_;
    QCheckBox *stopOnErrorCheck_;
};

} // namespace ui_shell
