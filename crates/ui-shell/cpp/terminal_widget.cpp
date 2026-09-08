#include "terminal_widget.h"

#include "theme.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <algorithm>
#include <cstddef>
#include <cstdlib>
#include <limits>

#include <QAction>
#include <QApplication>
#include <QClipboard>
#include <QColor>
#include <QContextMenuEvent>
#include <QDesktopServices>
#include <QEvent>
#include <QFocusEvent>
#include <QFontMetricsF>
#include <QKeyEvent>
#include <QKeySequence>
#include <QMenu>
#include <QMouseEvent>
#include <QPainter>
#include <QPaintEvent>
#include <QPalette>
#include <QResizeEvent>
#include <QScrollBar>
#include <QShowEvent>
#include <QSignalBlocker>
#include <QUrl>
#include <QWheelEvent>

namespace ui_shell {

namespace {

// Uniform padding around the grid on every side (T3) — JetBrains-style
// breathing room, rather than text glued to the dock's own edge. Grid
// geometry (`syncGridSizeToWidget`) and pixel<->cell mapping (`cellAt`) both
// work in the padding-deflated rect; the padding band itself is filled once
// per frame in the widget's backdrop colour and never drawn into again.
constexpr int kPadding = 6;

} // namespace

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

    // Scrollback (T5): a plain child scrollbar, positioned in
    // `layoutScrollBar` — this widget paints its own grid rather than using
    // `QAbstractScrollArea`, so there is no built-in one to reuse.
    scrollBar_ = new QScrollBar(Qt::Vertical, this);
    scrollBar_->setRange(0, 0);
    connect(scrollBar_, &QScrollBar::valueChanged, this, &TerminalWidget::onScrollBarValueChanged);

    applyFont();
    applyPalette();

