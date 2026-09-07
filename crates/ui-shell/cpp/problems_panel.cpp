#include "problems_panel.h"

#include "e2e_mark.h"

#include "icon_cache.h"
#include "theme.h"

#include <QApplication>
#include <QClipboard>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QShortcut>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

// Item data roles on a diagnostic row; group rows carry only the path.
constexpr int kPathRole = Qt::UserRole;
constexpr int kLineRole = Qt::UserRole + 1;
constexpr int kColumnRole = Qt::UserRole + 2;
constexpr int kSeverityRole = Qt::UserRole + 3;
constexpr int kHaystackRole = Qt::UserRole + 4;

QString severityText(FfiSeverity severity)
{
    switch (severity) {
    case FfiSeverity::Error:
        return QObject::tr("Error");
    case FfiSeverity::Warning:
        return QObject::tr("Warning");
    case FfiSeverity::Information:
        return QObject::tr("Info");
    case FfiSeverity::Hint:
        return QObject::tr("Hint");
    }
    return QString();
}

} // namespace

QColor severityColor(FfiSeverity severity)
{
    // The product-wide semantic set (`theme.h`), not hues of this panel's
    // own: the same red has to mean the same thing in the Languages page,
    // the status bar and the editor's squiggles.
    const SemanticColors colors = semanticColors();
    switch (severity) {
    case FfiSeverity::Error:
        return colors.error;
    case FfiSeverity::Warning:
        return colors.warning;
    case FfiSeverity::Information:
        return colors.info;
    case FfiSeverity::Hint:
        return colors.muted;
    }
    return QColor();
}

ProblemsPanel::ProblemsPanel(LanguageService *languageService, BuildService *buildService,
                             DiagnosticsService *diagnosticsService, OpenAt openAt,
                             QWidget *parent)
  : QWidget(parent)
  , languageService_(languageService)
  , buildService_(buildService)
  , diagnosticsService_(diagnosticsService)
  , openAt_(std::move(openAt))
{
    filterEdit_ = new QLineEdit(this);
    filterEdit_->setPlaceholderText(tr("Filter"));
    filterEdit_->setClearButtonEnabled(true);

    // Hints are counted with the informational ones: a fourth toggle for a
    // severity most servers never emit would be a permanently disabled button.
    errorsButton_ = new QPushButton(this);
    warningsButton_ = new QPushButton(this);
    infosButton_ = new QPushButton(this);
    for (QPushButton *button : {errorsButton_, warningsButton_, infosButton_}) {
        button->setCheckable(true);
        button->setFlat(true);
    }
    errorsButton_->setChecked(true);
    warningsButton_->setChecked(true);
    // Info off by default: a chatty server would otherwise bury the errors.
    infosButton_->setChecked(false);

    tree_ = new QTreeWidget(this);
    tree_->setColumnCount(4);
    tree_->setHeaderLabels({tr("Severity"), tr("Line:Column"), tr("Message"), tr("Source")});
    tree_->header()->setSectionResizeMode(0, QHeaderView::ResizeToContents);
    tree_->header()->setSectionResizeMode(1, QHeaderView::ResizeToContents);
    tree_->header()->setSectionResizeMode(2, QHeaderView::Stretch);
    tree_->header()->setSectionResizeMode(3, QHeaderView::ResizeToContents);
    tree_->setUniformRowHeights(true);
    tree_->setRootIsDecorated(true);

    statusLabel_ = new QLabel(this);

    auto *topRow = new QHBoxLayout();
    topRow->addWidget(filterEdit_, 1);
    topRow->addWidget(errorsButton_);
    topRow->addWidget(warningsButton_);
    topRow->addWidget(infosButton_);

    auto *layout = new QVBoxLayout(this);
    layout->addLayout(topRow);
    layout->addWidget(tree_, 1);
    layout->addWidget(statusLabel_);

    // Tab order per the spec: filter, the three toggles, then the tree.
    setTabOrder(filterEdit_, errorsButton_);
    setTabOrder(errorsButton_, warningsButton_);
    setTabOrder(warningsButton_, infosButton_);
    setTabOrder(infosButton_, tree_);

    connect(filterEdit_, &QLineEdit::textChanged, this, [this]() { applyFilter(); });
    for (QPushButton *button : {errorsButton_, warningsButton_, infosButton_}) {
        connect(button, &QPushButton::toggled, this, [this]() { applyFilter(); });
    }
    // itemActivated covers both the double-click and Enter, so there is one
    // path into the editor rather than two that can drift apart.
    connect(tree_, &QTreeWidget::itemActivated, this, &ProblemsPanel::openRow);

    auto *copyShortcut = new QShortcut(QKeySequence::Copy, tree_);
    copyShortcut->setContext(Qt::WidgetShortcut);
    connect(copyShortcut, &QShortcut::activated, this, &ProblemsPanel::copySelection);

    auto *clearFilter = new QShortcut(QKeySequence(Qt::Key_Escape), filterEdit_);
    clearFilter->setContext(Qt::WidgetShortcut);
    connect(clearFilter, &QShortcut::activated, filterEdit_, &QLineEdit::clear);

    connect(languageService_, &LanguageService::diagnosticsChanged, this, &ProblemsPanel::refresh);
    connect(buildService_, &BuildService::diagnosticsChanged, this, &ProblemsPanel::refresh);
    connect(languageService_,
            &LanguageService::serverStateChanged,
            this,
            [this](const QString &, const QString &name, FfiServerState state,
                   const QString &detail, quint32 retryMs) {
                switch (state) {
                case FfiServerState::Starting:
                    serverStatus_ = tr("Waiting for %1...").arg(name);
                    break;
                case FfiServerState::Ready:
                    serverStatus_.clear();
                    break;
                case FfiServerState::Exited:
                    // Never a dialog: the restart backoff would make one
                    // unusable. The rows stay, dimmed by nothing but this
                    // sentence saying they are now stale.
                    serverStatus_ = tr("%1 stopped; restarting in %2 s. These results may be stale.")
                                      .arg(name)
                                      .arg(retryMs / 1000.0, 0, 'f', 1);
                    break;
                case FfiServerState::Failed:
                    serverStatus_ = tr("%1 is not running: %2").arg(name, detail);
                    break;
                }
                applyFilter();
            });

    refresh();
}

