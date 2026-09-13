#pragma once

#include <QWidget>

class QEvent;
class QMouseEvent;
class QPaintEvent;

namespace ui_shell {

class CodeEditor;

// R4: a thin, always-on strip beside the vertical scrollbar showing every
// diagnostic in the file as a coloured tick, independent of the minimap
// (ADR-0044's overlays disappear with it off; this does not) — its own
// translation unit for the same reason `minimap.cpp`/`code_editor_gutter.cpp`
// are: a self-contained strip, pushed marks from outside and deciding none
// of them itself.
//
// Ticks are mapped onto the scrollbar exactly the way ADR-0044's minimap
// slider is: the scrollbar already counts visible (unfolded) lines, so
// there is no second coordinate system to keep in sync. A hover shows the
// line's message through the shared `EditorPopup`; a click moves the caret
// there. The top few pixels are reserved for the file-level summary rather
// than a tick, so it reads as a corner badge, not one more line's mark.
class ErrorStripe : public QWidget
{
public:
    explicit ErrorStripe(CodeEditor *editor);

    // The strip's own opinion of its width, the same "widget decides,
    // CodeEditor reads it back" arrangement `Minimap::preferredWidth` uses.
    static int preferredWidth();

protected:
    void paintEvent(QPaintEvent *event) override;
    void mousePressEvent(QMouseEvent *event) override;
    void mouseMoveEvent(QMouseEvent *event) override;
    void leaveEvent(QEvent *event) override;

private:
    CodeEditor *editor_;
};

} // namespace ui_shell
