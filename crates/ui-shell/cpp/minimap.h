#pragma once

#include <QPixmap>
#include <QRect>
#include <QWidget>

class QEvent;
class QMouseEvent;
class QPaintEvent;
class QPainter;
class QWheelEvent;

namespace ui_shell {

class CodeEditor;

// Editor minimap (code map, issue #199): a scaled, syntax-coloured strip on
// the right edge of a CodeEditor, with a draggable viewport slider and a set
// of switchable overlays. 1:1 with `FfiMinimapOptions`, but view-local like
// `WhitespaceOptions` — kept plain so code_editor.h never includes the
// cxx-qt generated header.
struct MinimapOptions
{
    bool enabled = true;
    bool searchMatches = true;
    bool diagnostics = true;
    bool vcsChanges = true;
    bool breakpoints = true;
    bool caretLine = true;
};

// No Q_OBJECT: it calls its CodeEditor directly and needs no signals, the
// same arrangement LineNumberArea uses.
class Minimap : public QWidget
{
public:
    explicit Minimap(CodeEditor *editor);

    void setOptions(const MinimapOptions &options);
    const MinimapOptions &options() const { return options_; }

    // Width the strip should be given, in pixels — the widget's own opinion,
    // read by CodeEditor when it lays out the viewport margins.
    static int preferredWidth();

protected:
    void paintEvent(QPaintEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;
    void mouseReleaseEvent(QMouseEvent *event) override;
    void wheelEvent(QWheelEvent *event) override;
    void leaveEvent(QEvent *event) override;

private:
    void ensureCodeCache(int firstRow, int rowCount);
    void renderCode(QPainter &painter, int firstRow, int rowCount);
    void paintOverlays(QPainter &painter, int firstRow, int rowCount);
    void paintSlider(QPainter &painter);

    int totalRows() const;
    int visibleRows() const;
    int firstRow() const;
    QRect sliderRect() const;
    // The visible-row index (0-based, folded blocks excluded) a document
    // block paints at, or -1 when the block itself is hidden by a fold.
    //
    // ponytail: O(document blocks) per call — fine for the handful of
    // overlay items a normal file has (breakpoints, the caret, a few dozen
    // diagnostics). A file whose find-in-file lights up thousands of rows
    // would pay for a walk per match; upgrade to a single per-paint
    // block-number -> visible-row array if that shows up as a real stall.
    int visibleRowForBlock(int blockNumber) const;
    void scrollToPixelY(int y);

    CodeEditor *editor_;
    MinimapOptions options_;

    QPixmap codeCache_;
    int cacheRevision_ = -1;
    int cacheFirstRow_ = -1;
    int cacheWidth_ = -1;
    int cacheHeight_ = -1;
    unsigned int cacheBaseColor_ = 0;

    bool dragging_ = false;
    bool hovered_ = false;
    int dragGrabOffsetY_ = 0;
};

} // namespace ui_shell
