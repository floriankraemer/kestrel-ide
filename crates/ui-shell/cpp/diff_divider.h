#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QVector>
#include <QWidget>

class QPlainTextEdit;

namespace ui_shell {

// The strip between a side-by-side diff's two panes: for every hunk, a
// filled shape joining the hunk's lines on the left to its lines on the
// right — JetBrains' own diff signature — painted from each pane's real
// layout geometry, so it tracks scrolling, collapsed regions and different
// line counts exactly. A side with no lines (a pure insertion or deletion)
// collapses to a line at the point where the change happened.
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

    static constexpr int kWidth = 36;

protected:
    void paintEvent(QPaintEvent *event) override;

private:
    // `blockTopIn` answers in the pane's viewport coordinates; the divider
    // paints in its own, and the two are siblings rather than nested.
    int paneYToLocal(const QPlainTextEdit *edit, int viewportY) const;

    QPlainTextEdit *leftEdit_;
    QPlainTextEdit *rightEdit_;
    QVector<Hunk> hunks_;
};

} // namespace ui_shell
