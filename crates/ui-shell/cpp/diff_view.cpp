#include "diff_view.h"

#include "code_editor.h"
#include "diff_pane.h"
#include "theme.h"

#include <QHBoxLayout>
#include <QKeySequence>
#include <QPlainTextEdit>
#include <QResizeEvent>
#include <QScrollBar>
#include <QShortcut>
#include <QSplitter>
#include <QTextBlock>

#include <algorithm>
#include <utility>

namespace ui_shell {

namespace {

int totalLines(const QPlainTextEdit *edit)
{
    return std::max(1, edit->document()->blockCount());
}

// A run of unchanged lines longer than this collapses by default. Short
// enough that two nearby hunks still read as "one screen", long enough that
// collapsing a three-line gap wouldn't just be noise.
constexpr int kCollapseThreshold = 8;

// Unchanged lines kept visible on each side of a hunk when the run between
// two hunks collapses — the context a reader needs to place the change.
constexpr int kContextLines = 2;

// Paint a diff onto whichever kind of pane this is: a `DiffPane` owns its
// extra selections outright, a live `CodeEditor` merges them with its own
// (current line, find matches, occurrences).
void paintDiffOn(QPlainTextEdit *edit,
                 const QVector<DiffLineBackground> &backgrounds,
                 const QVector<DiffInlineSpan> &spans)
{
    if (auto *pane = dynamic_cast<DiffPane *>(edit)) {
        pane->setDiffSelections(backgrounds, spans);
    } else if (auto *editor = qobject_cast<CodeEditor *>(edit)) {
        editor->setDiffSelections(backgrounds, spans);
    }
}

} // namespace

DiffView::DiffView(const QString &leftText,
                   const QString &rightText,
                   const ::rust::Vec<FfiHunk> &hunks,
                   const ::rust::Vec<FfiInlineSpan> &spans,
                   const QString &fileName,
                   QWidget *parent)
  : QWidget(parent)
{
    rightEdit_ = new DiffPane(rightText, fileName, this);
    ownsRightEdit_ = true;
    init(leftText, hunks, spans, fileName);
}

DiffView::DiffView(const QString &leftText,
                   QPlainTextEdit *rightPane,
                   const ::rust::Vec<FfiHunk> &hunks,
                   const ::rust::Vec<FfiInlineSpan> &spans,
                   const QString &fileName,
                   QWidget *parent)
  : QWidget(parent)
{
    rightEdit_ = rightPane;
    ownsRightEdit_ = false;
    init(leftText, hunks, spans, fileName);
}

QPlainTextEdit *DiffView::releaseRightPane()
{
    if (ownsRightEdit_ || !rightEdit_) {
        return nullptr;
    }
    expandAllGaps();
    paintDiffOn(rightEdit_, {}, {});
    QPlainTextEdit *released = rightEdit_;
    released->setParent(nullptr);
    rightEdit_ = nullptr;
    return released;
}

QPlainTextEdit *DiffView::leftPane() const
{
    return leftPane_;
}

void DiffView::init(const QString &leftText,
                    const ::rust::Vec<FfiHunk> &hunks,
                    const ::rust::Vec<FfiInlineSpan> &spans,
                    const QString &fileName)
{
    leftPane_ = new DiffPane(leftText, fileName, this);
    divider_ = new DiffDivider(leftPane_, rightEdit_, this);

    splitter_ = new QSplitter(this);
    splitter_->setHandleWidth(0);
    splitter_->addWidget(leftPane_);
    splitter_->addWidget(divider_);
    splitter_->addWidget(rightEdit_);
    // A reparented editor arrives hidden — its tab stack hid it explicitly
    // when it stopped being the current page — and a hidden splitter child
    // gets no width at all.
    rightEdit_->show();
    splitter_->setStretchFactor(0, 1);
    splitter_->setStretchFactor(1, 0);
    splitter_->setStretchFactor(2, 1);
    splitter_->setCollapsible(1, false);
    connect(splitter_, &QSplitter::splitterMoved, this, [this] { splitterDragged_ = true; });

    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->addWidget(splitter_);

    connect(leftPane_->verticalScrollBar(), &QScrollBar::valueChanged, this, [this](int) {
        syncScrollFrom(leftPane_, rightEdit_, /*fromIsLeft=*/true);
        divider_->update();
    });
    connect(rightEdit_->verticalScrollBar(), &QScrollBar::valueChanged, this, [this](int) {
        syncScrollFrom(rightEdit_, leftPane_, /*fromIsLeft=*/false);
        divider_->update();
    });
    connect(leftPane_, &QPlainTextEdit::updateRequest, this, [this](const QRect &, int) {
        repositionFoldHints();
        divider_->update();
    });
    connect(rightEdit_, &QPlainTextEdit::updateRequest, this, [this](const QRect &, int) {
        repositionFoldHints();
        divider_->update();
    });

    setDiff(hunks, spans, ::rust::Vec<FfiDiffRow>());

    auto *nextShortcut = new QShortcut(QKeySequence(Qt::Key_F7), this);
    nextShortcut->setContext(Qt::WidgetWithChildrenShortcut);
    connect(nextShortcut, &QShortcut::activated, this, &DiffView::selectNextHunk);
    auto *prevShortcut = new QShortcut(QKeySequence(Qt::SHIFT | Qt::Key_F7), this);
    prevShortcut->setContext(Qt::WidgetWithChildrenShortcut);
    connect(prevShortcut, &QShortcut::activated, this, &DiffView::selectPreviousHunk);
}

void DiffView::setDiff(const ::rust::Vec<FfiHunk> &hunks,
                       const ::rust::Vec<FfiInlineSpan> &spans,
                       const ::rust::Vec<FfiDiffRow> &rows)
{
    // Undo any fold before rebuilding: a stale hidden range from the old
    // hunk set could otherwise hide lines with no gap left to explain why.
    expandAllGaps();

    hunks_.clear();
    for (const FfiHunk &h : hunks) {
        hunks_.append(Hunk{static_cast<int>(h.old_start), static_cast<int>(h.old_len),
                           static_cast<int>(h.new_start), static_cast<int>(h.new_len), h.kind});
    }
    spans_.clear();
    for (const FfiInlineSpan &s : spans) {
        spans_.append(
          Span{s.side, static_cast<int>(s.line), static_cast<int>(s.start), static_cast<int>(s.end)});
    }
    rows_.clear();
    rowOfOld_.clear();
    rowOfNew_.clear();
    rows_.reserve(static_cast<int>(rows.size()));
    for (const FfiDiffRow &row : rows) {
        const int index = rows_.size();
        rows_.append(row);
        if (row.old_line >= 0) {
            rowOfOld_.append(index);
        }
        if (row.new_line >= 0) {
            rowOfNew_.append(index);
        }
    }
    currentHunk_ = -1;

    divider_->setHunks(hunks_);
    applySelections();
    recomputeCollapsedGaps();
}

void DiffView::setOptions(bool collapseUnchanged, bool syncScroll, FfiHighlightMode highlight)
{
    collapseUnchanged_ = collapseUnchanged;
    syncScroll_ = syncScroll;
    paintBackgrounds_ = highlight != FfiHighlightMode::None;
    applySelections();
    recomputeCollapsedGaps();
}

void DiffView::applySelections()
{
    QVector<DiffLineBackground> leftBackgrounds;
    QVector<DiffLineBackground> rightBackgrounds;
    QVector<DiffInlineSpan> leftSpans;
    QVector<DiffInlineSpan> rightSpans;
    if (paintBackgrounds_) {
        const DiffColors colors = diffColors();
        for (const Hunk &hunk : hunks_) {
            const QColor leftColor =
              hunk.kind == FfiHunkKind::Removed ? colors.deletedLine : colors.modifiedLine;
            const QColor rightColor =
              hunk.kind == FfiHunkKind::Added ? colors.addedLine : colors.modifiedLine;
            for (int i = 0; i < hunk.oldLen; ++i) {
                leftBackgrounds.append({hunk.oldStart + i, leftColor});
            }
            for (int i = 0; i < hunk.newLen; ++i) {
                rightBackgrounds.append({hunk.newStart + i, rightColor});
            }
        }
        // Inline spans only exist for modified hunks, so both sides take the
        // modified shade — a renamed word reads as "changed", not as "this
        // side lost it, that side gained it".
        for (const Span &span : spans_) {
            const DiffInlineSpan inlineSpan{span.line, span.start, span.end, colors.modifiedInline};
            (span.side == FfiDiffSide::Old ? leftSpans : rightSpans).append(inlineSpan);
        }
    }
    paintDiffOn(leftPane_, leftBackgrounds, leftSpans);
    paintDiffOn(rightEdit_, rightBackgrounds, rightSpans);
}

void DiffView::syncScrollFrom(QPlainTextEdit *from, QPlainTextEdit *to, bool fromIsLeft)
{
    if (syncingScroll_ || !syncScroll_ || !to) {
        return;
    }
    syncingScroll_ = true;
    if (rows_.isEmpty()) {
        // No alignment model: by fraction of each pane's own range, since
        // the two sides routinely have different line counts.
        QScrollBar *fromBar = from->verticalScrollBar();
        QScrollBar *toBar = to->verticalScrollBar();
        const qreal fraction =
          fromBar->maximum() > 0 ? static_cast<qreal>(fromBar->value()) / fromBar->maximum() : 0.0;
        toBar->setValue(static_cast<int>(fraction * toBar->maximum()));
    } else {
        const int topBlock = firstVisibleBlockIn(from);
        const QVector<int> &rowOf = fromIsLeft ? rowOfOld_ : rowOfNew_;
        if (topBlock >= 0 && topBlock < rowOf.size()) {
            const FfiDiffRow &row = rows_[rowOf[topBlock]];
            const int target = static_cast<int>(fromIsLeft ? row.new_anchor : row.old_anchor);
            // A `QPlainTextEdit` scrolls in visible lines, which is what
            // `firstLineNumber()` counts (hidden, collapsed blocks included).
            const QTextBlock block = to->document()->findBlockByNumber(target);
            if (block.isValid()) {
                to->verticalScrollBar()->setValue(block.firstLineNumber());
            }
        }
    }
    syncingScroll_ = false;
}

void DiffView::recomputeCollapsedGaps()
{
    expandAllGaps();
    if (!collapseUnchanged_) {
        return;
    }

    // Sorted ascending by construction (`editor_core::diff::diff_lines`'s
    // own invariant, proven by `hunks_are_ascending_and_do_not_overlap`).
    // Each unchanged run keeps `kContextLines` visible next to the hunk on
    // either side of it (none at the start or end of the file) and collapses
    // what is left when that is still long enough to be worth it.
    int leftCursor = 0;
    int rightCursor = 0;
    auto considerGap = [this](int leftStart, int leftEnd, int rightStart, int rightEnd,
                              bool atFileStart, bool atFileEnd) {
        const int leading = atFileStart ? 0 : kContextLines;
        const int trailing = atFileEnd ? 0 : kContextLines;
        const int collapsed = leftEnd - leftStart - leading - trailing;
        if (collapsed < kCollapseThreshold) {
            return;
        }
        CollapsedGap gap{leftStart + leading, leftEnd - trailing, rightStart + leading,
                         rightEnd - trailing, nullptr, nullptr};
        setLinesVisible(leftPane_, gap.leftStart, gap.leftEndExclusive - 1, false);
        setLinesVisible(rightEdit_, gap.rightStart, gap.rightEndExclusive - 1, false);
        gap.leftHint = new FoldHint(collapsed, leftPane_->viewport());
        gap.rightHint = new FoldHint(collapsed, rightEdit_->viewport());
        connect(static_cast<QPushButton *>(gap.leftHint), &QPushButton::clicked, this,
                [this, hint = gap.leftHint] { expandGapWithHint(hint); });
        connect(static_cast<QPushButton *>(gap.rightHint), &QPushButton::clicked, this,
                [this, hint = gap.rightHint] { expandGapWithHint(hint); });
        gap.leftHint->show();
        gap.rightHint->show();
        gaps_.append(gap);
    };
    for (int i = 0; i < hunks_.size(); ++i) {
        const Hunk &hunk = hunks_[i];
        considerGap(leftCursor, hunk.oldStart, rightCursor, hunk.newStart, i == 0, false);
        leftCursor = hunk.oldStart + hunk.oldLen;
        rightCursor = hunk.newStart + hunk.newLen;
    }
    considerGap(leftCursor, totalLines(leftPane_), rightCursor, totalLines(rightEdit_),
                hunks_.isEmpty(), true);

    repositionFoldHints();
}

void DiffView::expandAllGaps()
{
    // Taken out of `gaps_` first: showing lines marks the document dirty,
    // which re-enters `repositionFoldHints` synchronously, and that must not
    // walk over a hint this loop has already deleted.
    const QVector<CollapsedGap> gaps = std::exchange(gaps_, {});
    for (const CollapsedGap &gap : gaps) {
        if (gap.leftHint) {
            setLinesVisible(leftPane_, gap.leftStart, gap.leftEndExclusive - 1, true);
            if (rightEdit_) {
                setLinesVisible(rightEdit_, gap.rightStart, gap.rightEndExclusive - 1, true);
            }
        }
        delete gap.leftHint;
        delete gap.rightHint;
    }
}

void DiffView::expandGapWithHint(QWidget *hint)
{
    const int index = std::find_if(gaps_.begin(), gaps_.end(),
                                   [hint](const CollapsedGap &gap) {
                                       return gap.leftHint == hint || gap.rightHint == hint;
                                   })
      - gaps_.begin();
    if (index >= gaps_.size()) {
        return;
    }
    const CollapsedGap gap = gaps_[index];
    gaps_.remove(index);
    setLinesVisible(leftPane_, gap.leftStart, gap.leftEndExclusive - 1, true);
    setLinesVisible(rightEdit_, gap.rightStart, gap.rightEndExclusive - 1, true);
    delete gap.leftHint;
    delete gap.rightHint;
}

void DiffView::repositionFoldHints()
{
    for (const CollapsedGap &gap : gaps_) {
        if (!gap.leftHint || !gap.rightHint) {
            continue;
        }
        // The hint covers the gap's first line — the one row of the collapsed
        // run that stays visible — edge to edge, so it reads as a fold
        // placeholder rather than a label floating over code.
        auto place = [](QPlainTextEdit *edit, QWidget *hint, int hostBlock) {
            const QTextBlock block = edit->document()->findBlockByNumber(hostBlock);
            if (!block.isValid()) {
                return;
            }
            const QRect rect = edit->cursorRect(QTextCursor(block));
            hint->setGeometry(0, rect.top(), edit->viewport()->width(), rect.height());
        };
        place(leftPane_, gap.leftHint, gap.leftStart);
        place(rightEdit_, gap.rightHint, gap.rightStart);
    }
}

void DiffView::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    // Equal halves until the user drags the divider: a splitter otherwise
    // splits by size hint, and a live `CodeEditor`'s is nothing like a
    // `DiffPane`'s.
    if (!splitterDragged_) {
        const int half = (width() - DiffDivider::kWidth) / 2;
        splitter_->setSizes({half, DiffDivider::kWidth, half});
    }
    repositionFoldHints();
}

