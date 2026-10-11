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

// The Run Configurations dialog's one form layout, shared by its own rows
// and every kind's page (#448): a row is a widget of its own holding a
// label and its field, so hiding the row hides both, and
// `alignFormLabels` gives every label one width, so every field column
// starts at the same x.
QWidget *addFormRow(QVBoxLayout *layout, QWidget *owner, const QString &label, QWidget *field);
void alignFormLabels(QWidget *root);
// E2E only: each visible row's label and its field's global x.
QString formRowsJson(const QWidget *root);

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
    // E2E only: the line a flow types a new group into.
    QLineEdit *addEdit() const { return addEdit_; }

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
    // E2E only: the fields a flow types into, with their rects and text.
    QString e2eFieldsJson() const;

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
    // Puts `field` in a labelled row of the shared form.
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
    QLineEdit *interpreterOptionsEdit_;
    QLineEdit *workingDirectoryEdit_;
    // Each field's whole row, so a scope can hide the rows it ignores.
    QHash<QWidget *, QWidget *> rows_;
};

} // namespace ui_shell
