#pragma once

#include "diff_divider.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QVector>
#include <QWidget>

class QPlainTextEdit;
class QResizeEvent;
class QSplitter;

namespace ui_shell {

class DiffPane;

// Two panes over a before/after text, JetBrains-style: a line-number
// gutter on each, full-width backgrounds on changed lines with a stronger
// shade over the words that changed, a divider joining each hunk's two
// sides from real layout geometry, scrolling that keeps corresponding
// lines level, collapsible unchanged regions and F7 / Shift+F7 navigation.
//
// Reusable and Git-free by design (ADR-0028): it knows nothing about where
// its two texts came from. Everything it paints — which lines changed, what
// changed within a line, which line sits level with which — is decided in
// `editor_core::diff` and crosses the seam as `hunks`/`spans`/`rows`; this
// class lays them out and nothing more, so it stays untested by design
// (`CLAUDE.md`: "C++ stays thin").
//
// `fileName` names the file to detect a language from — empty means plain
// monospace text.
class DiffView : public QWidget
{
    Q_OBJECT

public:
    // Two static, read-only texts (File History's "compare revisions",
    // Project Tree's "Compare with…", the refactor/replace previews).
    DiffView(const QString &leftText,
             const QString &rightText,
             const ::rust::Vec<FfiHunk> &hunks,
             const ::rust::Vec<FfiInlineSpan> &spans,
             const QString &fileName,
             QWidget *parent = nullptr);

    // The left side is a static HEAD/revision text; the right side is an
    // already-live, already-editable text widget (a real `CodeEditor`)
    // reparented in as-is — never re-created, never forced read-only. This
    // is what lets `vcs.showDiff`'s diff *mode* sit around the same
    // `Document`/undo stack a tab already has (ADR-0003) rather than a
    // second copy. Call `releaseRightPane()` before destroying this widget
    // if `rightPane` must outlive it (the normal case: toggling diff mode
    // off hands the editor back to its plain page).
    DiffView(const QString &leftText,
             QPlainTextEdit *rightPane,
             const ::rust::Vec<FfiHunk> &hunks,
             const ::rust::Vec<FfiInlineSpan> &spans,
             const QString &fileName,
             QWidget *parent = nullptr);

    // Reparents the right pane back out to `nullptr` and returns it, for a
    // caller to put back where it came from. Only meaningful for the
    // external-right-pane constructor; returns `nullptr` otherwise. Must be
    // called before this widget is destroyed, or the external pane is
    // destroyed along with it like any other Qt child.
    QPlainTextEdit *releaseRightPane();

    // Replace the diff in place — a toolbar option or a live edit both
    // recompute elsewhere and hand the new set back here rather than
    // rebuilding the whole widget. `rows` may be empty (the refactor and
    // replace previews pass none), in which case the two panes scroll by
    // fraction of their own length instead of by corresponding line.
    void setDiff(const ::rust::Vec<FfiHunk> &hunks,
                 const ::rust::Vec<FfiInlineSpan> &spans,
                 const ::rust::Vec<FfiDiffRow> &rows);

    // The view-side switches the toolbar owns. `highlight` only matters as
    // `None` here (no backgrounds at all); the spans already reflect it.
    void setOptions(bool collapseUnchanged, bool syncScroll, FfiHighlightMode highlight);

    // Jump to the next/previous hunk: scrolls it into view and selects its
    // lines on both panes. Wraps at either end. Wired to F7/Shift+F7 as
    // shortcuts on this widget, and exposed here so a host toolbar can put
    // them on buttons too.
    void selectNextHunk();
    void selectPreviousHunk();
    int hunkCount() const { return hunks_.size(); }

    QPlainTextEdit *leftPane() const;
    QPlainTextEdit *rightPane() const { return rightEdit_; }
    DiffDivider *divider() const { return divider_; }

private:
    using Hunk = DiffDivider::Hunk;

    struct Span
    {
        FfiDiffSide side;
        int line;
        int start;
        int end;
    };

    // A run of unchanged lines collapsed between two hunks (or before the
    // first / after the last). The first line of each range stays visible
    // as the host row the hint covers — a fold needs one row of height to
    // stand in for what it hides — and the rest are hidden.
    // `leftHint`/`rightHint` are only non-null while the gap is collapsed —
    // expanding deletes them.
    struct CollapsedGap
    {
        int leftStart;
        int leftEndExclusive;
        int rightStart;
        int rightEndExclusive;
        QWidget *leftHint = nullptr;
        QWidget *rightHint = nullptr;
    };

    // Shared tail of both constructors: `rightEdit_`/`ownsRightEdit_` must
    // already be set.
    void init(const QString &leftText,
              const ::rust::Vec<FfiHunk> &hunks,
              const ::rust::Vec<FfiInlineSpan> &spans,
              const QString &fileName);
    void selectHunk(int index);
    void applySelections();
    void syncScrollFrom(QPlainTextEdit *from, QPlainTextEdit *to, bool fromIsLeft);
    void recomputeCollapsedGaps();
    void expandAllGaps();
    // Finds the gap owning `hint` (either side) and expands it. Identified
    // by the clicked widget rather than an index: `gaps_` reshuffles every
    // time an earlier gap expands, and an index captured at connect-time
    // would silently point at the wrong gap afterwards.
    void expandGapWithHint(QWidget *hint);
    void repositionFoldHints();

    DiffPane *leftPane_ = nullptr;
    QPlainTextEdit *rightEdit_ = nullptr;
    DiffDivider *divider_ = nullptr;
    QSplitter *splitter_ = nullptr;
    bool splitterDragged_ = false;
    bool ownsRightEdit_ = true;
    QVector<Hunk> hunks_;
    QVector<Span> spans_;
    // `rowOfOld_[line]` / `rowOfNew_[line]` index `rows_`; all three empty
    // when no rows were supplied.
    QVector<FfiDiffRow> rows_;
    QVector<int> rowOfOld_;
    QVector<int> rowOfNew_;
    QVector<CollapsedGap> gaps_;
    int currentHunk_ = -1;
    bool syncingScroll_ = false;
    bool collapseUnchanged_ = true;
    bool syncScroll_ = true;
    bool paintBackgrounds_ = true;

protected:
    void resizeEvent(QResizeEvent *event) override;
};

} // namespace ui_shell
