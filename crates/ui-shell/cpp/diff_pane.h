#pragma once

#include "diff_selections.h"

#include <QPlainTextEdit>
#include <QPushButton>
#include <QString>
#include <QVector>

namespace ui_shell {

// --- Helpers that work on any `QPlainTextEdit` -------------------------
//
// The diff views have two kinds of pane: their own read-only `DiffPane`s,
// and — in the editable HEAD-vs-working-tree window — the tab's live
// `CodeEditor`, reparented in as-is. Anything that must treat both alike
// lives here as a free function over the base class.

// Hide or show the blocks strictly after `fromExclusive` up to and
// including `toInclusive` — the same "keep the header line, hide the rest,
// mark the document dirty over that range" technique `CodeEditor`'s code
// folding uses.
void setLinesVisible(QPlainTextEdit *edit, int fromExclusive, int toInclusive, bool visible);

// The y of `block`'s top edge in `edit`'s viewport coordinates — real
// layout geometry, so it tracks scrolling and collapsed (hidden) blocks
// cost 0 px. Public-API only (`cursorRect`), which is what lets it work on
// a `CodeEditor` too.
int blockTopIn(const QPlainTextEdit *edit, int block);

// The first and last block with any part inside `edit`'s viewport.
int firstVisibleBlockIn(const QPlainTextEdit *edit);
int lastVisibleBlockIn(const QPlainTextEdit *edit);

// A small "N unchanged lines" button floated over a pane's viewport at a
// collapsed gap's header line. Whoever collapses the gap positions it.
class FoldHint : public QPushButton
{
public:
    FoldHint(int lineCount, QWidget *viewport);
};

// --- The read-only pane --------------------------------------------------

// One side of a diff: read-only, unwrapped, monospace, syntax-highlighted
// when a file name says which language, with a line-number gutter whose
// labels the owner supplies — one number per block side by side, an
// "old   new" pair in the unified viewer. Deliberately not a `CodeEditor`:
// that gutter sets breakpoints, folds and shows blame, none of which a
// static revision text wants, and it cannot show two number columns.
class DiffPane : public QPlainTextEdit
{
public:
    DiffPane(const QString &text, const QString &fileName, QWidget *parent = nullptr);

    // One label per block. An empty vector means plain 1-based numbers.
    void setGutterLabels(const QVector<QString> &labels);

    void setDiffSelections(const QVector<DiffLineBackground> &backgrounds,
                           const QVector<DiffInlineSpan> &spans);

    int gutterWidth() const;
    void paintGutter(QPaintEvent *event);

protected:
    void resizeEvent(QResizeEvent *event) override;

private:
    class Gutter;

    QString labelFor(int block) const;
    void updateGutterWidth();

    Gutter *gutter_ = nullptr;
    QVector<QString> labels_;
};

} // namespace ui_shell
