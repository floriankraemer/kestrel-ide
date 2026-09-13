#include "search_results_panel.h"

#include "editor_tabs.h"
#include "highlight_delegate.h"
#include "icon_cache.h"
#include "refactor_preview_dialog.h"
#include "search_preview_pane.h"

#include <QCheckBox>
#include <QComboBox>
#include <QFileInfo>
#include <QHBoxLayout>
#include <QHeaderView>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QSplitter>
#include <QTreeWidget>
#include <QVBoxLayout>
#include <QVariantList>

namespace {

// Item data roles on a match row. The file-group rows carry only the path.
constexpr int kPathRole = Qt::UserRole;
constexpr int kLineRole = Qt::UserRole + 1;
constexpr int kStartRole = Qt::UserRole + 2;
constexpr int kEndRole = Qt::UserRole + 3;

// R8's scope combo. `Directory...` (a picker dialog) is out of scope for
// this pass — see the PR's Plan deviations — so the choices are the three
// that need no extra UI: the whole project, every open tab, or just the
// one the caret is in.
enum ScopeChoice
{
    kScopeProject = 0,
    kScopeOpenFiles = 1,
    kScopeCurrentFile = 2,
};

} // namespace

SearchResultsPanel::SearchResultsPanel(SearchModel *searchModel, ui_shell::EditorTabs *editorTabs,
                                        OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , searchModel_(searchModel)
  , editorTabs_(editorTabs)
  , openAt_(std::move(openAt))
{
    queryEdit_ = new QLineEdit(this);
    queryEdit_->setPlaceholderText(tr("Find in files..."));
    regexCheck_ = new QCheckBox(tr("Regex"), this);
    caseCheck_ = new QCheckBox(tr("Match case"), this);
    wholeWordCheck_ = new QCheckBox(tr("Whole word"), this);
    replaceEdit_ = new QLineEdit(this);
    replaceEdit_->setPlaceholderText(tr("Replace with..."));
    auto *replaceAllButton = new QPushButton(tr("Replace All"), this);
    statusLabel_ = new QLabel(this);

    maskEdit_ = new QLineEdit(this);
    maskEdit_->setPlaceholderText(tr("File mask (*.rs, !*_test.rs)"));
    scopeCombo_ = new QComboBox(this);
    scopeCombo_->addItem(tr("Project"), kScopeProject);
    scopeCombo_->addItem(tr("Open Files"), kScopeOpenFiles);
    scopeCombo_->addItem(tr("Current File"), kScopeCurrentFile);

    results_ = new QTreeWidget(this);
    results_->setColumnCount(1);
    results_->setHeaderHidden(true);
    results_->setUniformRowHeights(true);
    results_->setItemDelegate(new HighlightDelegate(results_));

    preview_ = new ui_shell::SearchPreviewPane(this);

    auto *topRow = new QHBoxLayout();
    topRow->addWidget(queryEdit_, 1);
    topRow->addWidget(regexCheck_);
    topRow->addWidget(caseCheck_);
    topRow->addWidget(wholeWordCheck_);

    auto *scopeRow = new QHBoxLayout();
    scopeRow->addWidget(maskEdit_, 1);
    scopeRow->addWidget(scopeCombo_);

    auto *replaceRow = new QHBoxLayout();
    replaceRow->addWidget(replaceEdit_, 1);
    replaceRow->addWidget(replaceAllButton);

    auto *splitter = new QSplitter(Qt::Horizontal, this);
    splitter->addWidget(results_);
    splitter->addWidget(preview_);
    splitter->setStretchFactor(0, 1);
    splitter->setStretchFactor(1, 1);

    auto *layout = new QVBoxLayout(this);
    layout->addLayout(topRow);
    layout->addLayout(scopeRow);
    layout->addLayout(replaceRow);
    layout->addWidget(statusLabel_);
    layout->addWidget(splitter, 1);

    connect(queryEdit_, &QLineEdit::returnPressed, this, &SearchResultsPanel::runSearch);
    connect(regexCheck_, &QCheckBox::toggled, this, &SearchResultsPanel::runSearch);
    connect(caseCheck_, &QCheckBox::toggled, this, &SearchResultsPanel::runSearch);
    connect(wholeWordCheck_, &QCheckBox::toggled, this, &SearchResultsPanel::runSearch);
    connect(maskEdit_, &QLineEdit::returnPressed, this, &SearchResultsPanel::runSearch);
    connect(scopeCombo_, &QComboBox::currentIndexChanged, this, &SearchResultsPanel::runSearch);
    connect(replaceAllButton, &QPushButton::clicked, this, &SearchResultsPanel::replaceAll);
    connect(results_, &QTreeWidget::itemDoubleClicked, this, &SearchResultsPanel::openMatch);
    connect(results_, &QTreeWidget::currentItemChanged, this,
            &SearchResultsPanel::previewSelection);

    connect(searchModel_, &SearchModel::indexReady, this, [this]() {
        statusLabel_->setText(tr("Index ready."));
    });
    connect(searchModel_, &SearchModel::indexFailed, this, [this](const QString &message) {
        statusLabel_->setText(tr("Index build failed: %1").arg(message));
    });
    connect(searchModel_, &SearchModel::searchBatch, this, &SearchResultsPanel::appendHits);
    connect(searchModel_, &SearchModel::searchFinished, this,
            [this](quint64 generation, quint32 totalHint) {
        if (generation != generation_) {
            return;
        }
        // R8: "N of M, refine" only when the cap actually cut the scan
        // short — an uncapped search's total_hint always equals the
        // number of rows shown, so the plain count is the honest phrasing.
        const QString counts = totalHint > static_cast<quint32>(matchCount_)
          ? tr("Showing %1 of %2 match(es) — refine your search.").arg(matchCount_).arg(totalHint)
          : tr("%1 match(es).").arg(matchCount_);
        statusLabel_->setText(pendingReplaceStatus_.isEmpty()
                                ? counts
                                : pendingReplaceStatus_ + QStringLiteral(" ") + counts);
        pendingReplaceStatus_.clear();
    });
    connect(searchModel_,
            &SearchModel::searchFailed,
            this,
            [this](quint64 generation, const QString &message) {
                if (generation != generation_) {
                    return;
                }
                statusLabel_->setText(tr("Search failed: %1").arg(message));
            });
    connect(searchModel_,
            &SearchModel::replaceFinished,
            this,
            [this](quint32 files, quint32 matches, quint32 skipped) {
                pendingReplaceStatus_ =
                  (skipped == 0
                     ? tr("Replaced %1 match(es) in %2 file(s).").arg(matches).arg(files)
                     : tr("Replaced %1 match(es) in %2 file(s); %3 file(s) skipped (changed "
                          "since the search).")
                         .arg(matches)
                         .arg(files)
                         .arg(skipped));
                statusLabel_->setText(pendingReplaceStatus_);
                // The files on disk moved on; the listed spans no longer
                // describe them, so re-run rather than leave stale rows.
                runSearch();
            });
    connect(searchModel_, &SearchModel::replaceFailed, this, [this](const QString &message) {
        statusLabel_->setText(tr("Replace failed: %1").arg(message));
    });
    connect(searchModel_, &SearchModel::replacePreviewReady, this,
            &SearchResultsPanel::onReplacePreviewReady);
    connect(searchModel_, &SearchModel::replacePreviewFailed, this,
            &SearchResultsPanel::onReplacePreviewFailed);
}

void SearchResultsPanel::focusQuery()
{
    queryEdit_->setFocus();
    queryEdit_->selectAll();
}

void SearchResultsPanel::searchFor(const QString &text)
{
    queryEdit_->setText(text);
    runSearch();
}

void SearchResultsPanel::runSearch()
{
    const QString pattern = queryEdit_->text();
    if (pattern.isEmpty()) {
        return;
    }
    results_->clear();
    preview_->clearPreview();
    matchCount_ = 0;
    ++generation_;
    statusLabel_->setText(tr("Searching..."));
    FfiSearchOptions options;
    options.is_regex = regexCheck_->isChecked();
    options.case_sensitive = caseCheck_->isChecked();
    options.whole_word = wholeWordCheck_->isChecked();
    searchModel_->search(pattern, options, maskEdit_->text(), resolveScopePaths(), generation_);
}

QStringList SearchResultsPanel::resolveScopePaths() const
{
    switch (scopeCombo_->currentData().toInt()) {
    case kScopeOpenFiles:
        return editorTabs_->openPaths();
    case kScopeCurrentFile: {
        const QString path = editorTabs_->currentPath();
        return path.isEmpty() ? QStringList() : QStringList{path};
    }
    case kScopeProject:
    default:
        return QStringList();
    }
}

void SearchResultsPanel::previewSelection()
{
    QTreeWidgetItem *item = results_->currentItem();
    if (!item || item->childCount() > 0) {
        return;
    }
    preview_->showMatch(item->data(0, kPathRole).toString(), item->data(0, kLineRole).toInt(),
                        item->data(0, kStartRole).toInt(), item->data(0, kEndRole).toInt());
}

QTreeWidgetItem *SearchResultsPanel::fileGroup(const QString &path)
{
    // Matches arrive grouped by file, so the last top-level row is the right
    // group unless the file just changed — no lookup table needed.
    if (results_->topLevelItemCount() > 0) {
        QTreeWidgetItem *last = results_->topLevelItem(results_->topLevelItemCount() - 1);
        if (last->data(0, kPathRole).toString() == path) {
            return last;
        }
    }
    auto *group = new QTreeWidgetItem(results_);
    group->setData(0, kPathRole, path);
    group->setText(0, QFileInfo(path).fileName());
    group->setIcon(0, ui_shell::fileIcon(path, ui_shell::smallIconPx(results_)));
    group->setToolTip(0, path);
    group->setExpanded(true);
    return group;
}

void SearchResultsPanel::appendHits(quint64 generation, const ::rust::Vec<FfiSearchHit> &hits)
{
    if (generation != generation_) {
        return;
    }
    // Batches can be large; repainting per row is the expensive part.
    results_->setUpdatesEnabled(false);
    for (const FfiSearchHit &hit : hits) {
        const QString path = hit.path;
        QTreeWidgetItem *group = fileGroup(path);

        auto *item = new QTreeWidgetItem(group);
        const QString snippet = hit.text;
        item->setText(0, tr("%1: %2").arg(hit.line).arg(snippet));
        item->setData(0, kPathRole, path);
        item->setData(0, kLineRole, hit.line);
        item->setData(0, kStartRole, hit.start);
        item->setData(0, kEndRole, hit.end);

        // The bridge reports character offsets into the snippet; this row
        // prepends "<line>: ", so they shift by that prefix.
        const int prefix = QString::number(hit.line).size() + 2;
        QVariantList positions;
        for (quint32 offset : hit.positions) {
            positions.append(static_cast<int>(offset) + prefix);
        }
        item->setData(0, kMatchPositionsRole, positions);

        // Checked by default, so Replace All means "all of these" unless the
        // user opts individual matches out.
        item->setFlags(item->flags() | Qt::ItemIsUserCheckable);
        item->setCheckState(0, Qt::Checked);
        ++matchCount_;
    }
    results_->setUpdatesEnabled(true);
    statusLabel_->setText(tr("Searching... %1 match(es)").arg(matchCount_));
}

void SearchResultsPanel::replaceAll()
{
    QVector<PendingReplacement> edits;
    for (int g = 0; g < results_->topLevelItemCount(); ++g) {
        QTreeWidgetItem *group = results_->topLevelItem(g);
        for (int i = 0; i < group->childCount(); ++i) {
            const QTreeWidgetItem *item = group->child(i);
            if (item->checkState(0) != Qt::Checked) {
                continue;
            }
            edits.append({item->data(0, kPathRole).toString(),
                          item->data(0, kLineRole).toUInt(),
                          item->data(0, kStartRole).toUInt(),
                          item->data(0, kEndRole).toUInt()});
        }
    }
    if (edits.isEmpty()) {
        statusLabel_->setText(tr("No matches selected."));
        return;
    }

    pendingEdits_ = edits;
    pendingPattern_ = queryEdit_->text();
    pendingReplacement_ = replaceEdit_->text();
    pendingIsRegex_ = regexCheck_->isChecked();
    pendingCaseSensitive_ = caseCheck_->isChecked();

    statusLabel_->setText(tr("Preparing preview..."));
    searchModel_->previewReplacements(toFfiEdits(pendingEdits_),
                                      pendingPattern_,
                                      pendingReplacement_,
                                      pendingIsRegex_,
                                      pendingCaseSensitive_);
}

::rust::Vec<FfiFileReplacement> SearchResultsPanel::toFfiEdits(const QVector<PendingReplacement> &edits)
{
    ::rust::Vec<FfiFileReplacement> out;
    for (const PendingReplacement &edit : edits) {
        FfiFileReplacement ffi;
        ffi.path = edit.path;
        ffi.line = edit.line;
        ffi.start = edit.start;
        ffi.end = edit.end;
        out.push_back(std::move(ffi));
    }
    return out;
}

void SearchResultsPanel::onReplacePreviewReady(const QStringList &paths)
{
    if (paths.isEmpty()) {
        statusLabel_->setText(
          tr("Nothing to preview — every selected file changed since the search."));
        pendingEdits_.clear();
        return;
    }

    QList<ui_shell::RefactorPreviewDialog::Row> rows;
    for (const QString &path : paths) {
        const int changes = static_cast<int>(searchModel_->replacePreviewHunks(path).size());
        rows.append({path, 0,
                     tr("%n change(s) — replace with \"%1\"", "", changes)
                       .arg(pendingReplacement_),
                     true, true});
    }

    ui_shell::RefactorPreviewDialog::DiffProvider diffProvider =
      [this](const QString &path, QString &oldText, QString &newText,
             ::rust::Vec<FfiHunk> &hunks, ::rust::Vec<FfiInlineSpan> &spans) {
          const FfiFileDiff diff = searchModel_->replacePreviewDiff(path);
          if (diff.path.isEmpty()) {
              return false;
          }
          oldText = diff.old_text;
          newText = diff.new_text;
          hunks = searchModel_->replacePreviewHunks(path);
          spans = searchModel_->replacePreviewSpans(path);
          return true;
      };

    ui_shell::RefactorPreviewDialog dialog(
      tr("Replace in Files"),
      tr("Replace \"%1\" with \"%2\" across %3 file(s). This writes to disk and cannot be "
         "undone.")
        .arg(pendingPattern_, pendingReplacement_)
        .arg(paths.size()),
      rows,
      this,
      diffProvider);
    if (dialog.exec() != QDialog::Accepted) {
        pendingEdits_.clear();
        return;
    }

    const QStringList excluded = dialog.excludedPaths();
    QVector<PendingReplacement> finalEdits;
    for (const PendingReplacement &edit : pendingEdits_) {
        if (!excluded.contains(edit.path)) {
            finalEdits.append(edit);
        }
    }
    pendingEdits_.clear();
    if (finalEdits.isEmpty()) {
        statusLabel_->setText(tr("No matches selected."));
        return;
    }

    statusLabel_->setText(tr("Replacing..."));
    searchModel_->replaceInFiles(toFfiEdits(finalEdits),
                                 pendingPattern_,
                                 pendingReplacement_,
                                 pendingIsRegex_,
                                 pendingCaseSensitive_);
}

void SearchResultsPanel::onReplacePreviewFailed(const QString &message)
{
    statusLabel_->setText(tr("Preview failed: %1").arg(message));
    pendingEdits_.clear();
}

void SearchResultsPanel::openMatch(QTreeWidgetItem *item, int column)
{
    Q_UNUSED(column);
    if (!item || item->childCount() > 0) {
        // A file group row: expanding it is the useful action, not jumping.
        return;
    }
    openAt_(item->data(0, kPathRole).toString(),
            item->data(0, kLineRole).toInt(),
            item->data(0, kStartRole).toInt());
}
