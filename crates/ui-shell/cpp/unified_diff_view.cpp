#include "unified_diff_view.h"

#include "diff_pane.h"
#include "diff_view_page.h"
#include "theme.h"

#include <QKeySequence>
#include <QResizeEvent>
#include <QShortcut>
#include <QTextBlock>
#include <QVBoxLayout>

#include <algorithm>
#include <utility>

namespace ui_shell {

namespace {

// Same thresholds as the side-by-side view, so toggling the viewer never
// changes what is folded.
constexpr int kCollapseThreshold = 8;
constexpr int kContextLines = 2;

// `QString::split` keeps the empty piece after a trailing newline that
// `str::lines()` on the Rust side drops; the rows index Rust's lines.
QStringList linesOf(const QString &text)
{
    QStringList lines = text.split(QLatin1Char('\n'));
    if (!lines.isEmpty() && lines.last().isEmpty()) {
        lines.removeLast();
    }
    return lines;
}

} // namespace

UnifiedDiffView::UnifiedDiffView(const QString &fileName, QWidget *parent)
  : QWidget(parent)
  , fileName_(fileName)
{
    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    replacePane(QString());

    auto *nextShortcut = new QShortcut(QKeySequence(Qt::Key_F7), this);
    nextShortcut->setContext(Qt::WidgetWithChildrenShortcut);
    connect(nextShortcut, &QShortcut::activated, this, &UnifiedDiffView::selectNextHunk);
    auto *prevShortcut = new QShortcut(QKeySequence(Qt::SHIFT | Qt::Key_F7), this);
    prevShortcut->setContext(Qt::WidgetWithChildrenShortcut);
    connect(prevShortcut, &QShortcut::activated, this, &UnifiedDiffView::selectPreviousHunk);
}

// A fresh pane per text rather than `setPlainText` on the old one: the
// syntax highlighter binds to the text it is constructed over, and a
// unified rendering is rebuilt whole whenever the diff changes anyway.
void UnifiedDiffView::replacePane(const QString &text)
{
    delete pane_;
    pane_ = new DiffPane(text, fileName_, this);
    layout()->addWidget(pane_);
    connect(pane_, &QPlainTextEdit::updateRequest, this,
            [this](const QRect &, int) { repositionFoldHints(); });
}

void UnifiedDiffView::setContent(const DiffData &data)
{
    leftLines_ = linesOf(data.leftText);
    rightLines_ = linesOf(data.rightText);
    rows_.clear();
    for (const FfiDiffRow &row : data.rows) {
        rows_.append(row);
    }
    hunks_.clear();
    for (const FfiHunk &hunk : data.hunks) {
        hunks_.append(hunk);
    }
    spans_.clear();
    for (const FfiInlineSpan &span : data.spans) {
        spans_.append(span);
    }
    currentHunk_ = -1;
    rebuild();
}

void UnifiedDiffView::setOptions(bool collapseUnchanged, FfiHighlightMode highlight)
{
    collapseUnchanged_ = collapseUnchanged;
    paintBackgrounds_ = highlight != FfiHighlightMode::None;
    applySelections();
    recomputeCollapsedRuns();
}

void UnifiedDiffView::rebuild()
{
    expandAllRuns();

    QStringList text;
    QVector<QString> labels;
    text.reserve(rows_.size());
    labels.reserve(rows_.size());
    blockOfOld_ = QVector<int>(leftLines_.size(), -1);
    blockOfNew_ = QVector<int>(rightLines_.size(), -1);
    const int digits = QString::number(std::max(leftLines_.size(), rightLines_.size())).size();
    for (int i = 0; i < rows_.size(); ++i) {
        const FfiDiffRow &row = rows_[i];
        const bool hasOld = row.old_line >= 0 && row.old_line < leftLines_.size();
        const bool hasNew = row.new_line >= 0 && row.new_line < rightLines_.size();
        text.append(hasNew ? rightLines_[row.new_line] : hasOld ? leftLines_[row.old_line] : QString());
        const QString oldLabel = hasOld ? QString::number(row.old_line + 1) : QString();
        const QString newLabel = hasNew ? QString::number(row.new_line + 1) : QString();
        labels.append(QStringLiteral("%1  %2")
                        .arg(oldLabel, digits, QLatin1Char(' '))
                        .arg(newLabel, digits, QLatin1Char(' ')));
        if (hasOld) {
            blockOfOld_[row.old_line] = i;
        }
        if (hasNew) {
            blockOfNew_[row.new_line] = i;
        }
    }
    replacePane(text.join(QLatin1Char('\n')));
    pane_->setGutterLabels(labels);

    // A hunk's blocks: its removed rows come first, its added rows last.
    hunkBlocks_.clear();
    for (const FfiHunk &hunk : hunks_) {
        int first = -1;
        int last = -1;
        auto note = [&first, &last](int block) {
            if (block < 0) {
                return;
            }
            first = first < 0 ? block : std::min(first, block);
            last = std::max(last, block);
        };
        for (quint32 k = 0; k < hunk.old_len; ++k) {
            note(blockOfOld_.value(static_cast<int>(hunk.old_start + k), -1));
        }
        for (quint32 k = 0; k < hunk.new_len; ++k) {
            note(blockOfNew_.value(static_cast<int>(hunk.new_start + k), -1));
        }
        hunkBlocks_.append({first, last});
    }

    applySelections();
    recomputeCollapsedRuns();
}

void UnifiedDiffView::applySelections()
{
    QVector<DiffLineBackground> backgrounds;
    QVector<DiffInlineSpan> inlineSpans;
    if (paintBackgrounds_) {
        const DiffColors colors = diffColors();
        for (int i = 0; i < rows_.size(); ++i) {
            switch (rows_[i].kind) {
            case FfiRowKind::Removed:
                backgrounds.append({i, colors.deletedLine});
                break;
            case FfiRowKind::Added:
                backgrounds.append({i, colors.addedLine});
                break;
            case FfiRowKind::Context:
            default:
                break;
            }
        }
        for (const FfiInlineSpan &span : spans_) {
            const int line = static_cast<int>(span.line);
            const int block = span.side == FfiDiffSide::Old ? blockOfOld_.value(line, -1)
                                                             : blockOfNew_.value(line, -1);
            if (block >= 0) {
                inlineSpans.append({block, static_cast<int>(span.start),
                                    static_cast<int>(span.end), colors.modifiedInline});
            }
        }
    }
    pane_->setDiffSelections(backgrounds, inlineSpans);
}

void UnifiedDiffView::recomputeCollapsedRuns()
{
    expandAllRuns();
    if (!collapseUnchanged_) {
        return;
    }
    auto considerRun = [this](int start, int end, bool atFileStart, bool atFileEnd) {
        const int leading = atFileStart ? 0 : kContextLines;
        const int trailing = atFileEnd ? 0 : kContextLines;
        const int collapsed = end - start - leading - trailing;
        if (collapsed < kCollapseThreshold) {
            return;
        }
        CollapsedRun run{start + leading, end - trailing, nullptr};
        setLinesVisible(pane_, run.start, run.endExclusive - 1, false);
        run.hint = new FoldHint(collapsed, pane_->viewport());
        connect(static_cast<QPushButton *>(run.hint), &QPushButton::clicked, this,
                [this, hint = run.hint] { expandRunWithHint(hint); });
        run.hint->show();
        runs_.append(run);
    };
    int cursor = 0;
    for (int i = 0; i < hunkBlocks_.size(); ++i) {
        const auto [first, last] = hunkBlocks_[i];
        if (first < 0) {
            continue;
        }
        considerRun(cursor, first, i == 0, false);
        cursor = last + 1;
    }
    considerRun(cursor, rows_.size(), hunkBlocks_.isEmpty(), true);
    repositionFoldHints();
}

void UnifiedDiffView::expandAllRuns()
{
    // Taken out first: showing lines re-enters `repositionFoldHints`
    // synchronously, which must not walk a hint this loop deleted.
    const QVector<CollapsedRun> runs = std::exchange(runs_, {});
    for (const CollapsedRun &run : runs) {
        setLinesVisible(pane_, run.start, run.endExclusive - 1, true);
        delete run.hint;
    }
}

void UnifiedDiffView::expandRunWithHint(QWidget *hint)
{
    const int index =
      std::find_if(runs_.begin(), runs_.end(),
                   [hint](const CollapsedRun &run) { return run.hint == hint; })
      - runs_.begin();
    if (index >= runs_.size()) {
        return;
    }
    const CollapsedRun run = runs_[index];
    runs_.remove(index);
    setLinesVisible(pane_, run.start, run.endExclusive - 1, true);
    delete run.hint;
}

void UnifiedDiffView::repositionFoldHints()
{
    for (const CollapsedRun &run : runs_) {
        const QTextBlock block = pane_->document()->findBlockByNumber(run.start);
        if (!run.hint || !block.isValid()) {
            continue;
        }
        const QRect rect = pane_->cursorRect(QTextCursor(block));
        run.hint->setGeometry(0, rect.top(), pane_->viewport()->width(), rect.height());
    }
}

void UnifiedDiffView::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    repositionFoldHints();
}

void UnifiedDiffView::selectHunk(int index)
{
    if (hunkBlocks_.isEmpty()) {
        return;
    }
    currentHunk_ = ((index % hunkBlocks_.size()) + hunkBlocks_.size()) % hunkBlocks_.size();
    const auto [first, last] = hunkBlocks_[currentHunk_];
    if (first < 0) {
        return;
    }
    const QTextBlock startBlock = pane_->document()->findBlockByNumber(first);
    const QTextBlock endBlock = pane_->document()->findBlockByNumber(last);
    QTextCursor cursor(startBlock);
    cursor.setPosition(endBlock.position() + std::max(0, endBlock.length() - 1),
                       QTextCursor::KeepAnchor);
    pane_->setTextCursor(cursor);
    pane_->centerCursor();
}

void UnifiedDiffView::selectNextHunk()
{
    selectHunk(currentHunk_ + 1);
}

void UnifiedDiffView::selectPreviousHunk()
{
    selectHunk(currentHunk_ - 1);
}

} // namespace ui_shell
