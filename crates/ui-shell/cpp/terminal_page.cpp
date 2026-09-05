#include "terminal_page.h"

#include <QComboBox>
#include <QFileDialog>
#include <QFormLayout>
#include <QHBoxLayout>
#include <QLabel>
#include <QLineEdit>
#include <QObject>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QVBoxLayout>
#include <QVariant>
#include <QWidget>

namespace ui_shell {

namespace {

// The combo's last entry: a shell this build's catalogue has never heard
// of, named by path. Its data is a value no `ShellCandidate::id` can be,
// so the two never collide.
const char *const kCustomShell = "__custom__";

} // namespace

TerminalPage buildTerminalPage(QWidget *parent, AppSettings *appSettings)
{
    const FfiTerminalSettings current = appSettings->terminalSettings();

    auto *page = new QWidget(parent);
    // A form inside a box, not the page's own layout: QFormLayout hands its
    // leftover vertical space to the last rows, which pushed the hint below
    // a band of empty space. The stretch at the bottom absorbs it instead,
    // and the form stays tight under the first row.
    auto *pageLayout = new QVBoxLayout(page);
    pageLayout->setContentsMargins(0, 0, 0, 0);
    auto *form = new QFormLayout();
    pageLayout->addLayout(form);

    auto *shellBox = new QComboBox(page);
    shellBox->addItem(QObject::tr("Default shell"), QString());
    for (const FfiShellCandidate &shell : appSettings->availableShells()) {
        shellBox->addItem(shell.label, shell.id);
    }
    shellBox->addItem(QObject::tr("Custom..."), QString::fromLatin1(kCustomShell));
    form->addRow(QObject::tr("Shell:"), shellBox);

    auto *pathEdit = new QLineEdit(current.shell_path, page);
    pathEdit->setPlaceholderText(QObject::tr("/usr/bin/fish"));
    form->addRow(QObject::tr("Shell path:"), pathEdit);

    auto *argsEdit = new QLineEdit(current.shell_args, page);
    argsEdit->setPlaceholderText(QObject::tr("--login"));
    form->addRow(QObject::tr("Shell arguments:"), argsEdit);

    // A custom path is what "Custom..." means, so the two controls follow
    // the combo rather than standing on their own.
    auto *pathRow = form->labelForField(pathEdit);
    auto syncCustomEnabled = [shellBox, pathEdit, pathRow]() {
        const bool custom = shellBox->currentData().toString() == QString::fromLatin1(kCustomShell);
        pathEdit->setEnabled(custom);
        if (pathRow != nullptr) {
            pathRow->setEnabled(custom);
        }
    };

    // A saved path is what makes the selection "Custom...", since it is the
    // field that wins at spawn time.
    if (!current.shell_path.isEmpty()) {
        shellBox->setCurrentIndex(shellBox->findData(QString::fromLatin1(kCustomShell)));
    } else {
        const int index = shellBox->findData(current.shell_id);
        // A shell named in the settings but no longer installed is not in
        // the list; showing "Default shell" says what will actually happen.
        shellBox->setCurrentIndex(index >= 0 ? index : 0);
    }
    QObject::connect(shellBox, &QComboBox::currentIndexChanged, page, syncCustomEnabled);
    syncCustomEnabled();

    auto *directoryEdit = new QLineEdit(current.start_directory, page);
    directoryEdit->setPlaceholderText(QObject::tr("Project root"));
    auto *browseButton = new QPushButton(QObject::tr("Browse..."), page);
    QObject::connect(browseButton, &QPushButton::clicked, page, [page, directoryEdit]() {
        const QString chosen = QFileDialog::getExistingDirectory(
          page, QObject::tr("Terminal Start Directory"), directoryEdit->text());
        if (!chosen.isEmpty()) {
            directoryEdit->setText(chosen);
        }
    });
    // The layout goes in directly rather than wrapped in a QWidget: a
    // wrapper's width hint is the sum of both children, which is wider than
    // the form's field column, and QFormLayout answers that by wrapping the
    // label onto its own row — leaving "Start directory:" floating above an
    // otherwise aligned form.
    auto *directoryLayout = new QHBoxLayout();
    directoryLayout->setContentsMargins(0, 0, 0, 0);
    directoryLayout->addWidget(directoryEdit, 1);
    directoryLayout->addWidget(browseButton);
    form->addRow(QObject::tr("Start directory:"), directoryLayout);

    auto *envEdit = new QPlainTextEdit(current.env, page);
    envEdit->setPlaceholderText(QStringLiteral("RUST_LOG=debug"));
    // Capped, or the only multi-line control on the page takes every pixel
    // the form does not, and a page of five short fields reads as a text
    // editor with some labels above it.
    envEdit->setMaximumHeight(envEdit->fontMetrics().lineSpacing() * 8);
    form->addRow(QObject::tr("Environment variables:"), envEdit);

    auto *hint = new QLabel(
      QObject::tr("One KEY=VALUE per line, added to the environment the shell inherits. "
                  "Open terminal tabs keep the shell they started with."),
      page);
    hint->setWordWrap(true);
    hint->setEnabled(false);
    pageLayout->addWidget(hint);
    pageLayout->addStretch(1);

    return TerminalPage{
      page,
      [appSettings, shellBox, pathEdit, argsEdit, directoryEdit, envEdit]() {
          const QString selected = shellBox->currentData().toString();
          const bool custom = selected == QString::fromLatin1(kCustomShell);
          FfiTerminalSettings settings;
          settings.shell_id = custom ? QString() : selected;
          settings.shell_path = custom ? pathEdit->text() : QString();
          settings.shell_args = argsEdit->text();
          settings.start_directory = directoryEdit->text();
          settings.env = envEdit->toPlainText();
          appSettings->saveTerminalSettings(settings);
      },
    };
}

} // namespace ui_shell
