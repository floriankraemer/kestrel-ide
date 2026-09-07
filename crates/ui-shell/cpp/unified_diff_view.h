#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QPair>
#include <QVector>
#include <QWidget>

namespace ui_shell {

class DiffPane;
struct DiffData;

// JetBrains' unified viewer: one pane showing `editor_core::diff`'s aligned
// rows top to bottom — context, then each hunk's removed lines (grey)
// followed by its added lines (green), modified pairs with the inline shade
// over the words that changed — with an "old | new" line-number gutter,
// collapsible unchanged runs and the same F7 / Shift+F7 navigation as the
// side-by-side view.
//
// Always read-only, even over the editable HEAD-vs-working-tree window:
// switching back to side-by-side returns the live editor. The rows decide
// everything shown here; this widget only lays a row per line.
class UnifiedDiffView : public QWidget
{
    Q_OBJECT

public:
    explicit UnifiedDiffView(const QString &fileName, QWidget *parent = nullptr);

    void setContent(const DiffData &data);
    void setOptions(bool collapseUnchanged, FfiHighlightMode highlight);

    void selectNextHunk();
    void selectPreviousHunk();

private:
    struct CollapsedRun
    {
        int start;
        int endExclusive;
        QWidget *hint = nullptr;
    };

    void replacePane(const QString &text);
    void rebuild();
    void applySelections();
    void recomputeCollapsedRuns();
    void expandAllRuns();
    void expandRunWithHint(QWidget *hint);
    void repositionFoldHints();
    void selectHunk(int index);

    DiffPane *pane_ = nullptr;
    QString fileName_;
    QVector<FfiDiffRow> rows_;
    QVector<FfiHunk> hunks_;
    QVector<FfiInlineSpan> spans_;
    QStringList leftLines_;
    QStringList rightLines_;
    // `blockOfOld_[line]` / `blockOfNew_[line]`: which pane block shows it.
    QVector<int> blockOfOld_;
    QVector<int> blockOfNew_;
    // Each hunk's first and last block in the pane, for navigation.
    QVector<QPair<int, int>> hunkBlocks_;
    QVector<CollapsedRun> runs_;
    int currentHunk_ = -1;
    bool collapseUnchanged_ = true;
    bool paintBackgrounds_ = true;

protected:
    void resizeEvent(QResizeEvent *event) override;
};

} // namespace ui_shell