    // Repaint only in response to genuinely new PTY output (per gridUpdated),
    // never on a timer — CLAUDE.md's/F3's explicit requirement. The signal is
    // shared by every session, so filter to this widget's own.
    connect(supervisor_, &TerminalSupervisor::gridUpdated, this, [this](quint64 sessionId) {
        if (sessionId == sessionId_) {
            snapshotStale_ = true;
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

void TerminalWidget::reapplyAppearance()
{
    applyFont();
    applyPalette();
    snapshotStale_ = true;
    syncGridSizeToWidget();
    update();
}

void TerminalWidget::applyFont()
{
    const FfiEditorFont terminalFont = appSettings_->terminalFont();
    font_ = QFont(terminalFont.family, static_cast<int>(terminalFont.size));
    // A terminal grid is only a grid if every glyph is one cell wide. The
    // configured family need not exist on this machine — "JetBrains Mono" is
    // the default and is not installed on a stock Windows — and without these
    // two hints Qt substitutes the *proportional* UI font, so the painted
    // runs drift left of their columns and the cursor lands far right of the
    // text (issue #234).
    font_.setStyleHint(QFont::Monospace, QFont::PreferMatch);
    font_.setFixedPitch(true);
    fontBold_ = font_;
    fontBold_.setBold(true);
    fontItalic_ = font_;
    fontItalic_.setItalic(true);
    fontBoldItalic_ = font_;
    fontBoldItalic_.setBold(true);
    fontBoldItalic_.setItalic(true);

    const QFontMetricsF metrics(font_);
    cellWidth_ = std::max(1, qRound(metrics.horizontalAdvance(QLatin1Char('M'))));
    cellHeight_ = qRound(metrics.height());
    ascent_ = metrics.ascent();

    // Cells are laid out on whole-pixel columns while `drawText` advances by
    // the font's own fractional widths, so a run painted as one string walks
    // off the grid a fraction of a pixel per character. Pinning each variant's
    // advance to exactly `cellWidth_` keeps every column aligned no matter how
    // long the run.
    for (QFont *variant : { &font_, &fontBold_, &fontItalic_, &fontBoldItalic_ }) {
        const qreal advance = QFontMetricsF(*variant).horizontalAdvance(QLatin1Char('M'));
        variant->setLetterSpacing(QFont::AbsoluteSpacing, cellWidth_ - advance);
    }
}

void TerminalWidget::applyPalette()
{
    const FfiTerminalPalette themedPalette = terminalPaletteForTheme(activeThemeName(), appSettings_);
    bgColor_ = QColor(themedPalette.background.r, themedPalette.background.g, themedPalette.background.b);
    selectionColor_ =
      QColor(themedPalette.selection.r, themedPalette.selection.g, themedPalette.selection.b);
    cursorColor_ = QColor(themedPalette.cursor.r, themedPalette.cursor.g, themedPalette.cursor.b);

    QPalette pal = palette();
    pal.setColor(QPalette::Window, bgColor_);
    setAutoFillBackground(true);
    setPalette(pal);
}

void TerminalWidget::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    syncGridSizeToWidget();
}

void TerminalWidget::resizeEvent(QResizeEvent *event)
{
    QWidget::resizeEvent(event);
    layoutScrollBar();
    syncGridSizeToWidget();
}

void TerminalWidget::layoutScrollBar()
{
    const int barWidth = scrollBar_->sizeHint().width();
    scrollBar_->setGeometry(width() - barWidth, 0, barWidth, height());
}

void TerminalWidget::syncGridSizeToWidget()
{
    const int usableWidth = width() - 2 * kPadding - scrollBar_->sizeHint().width();
    const int usableHeight = height() - 2 * kPadding;
    const quint32 newCols = static_cast<quint32>(std::max(1, usableWidth / cellWidth_));
    const quint32 newRows = static_cast<quint32>(std::max(1, usableHeight / cellHeight_));
    if (started_ && newCols == cols_ && newRows == rows_) {
        return;
    }
    cols_ = newCols;
    rows_ = newRows;
    snapshotStale_ = true;
    if (!started_) {
        started_ = true;
        supervisor_->start(sessionId_, shellId_, rows_, cols_);
    } else {
        supervisor_->resize(sessionId_, rows_, cols_);
    }
}

TerminalWidget::CellStyle TerminalWidget::styleFor(const FfiTerminalCell &cell, quint32 row, quint32 col,
                                                     quint32 cursorRow, quint32 cursorCol) const
{
    QColor fg(cell.fg_r, cell.fg_g, cell.fg_b);
    QColor bg(cell.bg_r, cell.bg_g, cell.bg_b);
    // An SGR-inverse cell swaps fg/bg — unrelated to selection or the
    // cursor, both handled below.
    if (cell.inverse) {
        std::swap(fg, bg);
    }
    if (cell.selected) {
        // Tint the background only; the glyph keeps its normal foreground.
        bg = selectionColor_;
    }
    return CellStyle{ fg,      bg,       cell.bold,
                       cell.italic,  cell.underline, cell.selected,
                       row == cursorRow && col == cursorCol };
}

const QFont &TerminalWidget::fontFor(bool bold, bool italic) const
{
    if (bold && italic) {
        return fontBoldItalic_;
    }
    if (bold) {
        return fontBold_;
    }
    if (italic) {
        return fontItalic_;
    }
    return font_;
}

void TerminalWidget::paintRunBody(QPainter &painter, const QColor &fg, const QColor &bg, bool bold,
                                   bool italic, bool underline, const QRect &rect, qreal baselineY,
                                   const std::u32string &text, bool forceFill)
{
    if (forceFill || bg != bgColor_) {
        painter.fillRect(rect, bg);
    }
    const bool blank = std::all_of(text.begin(), text.end(), [](char32_t c) { return c == U' '; });
    if (!text.empty() && !blank) {
        painter.setFont(fontFor(bold, italic));
        painter.setPen(fg);
        const QString run = QString::fromUcs4(text.data(), static_cast<qsizetype>(text.size()));
        painter.drawText(QPointF(rect.left(), baselineY), run);
    }
    if (underline) {
        painter.setPen(fg);
        const int y = static_cast<int>(baselineY) + 1;
        painter.drawLine(rect.left(), y, rect.left() + rect.width(), y);
    }
}

void TerminalWidget::paintEvent(QPaintEvent *event)
{
    Q_UNUSED(event);
    QPainter painter(this);
    painter.fillRect(rect(), bgColor_);

    // The actual perf win (T2): re-snapshot only when something changed the
    // grid since the last paint, not on every repaint. A scroll (T5) sets
    // this stale too, so the scrollbar's range/value are refreshed here,
    // alongside the grid it now reflects, rather than as a separate path.
    if (snapshotStale_) {
        cachedSnapshot_ = supervisor_->snapshot(sessionId_);
        refreshScrollState();
        snapshotStale_ = false;
    }

    const quint32 rows = cachedSnapshot_.rows;
    const quint32 cols = cachedSnapshot_.cols;
    if (rows == 0 || cols == 0) {
        return;
    }
    const rust::Vec<FfiTerminalCell> &cells = cachedSnapshot_.cells;
    // Scrolled up into history (T5), the cursor's row/col describe where it
    // would be on the live screen, not anywhere in the current view — an
    // out-of-range sentinel keeps every cell's `row == cursorRow` check
    // false without special-casing `styleFor`'s call site further.
    const quint32 cursorRow =
      cachedSnapshot_.cursor_visible ? cachedSnapshot_.cursor_row : std::numeric_limits<quint32>::max();
    const quint32 cursorCol =
      cachedSnapshot_.cursor_visible ? cachedSnapshot_.cursor_col : std::numeric_limits<quint32>::max();
    const bool focused = hasFocus();

    for (quint32 row = 0; row < rows; ++row) {
        quint32 col = 0;
        bool prevWide = false;
        while (col < cols) {
            const std::size_t idx = static_cast<std::size_t>(row) * cols + col;
            if (idx >= cells.size()) {
                break;
            }
            const bool isSpacer = prevWide;
            const CellStyle style = styleFor(cells[idx], row, col, cursorRow, cursorCol);

            std::u32string text;
            if (!isSpacer) {
                text.push_back(static_cast<char32_t>(cells[idx].character));
            }
            const quint32 runStart = col;
            prevWide = cells[idx].wide;
            ++col;

            // Extend the run while every cell shares this style — the
            // WIDE_CHAR_SPACER half of a wide glyph shares its leading
            // cell's style, so it merges into the run too; only its
            // character is skipped (already painted by the glyph itself).
            while (col < cols) {
                const std::size_t nextIdx = static_cast<std::size_t>(row) * cols + col;
                if (nextIdx >= cells.size()) {
                    break;
                }
                const bool nextIsSpacer = prevWide;
                const CellStyle nextStyle = styleFor(cells[nextIdx], row, col, cursorRow, cursorCol);
                if (!(nextStyle == style)) {
                    break;
                }
                if (!nextIsSpacer) {
                    text.push_back(static_cast<char32_t>(cells[nextIdx].character));
                }
                prevWide = cells[nextIdx].wide;
                ++col;
            }

            const QRect runRect(kPadding + static_cast<int>(runStart) * cellWidth_,
                                 kPadding + static_cast<int>(row) * cellHeight_,
                                 static_cast<int>(col - runStart) * cellWidth_, cellHeight_);
            const qreal baselineY = kPadding + static_cast<qreal>(row) * cellHeight_ + ascent_;

            if (style.isCursor) {
                if (focused) {
                    // Filled block in the theme's cursor colour, the glyph
                    // drawn in the cell's own background colour on top.
                    paintRunBody(painter, style.bg, cursorColor_, style.bold, style.italic,
                                 style.underline, runRect, baselineY, text, /*forceFill=*/true);
                } else {
                    paintRunBody(painter, style.fg, style.bg, style.bold, style.italic, style.underline,
                                 runRect, baselineY, text, /*forceFill=*/false);
                    painter.setPen(cursorColor_);
                    painter.drawRect(runRect.adjusted(0, 0, -1, -1));
                }
            } else {
                paintRunBody(painter, style.fg, style.bg, style.bold, style.italic, style.underline, runRect,
                             baselineY, text, /*forceFill=*/false);
            }
        }
    }

    // Ctrl-hovered link: underline the exact span the click would open, so
    // what is clickable is never a guess.
    if (hoverLink_.found && hoverLink_.row < rows) {
        const int y = kPadding + (static_cast<int>(hoverLink_.row) + 1) * cellHeight_ - 1;
        painter.setPen(cursorColor_);
        painter.drawLine(kPadding + static_cast<int>(hoverLink_.start_col) * cellWidth_, y,
                          kPadding + static_cast<int>(hoverLink_.end_col) * cellWidth_, y);
    }
}

QPoint TerminalWidget::cellAt(const QPoint &pos) const
{
    const int col = std::clamp((pos.x() - kPadding) / cellWidth_, 0, static_cast<int>(cols_) - 1);
    const int row = std::clamp((pos.y() - kPadding) / cellHeight_, 0, static_cast<int>(rows_) - 1);
    return {col, row};
}

bool TerminalWidget::rightHalf(const QPoint &pos) const
{
    const int xInGrid = std::max(0, pos.x() - kPadding);
    return (xInGrid % cellWidth_) * 2 >= cellWidth_;
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

void TerminalWidget::refreshScrollState()
{
    const FfiScrollState state = supervisor_->scrollState(sessionId_);
    const quint64 clampedHistory =
      std::min<quint64>(state.history, static_cast<quint64>(std::numeric_limits<int>::max()));
    const quint64 clampedOffset = std::min<quint64>(state.offset, clampedHistory);
    const int history = static_cast<int>(clampedHistory);
    const int offset = static_cast<int>(clampedOffset);

    // Block the bar's own `valueChanged` while syncing it from the model —
    // otherwise this would immediately re-enter `onScrollBarValueChanged`
    // and call `scrollTo` right back, fighting a wheel/keyboard scroll that
    // is what triggered this refresh in the first place.
    const QSignalBlocker blocker(scrollBar_);
    scrollBar_->setRange(0, history);
    scrollBar_->setValue(history - offset);
}

void TerminalWidget::scrollLines(int delta)
{
    if (delta == 0) {
        return;
    }
    supervisor_->scroll(sessionId_, delta);
    snapshotStale_ = true;
    update();
}

void TerminalWidget::onScrollBarValueChanged(int value)
{
    const int history = scrollBar_->maximum();
    const quint64 offset = static_cast<quint64>(std::max(0, history - value));
    supervisor_->scrollTo(sessionId_, offset);
    snapshotStale_ = true;
    update();
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
        snapshotStale_ = true;
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
    snapshotStale_ = true;
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
        snapshotStale_ = true;
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
    snapshotStale_ = true;
    // A double click also starts a drag in word/line units, matching every
    // other terminal.
    dragging_ = true;
    update();
    event->accept();
}

void TerminalWidget::wheelEvent(QWheelEvent *event)
{
    // The alternate screen (T5) means a full-screen app — `vim`, `less`,
    // `htop` — owns the viewport; there is no history to scroll, so the
    // wheel becomes the arrow keys those apps already read wheel input as,
    // matching every other terminal.
    if (supervisor_->altScreen(sessionId_)) {
        const int steps = event->angleDelta().y() / 120;
        if (steps == 0) {
            QWidget::wheelEvent(event);
            return;
        }
        const FfiTerminalKey key = steps > 0 ? FfiTerminalKey::Up : FfiTerminalKey::Down;
        for (int i = 0; i < std::abs(steps); ++i) {
            supervisor_->sendKey(sessionId_, key, 0, false, false, false);
        }
        event->accept();
        return;
    }

    scrollLines(event->angleDelta().y() / 120 * 3);
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
    // While the terminal has focus, several key combinations belong to the
    // shell/the running program rather than to Qt's own focus-chain or
    // window-level menu shortcuts, which otherwise see the key first. Not
    // accepting the ShortcutOverride here means keyPressEvent would never
    // get them (Tab would move focus instead of completing a command,
    // arrows/Home/End/PageUp/PageDown would be swallowed by whatever dock
    // action happens to bind them).
    //
    // Ctrl+` (view.terminal, the dock toggle) and Ctrl+Shift+C/V
    // (terminal.copy/terminal.paste, this widget's own QActions) are
    // deliberately excluded so there is always a way back out and copy/paste
    // keep working: only plain Ctrl+letter (no Shift/Alt) is taken here —
    // Ctrl+Shift+C/V already fail that check on Shift alone, and Ctrl+` fails
    // it because backtick is not in the A-Z range.
    if (event->type() == QEvent::ShortcutOverride) {
        auto *keyEvent = static_cast<QKeyEvent *>(event);
        if (keyEvent->modifiers() == Qt::ControlModifier && keyEvent->key() >= Qt::Key_A
            && keyEvent->key() <= Qt::Key_Z) {
            event->accept();
            return true;
        }
        switch (keyEvent->key()) {
        case Qt::Key_Tab:
        case Qt::Key_Backtab:
        case Qt::Key_Up:
        case Qt::Key_Down:
        case Qt::Key_Left:
        case Qt::Key_Right:
        case Qt::Key_Home:
        case Qt::Key_End:
        case Qt::Key_PageUp:
        case Qt::Key_PageDown:
            event->accept();
            return true;
        default:
            break;
        }
    }
    return QWidget::event(event);
}

namespace {

// Qt::Key -> FfiTerminalKey for the keys `terminal_core::keys::Key` names
// directly (everything except `Char`/`F`, which `sendTranslatedKey` builds
// separately since they carry a code point). `Key_Backtab` maps to the same
// `Tab` variant as `Key_Tab` — which one Qt delivers for Shift+Tab varies by
// platform, and either way it is the `shift` flag, not the key identity,
// that turns it into a back-tab (`terminal_core::keys::encode`).
bool namedFfiKeyFor(int qtKey, FfiTerminalKey &out)
{
    switch (qtKey) {
    case Qt::Key_Return:
    case Qt::Key_Enter:
        out = FfiTerminalKey::Enter;
        return true;
    case Qt::Key_Backspace:
        out = FfiTerminalKey::Backspace;
        return true;
    case Qt::Key_Tab:
    case Qt::Key_Backtab:
        out = FfiTerminalKey::Tab;
        return true;
    case Qt::Key_Escape:
        out = FfiTerminalKey::Escape;
        return true;
    case Qt::Key_Up:
        out = FfiTerminalKey::Up;
        return true;
    case Qt::Key_Down:
        out = FfiTerminalKey::Down;
        return true;
    case Qt::Key_Left:
        out = FfiTerminalKey::Left;
        return true;
    case Qt::Key_Right:
        out = FfiTerminalKey::Right;
        return true;
    case Qt::Key_Home:
        out = FfiTerminalKey::Home;
        return true;
    case Qt::Key_End:
        out = FfiTerminalKey::End;
        return true;
    case Qt::Key_PageUp:
        out = FfiTerminalKey::PageUp;
        return true;
    case Qt::Key_PageDown:
        out = FfiTerminalKey::PageDown;
        return true;
    case Qt::Key_Insert:
        out = FfiTerminalKey::Insert;
        return true;
    case Qt::Key_Delete:
        out = FfiTerminalKey::Delete;
        return true;
    default:
        return false;
    }
}

} // namespace

bool TerminalWidget::sendTranslatedKey(QKeyEvent *event)
{
    const bool shift = event->modifiers().testFlag(Qt::ShiftModifier);
    const bool ctrl = event->modifiers().testFlag(Qt::ControlModifier);
    const bool alt = event->modifiers().testFlag(Qt::AltModifier);

    FfiTerminalKey namedKey;
    if (namedFfiKeyFor(event->key(), namedKey)) {
        supervisor_->sendKey(sessionId_, namedKey, 0, shift, ctrl, alt);
        return true;
    }
    if (event->key() >= Qt::Key_F1 && event->key() <= Qt::Key_F12) {
        const quint32 n = static_cast<quint32>(event->key() - Qt::Key_F1 + 1);
        supervisor_->sendKey(sessionId_, FfiTerminalKey::F, n, shift, ctrl, alt);
        return true;
    }

    // Ctrl/Alt combinations on a printable key (Ctrl+C, Ctrl+[, Alt+b, ...):
    // Qt's own `event->text()` already applied to the shell's platform key
    // mapping, which is not the same encoding a terminal expects (and is
    // often empty). The base ASCII key value plus the modifiers we already
    // extracted is what `terminal_core::keys::encode` needs instead — Qt
    // reports these printable keys as their ASCII code regardless of
    // modifiers, so `shift` alone decides the letter's case.
    if ((ctrl || alt) && event->key() >= 0x20 && event->key() <= 0x7e) {
        QChar ch(static_cast<char16_t>(event->key()));
        if (!shift) {
            ch = ch.toLower();
        }
        supervisor_->sendKey(sessionId_, FfiTerminalKey::Char, ch.unicode(), shift, ctrl, alt);
        return true;
    }

    // Plain typing, incl. IME/composed text: a single code point goes
    // through the same translation path (harmless — `encode()` just returns
    // it unchanged); anything else (a multi-code-point composed string) is
    // forwarded to the PTY as-is, unchanged from this widget's original
    // behavior.
    const QString text = event->text();
    const auto codePoints = text.toUcs4();
    if (codePoints.size() == 1) {
        supervisor_->sendKey(sessionId_, FfiTerminalKey::Char, codePoints.first(), shift, ctrl, alt);
        return true;
    }
    if (!text.isEmpty()) {
        supervisor_->write(sessionId_, text);
        return true;
    }
    return false;
}

void TerminalWidget::keyPressEvent(QKeyEvent *event)
{
    // Scrollback (T5): Shift+PageUp/PageDown/Home/End are view-side
    // gestures — they move the viewport, not the shell's own cursor — so
    // they are handled here directly rather than through
    // `sendTranslatedKey`/`terminal_core::keys::encode`. The page size is
    // this widget's own current row count, the same value
    // `syncGridSizeToWidget` derived it from.
    if (event->modifiers() == Qt::ShiftModifier) {
        switch (event->key()) {
        case Qt::Key_PageUp:
            scrollLines(static_cast<int>(rows_));
            event->accept();
            return;
        case Qt::Key_PageDown:
            scrollLines(-static_cast<int>(rows_));
            event->accept();
            return;
        case Qt::Key_Home: {
            const FfiScrollState state = supervisor_->scrollState(sessionId_);
            supervisor_->scrollTo(sessionId_, state.history);
            snapshotStale_ = true;
            update();
            event->accept();
            return;
        }
        case Qt::Key_End:
            supervisor_->scrollToBottom(sessionId_);
            snapshotStale_ = true;
            update();
            event->accept();
            return;
        default:
            break;
        }
    }

    if (sendTranslatedKey(event)) {
        event->accept();
        return;
    }
    QWidget::keyPressEvent(event);
}

void TerminalWidget::focusInEvent(QFocusEvent *event)
{
    QWidget::focusInEvent(event);
    update();
}

void TerminalWidget::focusOutEvent(QFocusEvent *event)
{
    QWidget::focusOutEvent(event);
    update();
}

} // namespace ui_shell
