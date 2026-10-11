#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QWidget>

class QComboBox;
class QLabel;
class QLineEdit;
class QListWidget;
class QTimer;
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

// A checkable list of names plus a line to add one the list lacks: the
// `phpunit` page's group pickers. It holds what the user checked; which
// names exist is the bridge's answer (`setChoices`).
class GroupPicker : public QWidget
{
    Q_OBJECT
public:
    explicit GroupPicker(QWidget *parent);

    QStringList checked() const;
    // Checks exactly `names`, adding any the list lacks.
    void setChecked(const QStringList &names);
    // Replaces the offered names, keeping what is checked.
    void setChoices(const QStringList &names);

signals:
    void changed();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;

private:
    void addChecked(const QString &name, bool checked);
    QListWidget *list_;
    QLineEdit *addEdit_;
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
    void showEvent(QShowEvent *event) override;
    // Asks the bridge for the configuration file's suites and groups, once
    // the user stops typing the file name.
    void scheduleModelRequest();
    void applyModel(const FfiPhpUnitModel &model);
    // Fills `edit` with the project-relative path of a picked file or
    // directory.
    void browse(QLineEdit *edit, bool directory);
    QLineEdit *addField(const QString &label, bool browseDirectory, bool browseFile);
    // Puts `field` in a labelled row of its own.
    void addRow(const QString &label, QWidget *field);
    QVBoxLayout *layout_;

    RunConfigEditor *editor_;
    QComboBox *scopeCombo_;
    QLineEdit *pathEdit_;
    QLineEdit *classEdit_;
    QLineEdit *methodEdit_;
    QLineEdit *dataSetEdit_;
    QLineEdit *filterEdit_;
    QComboBox *testsuiteCombo_;
    QLineEdit *configurationEdit_;
    QLabel *configurationProblem_;
    GroupPicker *groupsPicker_;
    GroupPicker *excludeGroupsPicker_;
    QTimer *modelTimer_;
    QLineEdit *runnerOptionsEdit_;
    QLineEdit *interpreterEdit_;
    QLineEdit *workingDirectoryEdit_;
    // Each field's whole row, so a scope can hide the rows it ignores.
    QHash<QWidget *, QWidget *> rows_;
};

} // namespace ui_shell
