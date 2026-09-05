#include "terminal_widget.h"

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <algorithm>
#include <cstddef>

#include <QAction>
#include <QApplication>
#include <QClipboard>
#include <QColor>
#include <QContextMenuEvent>
#include <QDesktopServices>
#include <QEvent>
#include <QFontDatabase>
#include <QFontMetrics>
#include <QKeyEvent>
#include <QKeySequence>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QPaintEvent>
#include <QPalette>
#include <QResizeEvent>
#include <QShowEvent>
#include <QUrl>

namespace ui_shell {

TerminalWidget::TerminalWidget(TerminalSupervisor *supervisor, quint64 sessionId, QString shellId,
                                AppSettings *appSettings, OpenAt openAt, QWidget *parent)
  : QWidget(parent)
  , supervisor_(supervisor)
  , openAt_(std::move(openAt))
  , sessionId_(sessionId)
  , shellId_(std::move(shellId))
  , appSettings_(appSettings)
{
    setFocusPolicy(Qt::StrongFocus);
    // Needed for Ctrl-hover link feedback, which has to react to moves with
    // no button held down.
    setMouseTracking(true);

    copyAction_ = new QAction(tr("Copy"), this);
    copyAction_->setShortcut(QKeySequence(appSettings_->shortcutFor(QStringLiteral("terminal.copy")),
                                           QKeySequence::PortableText));
    copyAction_->setShortcutContext(Qt::WidgetShortcut);
    connect(copyAction_, &QAction::triggered, this, &TerminalWidget::copySelection);
    addAction(copyAction_);

    pasteAction_ = new QAction(tr("Paste"), this);
    pasteAction_->setShortcut(QKeySequence(
      appSettings_->shortcutFor(QStringLiteral("terminal.paste")), QKeySequence::PortableText));
    pasteAction_->setShortcutContext(Qt::WidgetShortcut);
    connect(pasteAction_, &QAction::triggered, this, &TerminalWidget::pasteClipboard);
    addAction(pasteAction_);

    font_ = QFontDatabase::systemFont(QFontDatabase::FixedFont);
    font_.setPointSize(10);
    const QFontMetrics metrics(font_);
    cellWidth_ = std::max(1, metrics.horizontalAdvance(QLatin1Char('M')));
    cellHeight_ = std::max(1, metrics.height());

    QPalette pal = palette();
    pal.setColor(QPalette::Window, Qt::black);
    setAutoFillBackground(true);
    setPalette(pal);

    // Repaint only in response to genuinely new PTY output (per gridUpdated),
    // never on a timer — CLAUDE.md's/F3's explicit requirement. The signal is
    // shared by every session, so filter to this widget's own.
    connect(supervisor_, &TerminalSupervisor::gridUpdated, this, [this](quint64 sessionId) {
        if (sessionId == sessionId_) {
            update();
        }
    });
}

void TerminalWidget::reapplyKeymap()
{
    copyAction_->setShortcut(QKeySequence(appSettings_->shortcutFor(QStringLiteral("terminal.copy")),
                                           QKeySequence::PortableText));
    pasteAction_->setShortcut(QKeySequence(
      appSettings_->shortcutFor(QStringLiteral("terminal.paste")), QKeySequence::PortableText));
}

void TerminalWidget::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    syncGridSizeToWidget();
}

void TerminalWidget::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    syncGridSizeToWidget();
}

void TerminalWidget::syncGridSizeToWidget()
{
    const quint32 newCols = static_cast<quint32>(std::max(1, width() / cellWidth_));
    const quint32 newRows = static_cast<quint32>(std::max(1, height() / cellHeight_));
    if (started_ && newCols == cols_ && newRows == rows_) {
        return;
    }
    cols_ = newCols;
    rows_ = newRows;
    if (!started_) {
        started_ = true;
        supervisor_->start(sessionId_, shellId_, rows_, cols_);
    } else {
        supervisor_->resize(sessionId_, rows_, cols_);
    }
}