void DiffView::selectHunk(int index)
{
    if (hunks_.isEmpty()) {
        return;
    }
    currentHunk_ = ((index % hunks_.size()) + hunks_.size()) % hunks_.size();
    const Hunk &hunk = hunks_[currentHunk_];

    auto selectRange = [](QPlainTextEdit *edit, int start, int len) {
        const int blockCount = edit->document()->blockCount();
        const int from = std::min(start, blockCount - 1);
        const int to = std::min(start + std::max(len, 1) - 1, blockCount - 1);
        const QTextBlock startBlock = edit->document()->findBlockByNumber(from);
        const QTextBlock endBlock = edit->document()->findBlockByNumber(to);
        QTextCursor cursor(startBlock);
        cursor.setPosition(endBlock.position() + std::max(0, endBlock.length() - 1),
                           QTextCursor::KeepAnchor);
        edit->setTextCursor(cursor);
        edit->centerCursor();
    };
    selectRange(leftPane_, hunk.oldStart, hunk.oldLen);
    selectRange(rightEdit_, hunk.newStart, hunk.newLen);
}

void DiffView::selectNextHunk()
{
    selectHunk(currentHunk_ + 1);
}

void DiffView::selectPreviousHunk()
{
    selectHunk(currentHunk_ - 1);
}

} // namespace ui_shell
