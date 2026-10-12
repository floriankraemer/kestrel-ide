#include "languages_page.h"

#include "theme.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QDesktopServices>
#include <QDialog>
#include <QDialogButtonBox>
#include <QFileDialog>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QMessageBox>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QRadioButton>
#include <QStringList>
#include <QTreeWidget>
#include <QTreeWidgetItem>
#include <QUrl>
#include <QVBoxLayout>
#include <QWidget>

#include <memory>

namespace ui_shell {

namespace {

constexpr int kLanguageColumn = 0;
constexpr int kMatchesColumn = 1;
constexpr int kSourceColumn = 2;
constexpr int kStatusColumn = 3;
constexpr int kIdRole = Qt::UserRole;

QString sourceGroup(FfiLanguageSource source)
{
    switch (source) {
    case FfiLanguageSource::BuiltIn:
        return QObject::tr("Built-in");
    case FfiLanguageSource::Overlay:
        return QObject::tr("User config");
    case FfiLanguageSource::Library:
        return QObject::tr("Grammar libraries");
    }
    return QString();
}

QString sourceText(FfiLanguageSource source)
{
    switch (source) {
    case FfiLanguageSource::BuiltIn:
        return QObject::tr("Built-in");
    case FfiLanguageSource::Overlay:
        return QObject::tr("Overlay");
    case FfiLanguageSource::Library:
        return QObject::tr("Library");
    }
    return QString();
}

QColor severityColorFor(FfiRowSeverity severity)
{
    const SemanticColors colors = semanticColors();
    switch (severity) {
    case FfiRowSeverity::Error:
        return colors.error;
    case FfiRowSeverity::Warning:
        return colors.warning;
    case FfiRowSeverity::Muted:
        return colors.muted;
    case FfiRowSeverity::Healthy:
        break;
    }
    return QColor();
}

QString selectedId(const QTreeWidget *tree)
{
    const QTreeWidgetItem *item = tree->currentItem();
    return item ? item->data(kLanguageColumn, kIdRole).toString() : QString();
}

// Rebuilds the tree, grouped by source. A source with no languages gets no
// header — a user with no overlays never sees a `User config` group.
void populate(QTreeWidget *tree, LanguageCatalog *catalog, bool problemsOnly,
              const QString &keepId)
{
    tree->clear();

    QHash<int, QTreeWidgetItem *> groups;
    for (const FfiLanguageRow &row : catalog->languages()) {
        if (problemsOnly && row.status.isEmpty()) {
            continue;
        }
        const int key = static_cast<int>(row.source);
        QTreeWidgetItem *group = groups.value(key);
        if (!group) {
            group = new QTreeWidgetItem(tree, QStringList{sourceGroup(row.source)});
            group->setFlags(Qt::ItemIsEnabled);
            group->setExpanded(true);
            groups.insert(key, group);
        }

        auto *item = new QTreeWidgetItem(
          group, QStringList{row.name, row.matches, sourceText(row.source), row.status});
        item->setData(kLanguageColumn, kIdRole, row.id);
        const QColor color = severityColorFor(row.severity);
        if (color.isValid()) {
            item->setForeground(kStatusColumn, color);
        }
        if (row.id == keepId) {
            tree->setCurrentItem(item);
        }
    }
    tree->expandAll();
}

// The `Add Language...` modal: two paths, because the editor ships two
// mechanisms and pretending they are one would lie about the choice.
// Returns the id that was added, or an empty string when nothing was.
QString runAddDialog(QWidget *parent, LanguageCatalog *catalog)
{
    QDialog dialog(parent);
    dialog.setWindowTitle(QObject::tr("Add a language"));
    auto *layout = new QVBoxLayout(&dialog);

    auto *folderRadio = new QRadioButton(
      QObject::tr("From a folder of tree-sitter queries"), &dialog);
    folderRadio->setChecked(true);
    auto *folderHint = new QLabel(
      QObject::tr("A folder containing language.toml and one or more .scm files."), &dialog);
    auto *folderEdit = new QLineEdit(&dialog);
    auto *folderBrowse = new QPushButton(QObject::tr("Browse…"), &dialog);

    auto *libraryRadio =
      new QRadioButton(QObject::tr("From a compiled grammar library"), &dialog);
    auto *libraryHint =
      new QLabel(QObject::tr("A shared library exporting tree_sitter_<name>."), &dialog);
    auto *libraryEdit = new QLineEdit(&dialog);
    auto *libraryBrowse = new QPushButton(QObject::tr("Browse…"), &dialog);
    // Plain text in the default foreground, not a red box: it states a fact
    // the user needs before choosing, and shouting it gets it tuned out.
    auto *libraryWarning = new QLabel(
      QObject::tr("Loading a grammar library runs code inside the editor. A faulty grammar "
                  "can crash it."),
      &dialog);
    libraryWarning->setWordWrap(true);

    auto *folderRow = new QHBoxLayout();
    folderRow->addWidget(folderEdit, 1);
    folderRow->addWidget(folderBrowse);
    auto *libraryRow = new QHBoxLayout();
    libraryRow->addWidget(libraryEdit, 1);
    libraryRow->addWidget(libraryBrowse);

    layout->addWidget(folderRadio);
    layout->addWidget(folderHint);
    layout->addLayout(folderRow);
    layout->addSpacing(8);
    layout->addWidget(libraryRadio);
    layout->addWidget(libraryHint);
    layout->addLayout(libraryRow);
    layout->addWidget(libraryWarning);

    auto *buttons =
      new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, &dialog);
    buttons->button(QDialogButtonBox::Ok)->setText(QObject::tr("Add"));
    layout->addWidget(buttons);
    QObject::connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    QObject::connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);