void TerminalWidget::paintEvent(QPaintEvent *event)
{
    Q_UNUSED(event);
    QPainter painter(this);
    painter.setFont(font_);
    painter.fillRect(rect(), Qt::black);

    const quint32 rows = supervisor_->gridRows(sessionId_);
    const quint32 cols = supervisor_->gridCols(sessionId_);
    if (rows == 0 || cols == 0) {
        return;
    }
    const rust::Vec<FfiTerminalCell> cells = supervisor_->gridCells(sessionId_);
    const quint32 cursorRow = supervisor_->cursorRow(sessionId_);
    const quint32 cursorCol = supervisor_->cursorCol(sessionId_);

    for (quint32 row = 0; row < rows; ++row) {
        for (quint32 col = 0; col < cols; ++col) {
            const std::size_t idx = static_cast<std::size_t>(row) * cols + col;
            if (idx >= cells.size()) {
                continue;
            }
            const FfiTerminalCell &cell = cells[idx];

            QColor fg(cell.fg_r, cell.fg_g, cell.fg_b);
            QColor bg(cell.bg_r, cell.bg_g, cell.bg_b);
            // An SGR-inverse cell swaps fg/bg; the cursor block does the
            // same on top of whatever the cell already is, so landing on an
            // already-inverse cell cancels back out (XOR).
            // A selected cell inverts on top of that again, so selecting an
            // already-inverse cell cancels back out the same way.
            if (cell.inverse != (row == cursorRow && col == cursorCol) != cell.selected) {
                std::swap(fg, bg);
            }

            const QRect cellRect(static_cast<int>(col) * cellWidth_,
                                  static_cast<int>(row) * cellHeight_, cellWidth_, cellHeight_);
            painter.fillRect(cellRect, bg);
            painter.setPen(fg);
            painter.drawText(cellRect, Qt::AlignLeft | Qt::AlignVCenter, cell.character);
        }
    }

    // Ctrl-hovered link: underline the exact span the click would open, so
    // what is clickable is never a guess.
    if (hoverLink_.found && hoverLink_.row < rows) {
        const int y = (static_cast<int>(hoverLink_.row) + 1) * cellHeight_ - 1;
        painter.setPen(QColor(Qt::white));
        painter.drawLine(static_cast<int>(hoverLink_.start_col) * cellWidth_, y,
                          static_cast<int>(hoverLink_.end_col) * cellWidth_, y);
    }
}

QPoint TerminalWidget::cellAt(const QPoint &pos) const
{
    const int col = std::clamp(pos.x() / cellWidth_, 0, static_cast<int>(cols_) - 1);
    const int row = std::clamp(pos.y() / cellHeight_, 0, static_cast<int>(rows_) - 1);
    return {col, row};
}

bool TerminalWidget::rightHalf(const QPoint &pos) const
{
    return (pos.x() % cellWidth_) * 2 >= cellWidth_;
}

void TerminalWidget::copySelection()
{
    if (!supervisor_->hasSelection(sessionId_)) {
        return;
    }
    QGuiApplication::clipboard()->setText(supervisor_->selectionText(sessionId_));
}

void TerminalWidget::pasteClipboard()
{
    const QString text = QGuiApplication::clipboard()->text();
    if (text.isEmpty()) {
        return;
    }
    supervisor_->paste(sessionId_, text);
}

void TerminalWidget::openLink(const FfiTerminalLink &link)
{
    if (!link.found) {
        return;
    }
    // Which kind of link this is was decided in `TerminalSupervisor::linkAt`
    // (R2-6); this only routes it.
    if (link.is_file) {
        if (openAt_) {
            openAt_(link.path, static_cast<int>(link.line),
                    link.has_column ? static_cast<int>(link.column) : 0);
        }
        return;
    }
    QDesktopServices::openUrl(QUrl(link.url));
}

void TerminalWidget::updateHoverLink(const QPoint &pos, bool ctrlHeld)
{
    const QPoint cell = cellAt(pos);
    const FfiTerminalLink link = ctrlHeld
      ? supervisor_->linkAt(sessionId_, static_cast<quint32>(cell.y()), static_cast<quint32>(cell.x()))
      : FfiTerminalLink{};
    if (link.found == hoverLink_.found && link.row == hoverLink_.row
        && link.start_col == hoverLink_.start_col && link.end_col == hoverLink_.end_col) {
        return;
    }
    hoverLink_ = link;
    setCursor(hoverLink_.found ? Qt::PointingHandCursor : Qt::IBeamCursor);
    update();
}

void TerminalWidget::mousePressEvent(QMouseEvent *event)
{
    if (event->button() != Qt::LeftButton) {
        QWidget::mousePressEvent(event);
        return;
    }

    const QPoint cell = cellAt(event->pos());

    // Qt has no triple-click event: a triple click arrives as press,
    // double-click, press. So the press landing within the double-click
    // interval of a double click is the third one — select the whole line.
    if (doubleClickTimer_.isValid()
        && doubleClickTimer_.elapsed() < QApplication::doubleClickInterval()) {
        doubleClickTimer_.invalidate();
        supervisor_->selectionStart(sessionId_, static_cast<quint32>(cell.y()),
                                     static_cast<quint32>(cell.x()), rightHalf(event->pos()),
                                     FfiSelectionKind::Line);
        dragging_ = true;
        update();
        event->accept();
        return;
    }

    if (event->modifiers().testFlag(Qt::ControlModifier)) {
        const FfiTerminalLink link =
          supervisor_->linkAt(sessionId_, static_cast<quint32>(cell.y()), static_cast<quint32>(cell.x()));
        if (link.found) {
            openLink(link);
            event->accept();
            return;
        }
    }

    supervisor_->selectionClear(sessionId_);
    supervisor_->selectionStart(sessionId_, static_cast<quint32>(cell.y()),
                                 static_cast<quint32>(cell.x()), rightHalf(event->pos()),
                                 FfiSelectionKind::Simple);
    dragging_ = true;
    update();
    event->accept();
}

