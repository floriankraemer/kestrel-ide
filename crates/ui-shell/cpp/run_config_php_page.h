#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QWidget>

class QComboBox;
class QLineEdit;
class QSpinBox;
class QVBoxLayout;

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

// The `phpunit` run configuration's own page (PHPUnit editor runs plan, E4):
// the scope selector and the fields each scope reads, then the options every
// scope shares — `FfiPhpUnitOptions`'s exact fields. Which fields a scope
// needs is shown, not decided: `php-core::phpunit_run` refuses a table that
// leaves a needed one blank when the dialog saves.
class PhpUnitOptionsPage : public QWidget
{
    Q_OBJECT
public:
    explicit PhpUnitOptionsPage(RunConfigEditor *editor, QWidget *parent);

    void setOptions(const FfiPhpUnitOptions &options);
    FfiPhpUnitOptions options() const;

signals:
    void changed();

private:
    void showFieldsForScope();
    // Fills `edit` with the project-relative path of a picked file or
    // directory.
    void browse(QLineEdit *edit, bool directory);
    QLineEdit *addField(const QString &label, bool browseDirectory, bool browseFile);
    QVBoxLayout *layout_;

    RunConfigEditor *editor_;
    QComboBox *scopeCombo_;
    QLineEdit *pathEdit_;
    QLineEdit *classEdit_;
    QLineEdit *methodEdit_;
    QLineEdit *dataSetEdit_;
    QLineEdit *filterEdit_;
    QLineEdit *testsuiteEdit_;
    QLineEdit *configurationEdit_;
    QLineEdit *groupsEdit_;
    QLineEdit *excludeGroupsEdit_;
    QLineEdit *runnerOptionsEdit_;
    QLineEdit *interpreterEdit_;
    // Each field's whole row, so a scope can hide the rows it ignores.
    QHash<QLineEdit *, QWidget *> rows_;
};

} // namespace ui_shell
