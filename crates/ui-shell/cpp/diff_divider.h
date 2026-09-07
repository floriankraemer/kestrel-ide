#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QRect>
#include <QVector>
#include <QWidget>

#include <functional>

class QPlainTextEdit;
class QToolButton;

namespace ui_shell {

// The strip between a side-by-side diff's two panes: for every hunk, a
// filled shape joining the hunk's lines on the left to its lines on the
// right — JetBrains' own diff signature — painted from each pane's real
// layout geometry, so it tracks scrolling, collapsed regions and different
// line counts exactly. A side with no lines (a pure insertion or deletion)
// collapses to a line at the point where the change happened.
//
// When the right pane is editable (`setEditable(true)`), each hunk also
// carries a chevron that replaces its right-side lines with the left
// side's. One button rather than JetBrains' apply-and-revert pair: with only
// the right side editable, "apply the left" and "revert the right" are the
// same edit.
class DiffDivider : public QWidget
{
    Q_OBJECT

public:
    struct Hunk
    {
        int oldStart;
        int oldLen;
        int newStart;
        int newLen;
        FfiHunkKind kind;
    };

    DiffDivider(QPlainTextEdit *leftEdit, QPlainTextEdit *rightEdit, QWidget *parent = nullptr);

    void setHunks(const QVector<Hunk> &hunks);
    void setEditable(bool editable);

    // Called with the hunk's index in the last `setHunks` when its chevron
    // is clicked. Only ever fires while editable.
    std::function<void(int)> onApplyHunk;

    static constexpr int kWidth = 36;

protected:
    void paintEvent(QPaintEvent *event) override;

private:
    // `blockTopIn` answers in the pane's viewport coordinates; the divider
    // paints in its own, and the two are siblings rather than nested.
    int paneYToLocal(const QPlainTextEdit *edit, int viewportY) const;
    void placeChevron(int index, int y, bool visible);

    QPlainTextEdit *leftEdit_;
    QPlainTextEdit *rightEdit_;
    QVector<Hunk> hunks_;
    QVector<QToolButton *> chevrons_;
    QHash<QToolButton *, QRect> markedRects_;
    bool editable_ = false;
};

} // namespace ui_shell