void ProblemsPanel::setFirstDiagnosticCallback(std::function<void()> callback)
{
    firstDiagnostic_ = std::move(callback);
}

void ProblemsPanel::setCurrentFile(const QString &path)
{
    if (currentFile_ == path) {
        return;
    }
    currentFile_ = path;
    refresh();
}

void ProblemsPanel::focusTree()
{
    tree_->setFocus();
}

void ProblemsPanel::refresh()
{
    // One store, one list (ADR-0046): `DiagnosticsService` already merges
    // every source's rows in file/line/column/severity order, so there is
    // no second merge to do here.
    const ::rust::Vec<FfiDiagnostic> allRows = diagnosticsService_->diagnostics();
    QVector<FfiDiagnostic> rows;
    rows.reserve(static_cast<int>(allRows.size()));
    for (const FfiDiagnostic &row : allRows) {
        rows.append(row);
    }

    tree_->clear();
    QTreeWidgetItem *currentFileGroup = nullptr;
    QTreeWidgetItem *group = nullptr;
    QString groupPath;
    int inGroup = 0;

    const auto finishGroup = [&]() {
        if (group) {
            group->setText(0, QStringLiteral("%1 (%2)").arg(groupPath).arg(inGroup));
        }
    };

    for (const FfiDiagnostic &row : rows) {
        const QString path = QString(row.path);
        if (!group || path != groupPath) {
            finishGroup();
            groupPath = path;
            inGroup = 0;
            group = new QTreeWidgetItem(tree_);
            group->setData(0, kPathRole, path);
            group->setIcon(0, fileIcon(path, smallIconPx(tree_)));
            group->setFirstColumnSpanned(true);
            group->setExpanded(true);
            if (path == currentFile_) {
                currentFileGroup = group;
            }
        }
        ++inGroup;

        auto *item = new QTreeWidgetItem(group);
        item->setText(0, severityText(row.severity));
        item->setForeground(0, severityColor(row.severity));
        item->setText(1, QStringLiteral("%1:%2").arg(row.line).arg(row.column + 1));
        item->setText(2, QString(row.message));
        item->setText(3, QString(row.source));
        item->setData(0, kPathRole, path);
        item->setData(0, kLineRole, static_cast<int>(row.line));
        item->setData(0, kColumnRole, static_cast<int>(row.column));
        item->setData(0, kSeverityRole, static_cast<int>(row.severity));
        item->setData(0,
                      kHaystackRole,
                      QStringLiteral("%1 %2 %3").arg(path, QString(row.message),
                                                      QString(row.source)));
    }
    finishGroup();

    // The file the user is looking at first — its diagnostics are the ones
    // being acted on.
    if (currentFileGroup) {
        const int index = tree_->indexOfTopLevelItem(currentFileGroup);
        if (index > 0) {
            tree_->insertTopLevelItem(0, tree_->takeTopLevelItem(index));
            currentFileGroup->setExpanded(true);
        }
    }

    if (!rows.empty() && !announced_) {
        announced_ = true;
        if (firstDiagnostic_) {
            firstDiagnostic_();
        }
    }

    applyFilter();
}