void TerminalWidget::mouseMoveEvent(QMouseEvent *event)
{
    if (dragging_) {
        const QPoint cell = cellAt(event->pos());
        supervisor_->selectionUpdate(sessionId_, static_cast<quint32>(cell.y()),
                                      static_cast<quint32>(cell.x()), rightHalf(event->pos()));
        update();
        event->accept();
        return;
    }
    updateHoverLink(event->pos(), event->modifiers().testFlag(Qt::ControlModifier));
    QWidget::mouseMoveEvent(event);
}

void TerminalWidget::mouseReleaseEvent(QMouseEvent *event)
{
    if (!dragging_) {
        QWidget::mouseReleaseEvent(event);
        return;
    }
    dragging_ = false;

    // Auto-copy goes to the X11 PRIMARY selection only — the middle-click
    // clipboard Unix users expect a drag to fill. The regular clipboard is
    // left alone so a selection never silently overwrites what the user
    // copied elsewhere; supportsSelection() is false on Windows, where this
    // is simply skipped.
    QClipboard *clipboard = QGuiApplication::clipboard();
    if (clipboard->supportsSelection() && supervisor_->hasSelection(sessionId_)) {
        clipboard->setText(supervisor_->selectionText(sessionId_), QClipboard::Selection);
    }
    event->accept();
}

void TerminalWidget::mouseDoubleClickEvent(QMouseEvent *event)
{
    if (event->button() != Qt::LeftButton) {
        QWidget::mouseDoubleClickEvent(event);
        return;
    }
    doubleClickTimer_.restart();

    const QPoint cell = cellAt(event->pos());
    supervisor_->selectionStart(sessionId_, static_cast<quint32>(cell.y()),
                                 static_cast<quint32>(cell.x()), rightHalf(event->pos()),
                                 FfiSelectionKind::Word);
    // A double click also starts a drag in word/line units, matching every
    // other terminal.
    dragging_ = true;
    update();
    event->accept();
}

void TerminalWidget::contextMenuEvent(QContextMenuEvent *event)
{
    const QPoint cell = cellAt(event->pos());
    const FfiTerminalLink link =
      supervisor_->linkAt(sessionId_, static_cast<quint32>(cell.y()), static_cast<quint32>(cell.x()));

    QMenu menu(this);
    copyAction_->setEnabled(supervisor_->hasSelection(sessionId_));
    menu.addAction(copyAction_);
    menu.addAction(pasteAction_);
    if (link.found) {
        menu.addSeparator();
        QAction *open = menu.addAction(link.is_file ? tr("Open File") : tr("Open Link"));
        connect(open, &QAction::triggered, this, [this, link]() { openLink(link); });
    }
    menu.exec(event->globalPos());
    // The action outlives the menu, so leave it usable for its shortcut.
    copyAction_->setEnabled(true);
    event->accept();
}

bool TerminalWidget::event(QEvent *event)
{
    // While the terminal has focus, Ctrl+letter belongs to the shell:
    // Ctrl+C is SIGINT, Ctrl+D is EOF, Ctrl+S/Ctrl+Q are flow control.
    // Qt offers window-level menu shortcuts (Edit > Copy is Ctrl+C) the key
    // first, so without accepting the ShortcutOverride here those combos
    // would never reach keyPressEvent. Only plain Ctrl+letter is taken —
    // Ctrl+Shift+C/V are this widget's own copy/paste actions, and
    // Ctrl+` still toggles the dock, so there is always a way back out.
    if (event->type() == QEvent::ShortcutOverride) {
        auto *keyEvent = static_cast<QKeyEvent *>(event);
        if (keyEvent->modifiers() == Qt::ControlModifier && keyEvent->key() >= Qt::Key_A
            && keyEvent->key() <= Qt::Key_Z) {
            event->accept();
            return true;
        }
    }
    return QWidget::event(event);
}

void TerminalWidget::keyPressEvent(QKeyEvent *event)
{
    // First-cut keyboard coverage: printable characters (incl. IME/composed
    // text via event->text()), Enter, Backspace, Tab, Escape. Arrow keys and
    // Ctrl-combinations are NOT translated to their escape sequences yet —
    // see this class's doc comment / the task's own report for the gap.
    QString toSend;
    switch (event->key()) {
    case Qt::Key_Return:
    case Qt::Key_Enter:
        toSend = QStringLiteral("\r");
        break;
    case Qt::Key_Backspace:
        toSend = QString(QChar(0x7f));
        break;
    case Qt::Key_Tab:
        toSend = QStringLiteral("\t");
        break;
    case Qt::Key_Escape:
        toSend = QString(QChar(0x1b));
        break;
    default:
        toSend = event->text();
        break;
    }

    if (toSend.isEmpty()) {
        QWidget::keyPressEvent(event);
        return;
    }
    supervisor_->write(sessionId_, toSend);
    event->accept();
}

} // namespace ui_shell