    auto syncEnabled = [=]() {
        const bool folder = folderRadio->isChecked();
        folderEdit->setEnabled(folder);
        folderBrowse->setEnabled(folder);
        libraryEdit->setEnabled(!folder);
        libraryBrowse->setEnabled(!folder);
    };
    QObject::connect(folderRadio, &QRadioButton::toggled, &dialog, syncEnabled);
    syncEnabled();

    QObject::connect(folderBrowse, &QPushButton::clicked, &dialog, [&dialog, folderEdit]() {
        const QString dir = QFileDialog::getExistingDirectory(
          &dialog, QObject::tr("Language Folder"), folderEdit->text());
        if (!dir.isEmpty()) {
            folderEdit->setText(dir);
        }
    });
    QObject::connect(libraryBrowse, &QPushButton::clicked, &dialog, [&dialog, libraryEdit]() {
        const QString file = QFileDialog::getOpenFileName(&dialog, QObject::tr("Grammar Library"),
                                                           libraryEdit->text());
        if (!file.isEmpty()) {
            libraryEdit->setText(file);
        }
    });

    if (dialog.exec() != QDialog::Accepted) {
        return QString();
    }

    const bool folder = folderRadio->isChecked();
    const QString path = folder ? folderEdit->text().trimmed() : libraryEdit->text().trimmed();
    if (path.isEmpty()) {
        return QString();
    }
    const FfiResult result =
      folder ? catalog->addLanguageFolder(path) : catalog->addGrammarLibrary(path);
    if (result.code != 0) {
        QMessageBox::critical(parent, QObject::tr("Cannot add language"), result.message);
        return QString();
    }
    // Whatever happened next is visible in the list and the details pane; a
    // modal saying "added successfully" over a list saying "query error"
    // would be two sources of truth.
    return QFileInfo(path).completeBaseName();
}

} // namespace