bool ProblemsPanel::severityEnabled(FfiSeverity severity) const
{
    switch (severity) {
    case FfiSeverity::Error:
        return errorsButton_->isChecked();
    case FfiSeverity::Warning:
        return warningsButton_->isChecked();
    case FfiSeverity::Information:
    case FfiSeverity::Hint:
        return infosButton_->isChecked();
    }
    return true;
}

void ProblemsPanel::applyFilter()
{
    const QString needle = filterEdit_->text().trimmed();
    int shown = 0;
    int total = 0;
    int errors = 0;
    int warnings = 0;
    int infos = 0;

    for (int g = 0; g < tree_->topLevelItemCount(); ++g) {
        QTreeWidgetItem *group = tree_->topLevelItem(g);
        int visibleChildren = 0;
        for (int i = 0; i < group->childCount(); ++i) {
            QTreeWidgetItem *item = group->child(i);
            const auto severity = static_cast<FfiSeverity>(item->data(0, kSeverityRole).toInt());
            switch (severity) {
            case FfiSeverity::Error:
                ++errors;
                break;
            case FfiSeverity::Warning:
                ++warnings;
                break;
            default:
                ++infos;
                break;
            }
            ++total;

            const bool matches =
              needle.isEmpty()
              || item->data(0, kHaystackRole).toString().contains(needle, Qt::CaseInsensitive);
            const bool visible = matches && severityEnabled(severity);
            item->setHidden(!visible);
            if (visible) {
                ++visibleChildren;
                ++shown;
            }
        }
        // A group with nothing left in it is not a group.
        group->setHidden(visibleChildren == 0);
    }

    errorsButton_->setText(tr("%n Error(s)", nullptr, errors));
    warningsButton_->setText(tr("%n Warning(s)", nullptr, warnings));
    infosButton_->setText(tr("%n Info(s)", nullptr, infos));
    errorsButton_->setEnabled(errors > 0);
    warningsButton_->setEnabled(warnings > 0);
    infosButton_->setEnabled(infos > 0);

    updateStatus(shown, total);

    // The only way anything outside the process can know the dock has caught
    // up with a build or a language server — an E2E flow asserting a
    // compile error reached it would otherwise race the signal that put it
    // there (B1-9).
    e2eMark(QStringLiteral("{\"ev\":\"problems_refreshed\",\"shown\":%1,\"total\":%2}")
              .arg(shown)
              .arg(total));
}

void ProblemsPanel::updateStatus(int shown, int total)
{
    if (!serverStatus_.isEmpty()) {
        statusLabel_->setText(serverStatus_);
        return;
    }
    if (total == 0) {
        statusLabel_->setText(
          !currentFile_.isEmpty() && !languageService_->hasServerForFile(currentFile_)
            ? tr("No language server is running for this file. Configure one in "
                 "settings.toml's [[language_server]] table.")
            : tr("No problems."));
        return;
    }
    statusLabel_->setText(shown == total ? tr("%n problem(s).", nullptr, total)
                                          : tr("Showing %1 of %2.").arg(shown).arg(total));
}

void ProblemsPanel::openRow(QTreeWidgetItem *item, int)
{
    if (!item || item->childCount() > 0 || !item->parent()) {
        // A group header expands/collapses (Qt does that itself) and opens
        // nothing.
        return;
    }
    if (openAt_) {
        openAt_(item->data(0, kPathRole).toString(),
                item->data(0, kLineRole).toInt(),
                item->data(0, kColumnRole).toInt());
    }
}

void ProblemsPanel::copySelection()
{
    QTreeWidgetItem *item = tree_->currentItem();
    if (!item || !item->parent()) {
        return;
    }
    // The form that pastes usefully into a terminal or an issue.
    QApplication::clipboard()->setText(QStringLiteral("%1:%2:%3: %4: %5")
                                          .arg(item->data(0, kPathRole).toString())
                                          .arg(item->data(0, kLineRole).toInt())
                                          .arg(item->data(0, kColumnRole).toInt() + 1)
                                          .arg(item->text(0).toLower(), item->text(2)));
}

} // namespace ui_shell