QWidget *buildLanguagesPage(QWidget *parent,
                            LanguageCatalog *catalog,
                            std::function<void(const QString &)> openFile,
                            std::function<void()> languagesChanged)
{
    auto *page = new QWidget(parent);
    page->setMinimumSize(560, 460);
    auto *layout = new QVBoxLayout(page);

    auto *problemsOnly = new QCheckBox(QObject::tr("Show only languages with problems"), page);
    auto *addButton = new QPushButton(QObject::tr("Add Language…"), page);
    // Rescans the config directory and re-renders. Live reload of the
    // *running* registry is plan task G2's `Arc` swap; until that lands this
    // shows what a restart would pick up, which is what the page is for.
    auto *reloadButton = new QPushButton(QObject::tr("Reload Languages"), page);
    auto *topRow = new QHBoxLayout();
    topRow->addWidget(problemsOnly, 1);
    topRow->addWidget(reloadButton);
    topRow->addWidget(addButton);
    layout->addLayout(topRow);

    auto *tree = new QTreeWidget(page);
    tree->setColumnCount(4);
    tree->setHeaderLabels({QObject::tr("Language"), QObject::tr("Matches"), QObject::tr("Source"),
                           QObject::tr("Status")});
    // Matches is the one column allowed to absorb the leftover width, and
    // therefore the one that elides: Bash and Dockerfile enumerate a dozen
    // filenames each, and sizing to that longest content pushed Status off
    // the right edge behind a horizontal scrollbar. Status is the column
    // this page exists for — a healthy row leaves it empty precisely so the
    // one row that failed catches the eye, which only works while it is on
    // screen. Every other column sizes to its content and the last section
    // does not stretch, so the sections always sum to the viewport and the
    // horizontal scrollbar never appears.
    tree->header()->setStretchLastSection(false);
    tree->header()->setSectionResizeMode(kLanguageColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kMatchesColumn, QHeaderView::Stretch);
    tree->header()->setSectionResizeMode(kSourceColumn, QHeaderView::ResizeToContents);
    tree->header()->setSectionResizeMode(kStatusColumn, QHeaderView::ResizeToContents);
    tree->setTextElideMode(Qt::ElideRight);
    tree->setRootIsDecorated(false);
    tree->setIndentation(12);
    layout->addWidget(tree, 1);

    // The details pane: selectable so the message and the path can be
    // copied, and hidden entirely when the selected language is healthy.
    auto *details = new QPlainTextEdit(page);
    details->setReadOnly(true);
    details->setMaximumHeight(110);
    details->setVisible(false);
    layout->addWidget(details);

    // The strip's own control, always present, acting on the selected row.
    // It carries both directions: a language the user turned off is switched
    // back on from the same button that turned it off, and the caption says
    // which way it goes. The per-problem buttons keep to the right.
    auto *toggleButton = new QPushButton(page);
    auto *openFileButton = new QPushButton(QObject::tr("Open File"), page);
    auto *reloadProblemButton = new QPushButton(QObject::tr("Reload"), page);
    auto *openFolderButton = new QPushButton(QObject::tr("Open Folder"), page);
    auto *actionsRow = new QHBoxLayout();
    actionsRow->addWidget(toggleButton);
    actionsRow->addStretch(1);
    actionsRow->addWidget(openFileButton);
    actionsRow->addWidget(reloadProblemButton);
    actionsRow->addWidget(openFolderButton);
    layout->addLayout(actionsRow);

    auto *pathLabel = new QLabel(
      QObject::tr("Languages are read from %1").arg(catalog->languagesDir()), page);
    pathLabel->setTextInteractionFlags(Qt::TextSelectableByMouse);
    pathLabel->setStyleSheet(QStringLiteral("color: %1;").arg(semanticColors().muted.name()));
    layout->addWidget(pathLabel);

    // The problem currently shown, so the action buttons know what to act on
    // without re-deriving it from the widgets.
    auto current = std::make_shared<FfiLanguageProblem>();

    // Which way the toggle currently goes, so the click handler acts on the
    // same decision the caption showed.
    auto toggleState = std::make_shared<FfiLanguageToggle>();

    auto showProblem = [=](const QString &id) {
        *current = catalog->problem(id);
        *toggleState = catalog->toggle(id);
        toggleButton->setText(toggleState->label);
        toggleButton->setEnabled(toggleState->enabled);
        const bool hasProblem = !current->sentence.isEmpty();
        details->setVisible(hasProblem);
        openFileButton->setVisible(hasProblem && current->open_file);
        reloadProblemButton->setVisible(hasProblem && current->reload);
        openFolderButton->setVisible(hasProblem && current->open_folder);
        if (!hasProblem) {
            details->clear();
            return;
        }
        QStringList lines;
        // No artefact for a language that simply failed nowhere — a user
        // disable names itself and nothing else.
        lines << (current->artifact.isEmpty() ? id
                                              : QObject::tr("%1 — %2").arg(id, current->artifact));
        lines << current->sentence;
        if (!current->detail.isEmpty()) {
            lines << current->detail;
        }
        lines << current->path;
        details->setPlainText(lines.join(QLatin1Char('\n')));
    };

    auto refresh = [=](const QString &keepId) {
        catalog->refresh();
        populate(tree, catalog, problemsOnly->isChecked(), keepId);
        showProblem(selectedId(tree));
    };

    QObject::connect(tree, &QTreeWidget::currentItemChanged, page,
                      [=]() { showProblem(selectedId(tree)); });
    QObject::connect(problemsOnly, &QCheckBox::toggled, page,
                      [=]() { populate(tree, catalog, problemsOnly->isChecked(), selectedId(tree)); });
    QObject::connect(reloadButton, &QPushButton::clicked, page,
                      [=]() { refresh(selectedId(tree)); });
    QObject::connect(reloadProblemButton, &QPushButton::clicked, page,
                      [=]() { refresh(selectedId(tree)); });

    QObject::connect(addButton, &QPushButton::clicked, page, [=]() {
        const QString added = runAddDialog(page, catalog);
        refresh(added);
    });

    QObject::connect(openFileButton, &QPushButton::clicked, page, [=]() {
        if (openFile && !current->path.isEmpty()) {
            openFile(current->path);
        }
    });

    QObject::connect(openFolderButton, &QPushButton::clicked, page, [=]() {
        QDesktopServices::openUrl(QUrl::fromLocalFile(QFileInfo(current->path).absolutePath()));
    });

    // Whether re-enabling needs confirming, and what it asks, is decided in
    // Rust: re-arming a grammar that already crashed the editor is the one
    // setting in this dialog that can take the application down, and a plain
    // user disable is not. Turning a language off needs no confirmation and
    // gets no "Enabled" checkmark anywhere — it is reversible from this same
    // button, and a healthy row keeps an empty Status cell so the one row
    // that failed still catches the eye.
    QObject::connect(toggleButton, &QPushButton::clicked, page, [=]() {
        const QString id = selectedId(tree);
        const bool disable = toggleState->disable;
        if (!disable && !current->confirm.isEmpty()) {
            const auto answer =
              QMessageBox::warning(page, QObject::tr("Enable Language"), current->confirm,
                                   QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel);
            if (answer != QMessageBox::Yes) {
                return;
            }
        }
        const FfiResult result = catalog->setDisabled(id, disable);
        if (result.code != 0) {
            QMessageBox::critical(page,
                                  disable ? QObject::tr("Cannot disable language")
                                          : QObject::tr("Cannot enable language"),
                                  result.message);
            return;
        }
        refresh(id);
        if (languagesChanged) {
            languagesChanged();
        }
    });

    refresh(QString());
    return page;
}

} // namespace ui_shell
