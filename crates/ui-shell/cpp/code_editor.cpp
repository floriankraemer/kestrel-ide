#include "code_editor.h"

#include "diff_pane.h"

#include "e2e_mark.h"
#include "theme.h"
#include <QContextMenuEvent>
#include <QMenu>

#include <QAbstractItemView>
#include <QAction>
#include <QColor>
#include <QCompleter>
#include <QEvent>
#include <QFocusEvent>
#include <QFontMetrics>
#include <QHelpEvent>
#include <QInputMethodEvent>
#include <QMimeData>
#include <QKeyEvent>
#include <QMouseEvent>
#include <QPalette>
#include <QPaintEvent>
#include <QPainter>
#include <QPolygon>
#include <QResizeEvent>
#include <QScrollBar>
#include <QStandardItemModel>
#include <QStringList>
#include <QTextBlock>
#include <QTextCursor>
#include <QTextDocument>
#include <QTextEdit>
#include <QWheelEvent>

#include <algorithm>

namespace ui_shell {

namespace {

// Which CompletionEntry a popup row stands for. Read back through the
// completer's proxy model, so no assumption is made about the proxy keeping
// the source order.
constexpr int kEntryIndexRole = Qt::UserRole + 1;

// Slack added to the popup's ideal width so the last glyph is not clipped.
constexpr int kPopupWidthPadding = 8;

} // namespace

CodeEditor::CodeEditor(QWidget *parent)
  : QPlainTextEdit(parent)
  , lineNumberArea_(new LineNumberArea(this))
  , minimap_(new Minimap(this))
{
    connect(this, &CodeEditor::blockCountChanged, this, &CodeEditor::updateLineNumberAreaWidth);
    connect(this, &CodeEditor::updateRequest, this, &CodeEditor::updateLineNumberArea);
    connect(this, &CodeEditor::cursorPositionChanged, this, &CodeEditor::highlightCurrentLine);
    // The minimap's slider tracks the scrollbar directly (its own row
    // mapping decision — see minimap.h), and its caret-line overlay tracks
    // the cursor; both are cheap enough to keep live unconditionally rather
    // than connect/disconnect on every options toggle.
    //
    // `repaint()`, not `update()`: `update()` only posts a paint request,
    // coalesced with whatever else is already pending on this widget, and a
    // background pane's minimap has nothing else keeping that queue moving
    // (the visible pane's own repaints, cursor blink, etc.) — so on a
    // pane that isn't otherwise being redrawn, a plain `update()` can sit
    // unpainted for a very visible stretch after the scrollbar has already
    // moved on. Repainting synchronously here costs nothing extra: a
    // scrollbar tick fires at most once per event, and the slider/overlay
    // redraw is cheap even when the code pixmap itself is cache-hit.
    connect(verticalScrollBar(), &QScrollBar::valueChanged, minimap_,
            qOverload<>(&QWidget::repaint));
    connect(this, &CodeEditor::cursorPositionChanged, minimap_, qOverload<>(&QWidget::repaint));

    // Code is read on a horizontal scrollbar, not reflowed — the same
    // default VS Code and IntelliJ ship. It is also what keeps a
    // machine-generated file usable: QPlainTextDocumentLayout lays a block
    // out atomically, so wrapping a single 600k-character line into
    // thousands of visual lines makes every layout touch of that block cost
    // the whole line.
    setLineWrapMode(QPlainTextEdit::NoWrap);

    // Ctrl-hover feedback needs move events with no button held (N7), the
    // same reason TerminalWidget enables tracking for its links.
    setMouseTracking(true);

    // L5: the completion popup. UnfilteredPopupCompletion is the point —
    // QCompleter's own prefix matching is bypassed entirely, because which
    // items match and in what order is the server's answer, computed in
    // `lsp_core::completion` before the model is filled.
    completionModel_ = new QStandardItemModel(this);
    completer_ = new QCompleter(completionModel_, this);
    completer_->setWidget(this);
    completer_->setCompletionMode(QCompleter::UnfilteredPopupCompletion);
    connect(completer_,
            qOverload<const QModelIndex &>(&QCompleter::activated),
            this,
            [this](const QModelIndex &index) {
                const int entry = index.data(kEntryIndexRole).toInt();
                if (entry >= 0 && entry < completionEntries_.size()) {
                    insertCompletion(completionEntries_.at(entry));
                }
            });
    // C7: the popup's selection moved — ask for a resolved preview of the
    // newly highlighted row. Whether the server offers resolve at all, and
    // dropping a stale answer, are `LanguageService`'s decisions; this only
    // reports the gesture, same as `completionRequested` does for a keystroke.
    connect(completer_,
            qOverload<const QModelIndex &>(&QCompleter::highlighted),
            this,
            [this](const QModelIndex &index) {
                const int entry = index.data(kEntryIndexRole).toInt();
                if (entry >= 0 && entry < completionEntries_.size()) {
                    emit completionPreviewRequested(completionEntries_.at(entry).resolveData);
                }
            });

    updateLineNumberAreaWidth(0);
    highlightCurrentLine();
}

QString CodeEditor::textBeforeCursor() const
{
    const QTextCursor cursor = textCursor();
    return cursor.block().text().left(cursor.positionInBlock());
}

void CodeEditor::showCompletions(const QVector<CompletionEntry> &items)
{
    completionEntries_ = items;
    completionModel_->clear();
    if (items.isEmpty()) {
        hideCompletionPopup();
        return;
    }
    for (int i = 0; i < items.size(); ++i) {
        const CompletionEntry &entry = items.at(i);
        auto *row = new QStandardItem(entry.label);
        row->setEditable(false);
        row->setData(i, kEntryIndexRole);
        QStringList tooltip;
        for (const QString &part : {entry.kind, entry.detail, entry.documentation}) {
            if (!part.isEmpty()) {
                tooltip << part;
            }
        }
        row->setToolTip(tooltip.join(QStringLiteral("\n")));
        completionModel_->appendRow(row);
    }
    QAbstractItemView *popup = completer_->popup();
    popup->setCurrentIndex(completer_->completionModel()->index(0, 0));
    QRect anchor = cursorRect();
    anchor.setWidth(popup->sizeHintForColumn(0) + popup->verticalScrollBar()->sizeHint().width()
                    + kPopupWidthPadding);
    completer_->complete(anchor);
}

void CodeEditor::updateCompletionPreview(const QString &detail, const QString &documentation)
{
    if (detail.isEmpty() && documentation.isEmpty()) {
        return;
    }
    const QModelIndex current = completer_->popup()->currentIndex();
    const int entry = current.data(kEntryIndexRole).toInt();
    if (!current.isValid() || entry < 0 || entry >= completionEntries_.size()) {
        return;
    }
    // `entry` is the row's index into `completionEntries_`, which is also
    // its row index in `completionModel_` — rows are appended in that same
    // order in showCompletions() — so this is correct even if the popup
    // shows the completer's own (possibly reordered) proxy model.
    QStandardItem *row = completionModel_->item(entry);
    if (!row) {
        return;
    }
    const CompletionEntry &original = completionEntries_.at(entry);
    QStringList tooltip;
    for (const QString &part :
         {original.kind, detail.isEmpty() ? original.detail : detail,
          documentation.isEmpty() ? original.documentation : documentation}) {
        if (!part.isEmpty()) {
            tooltip << part;
        }
    }
    row->setToolTip(tooltip.join(QStringLiteral("\n")));
}

void CodeEditor::refreshCompletions()
{
    emit completionFilterChanged(textBeforeCursor());
}

void CodeEditor::hideCompletionPopup()
{
    if (!completer_->popup()->isVisible() && completionEntries_.isEmpty()) {
        return;
    }
    completer_->popup()->hide();
    completionEntries_.clear();
    emit completionCanceled();
}

void CodeEditor::insertCompletion(const CompletionEntry &entry)
{
    // By value: `entry` is a reference into completionEntries_, which the
    // splice that follows can refill before the dismissal below clears it.
    const CompletionEntry chosen = entry;
    emit completionChosen(chosen);
    hideCompletionPopup();
}

void CodeEditor::keyPressEvent(QKeyEvent *event)
{
    // While the popup is up it owns these keys; QCompleter forwards them
    // here rather than handling them itself (Qt's Custom Completer example).
    if (completer_->popup()->isVisible()) {
        switch (event->key()) {
        case Qt::Key_Enter:
        case Qt::Key_Return:
        case Qt::Key_Tab: {
            const QModelIndex current = completer_->popup()->currentIndex();
            if (current.isValid()) {
                event->accept();
                const int entry = current.data(kEntryIndexRole).toInt();
                if (entry >= 0 && entry < completionEntries_.size()) {
                    insertCompletion(completionEntries_.at(entry));
                }
                return;
            }
            break;
        }
        case Qt::Key_Escape:
            event->accept();
            hideCompletionPopup();
            return;
        default:
            break;
        }
    }

    // Ctrl+Space: ask regardless of what is typed, mid-word or not.
    if (event->key() == Qt::Key_Space && event->modifiers().testFlag(Qt::ControlModifier)) {
        event->accept();
        emit completionRequested(textCursor().position(), textBeforeCursor(), true);
        return;
    }

    // A bare modifier key press (Shift held before the letter it modifies
    // arrives, Ctrl held before a shortcut's second key) carries no meaning
    // of its own. `xdotool type`'s Shift+digit combos (e.g. "!") deliver
    // exactly this: a Key_Shift press event with no text, ahead of the
    // character it is about to shift. Treating it as "some other operation"
    // would drop the multi-caret selection before the character it is
    // actually part of ever arrives.
    switch (event->key()) {
    case Qt::Key_Shift:
    case Qt::Key_Control:
    case Qt::Key_Alt:
    case Qt::Key_AltGr:
    case Qt::Key_Meta:
        QPlainTextEdit::keyPressEvent(event);
        return;
    default:
        break;
    }

    // F1-8/F1-15: every text-producing key is a transaction computed in
    // Rust now, one caret or two hundred — smart typing (auto-close,
    // type-over, smart backspace) is stateful and lives in `edit_ops`, and
    // there is no separate "plain" path left for it to fall through. This
    // sits after the popup interception on purpose (the popup owns Enter
    // and Tab while it is up).
    if (event->key() == Qt::Key_Escape && hasSecondaryCarets()) {
        event->accept();
        emit secondaryCaretsDropped();
        return;
    }

    const QString typedText = event->text();
    const bool typed = !typedText.isEmpty() && typedText.at(0).isPrint()
      && !event->modifiers().testFlag(Qt::ControlModifier);
    const bool deleted = event->key() == Qt::Key_Backspace || event->key() == Qt::Key_Delete;
    const bool newline = event->key() == Qt::Key_Return || event->key() == Qt::Key_Enter;

    if (typed || deleted || newline) {
        event->accept();
        if (newline) {
            emit multiCaretNewline();
            // A newline leaves the word the popup describes, same as an
            // ordinary caret move.
            hideCompletionPopup();
            return;
        }
        if (typed) {
            emit multiCaretTyped(typedText);
        } else if (event->key() == Qt::Key_Backspace) {
            emit multiCaretBackspace();
        } else {
            emit multiCaretDelete();
        }
        if (completer_->popup()->isVisible()) {
            refreshCompletions();
        }
        if (typed) {
            // Fired on every character: whether it is worth a request — a
            // trigger character, enough of a word, a list already in
            // hand — is `lsp_core::completion`'s decision, not this
            // widget's.
            emit completionRequested(textCursor().position(), textBeforeCursor(), false);
        }
        return;
    }

    // Everything else — arrows, Home, End, a shortcut — is not a
    // multi-caret operation in this version: the extra carets are dropped
    // and the key does exactly what it always did, which is a stated
    // ceiling (ADR-0023). Moving N carets is its own rule and belongs in
    // `editor_core::selection`, not here.
    if (hasSecondaryCarets()) {
        emit secondaryCaretsDropped();
    }

    QPlainTextEdit::keyPressEvent(event);
    // A caret move leaves the word the popup describes, so the list stops
    // being about anything.
    hideCompletionPopup();
}

void CodeEditor::insertFromMimeData(const QMimeData *source)
{
    if (source->hasText()) {
        emit pasteRequested(source->text());
        return;
    }
    QPlainTextEdit::insertFromMimeData(source);
}

void CodeEditor::focusOutEvent(QFocusEvent *event)
{
    hideCompletionPopup();
    QPlainTextEdit::focusOutEvent(event);
}

void CodeEditor::contextMenuEvent(QContextMenuEvent *event)
{
    // Move the caret to what was right-clicked, unless the click landed
    // inside an existing selection (where taking it away would throw away
    // what the user picked out). Every gesture in the menu acts on the
    // caret, so without this a right-click on one symbol would rename the
    // one the caret happened to be on.
    QTextCursor clicked = cursorForPosition(event->pos());
    const QTextCursor current = textCursor();
    const bool insideSelection = current.hasSelection()
      && clicked.position() >= current.selectionStart()
      && clicked.position() <= current.selectionEnd();
    if (!insideSelection) {
        setTextCursor(clicked);
    }

    QMenu *menu = createStandardContextMenu(event->pos());
    emit contextMenuAboutToShow(menu);
    menu->exec(event->globalPos());
    delete menu;
}

QPair<int, int> CodeEditor::identifierAt(const QPoint &pos) const
{
    QTextCursor cursor = cursorForPosition(pos);
    cursor.select(QTextCursor::WordUnderCursor);
    const QString word = cursor.selectedText();
    if (word.isEmpty()) {
        return {-1, -1};
    }
    const QChar first = word.at(0);
    if (!first.isLetter() && first != QLatin1Char('_')) {
        return {-1, -1};
    }
    return {cursor.selectionStart(), cursor.selectionEnd()};
}

void CodeEditor::updateHoverSpan(const QPoint &pos, bool ctrlHeld)
{
    const QPair<int, int> span = ctrlHeld ? identifierAt(pos) : QPair<int, int>{-1, -1};
    if (span == hoverSpan_) {
        return;
    }
    hoverSpan_ = span;
    viewport()->setCursor(hoverSpan_.first >= 0 ? Qt::PointingHandCursor : Qt::IBeamCursor);
    highlightCurrentLine();
}

void CodeEditor::clearHoverSpan()
{
    updateHoverSpan(QPoint(), false);
}

void CodeEditor::mouseMoveEvent(QMouseEvent *event)
{
    if (columnDragging_ && event->buttons().testFlag(Qt::LeftButton)) {
        event->accept();
        emit columnSelectRequested(columnAnchor_, cursorForPosition(event->pos()).position());
        return;
    }
    updateHoverSpan(event->pos(), event->modifiers().testFlag(Qt::ControlModifier));
    cancelHover();
    QPlainTextEdit::mouseMoveEvent(event);
}

void CodeEditor::mouseReleaseEvent(QMouseEvent *event)
{
    if (columnDragging_) {
        columnDragging_ = false;
        event->accept();
        return;
    }
    QPlainTextEdit::mouseReleaseEvent(event);
}

void CodeEditor::inputMethodEvent(QInputMethodEvent *event)
{
    if (hasSecondaryCarets()) {
        emit secondaryCaretsDropped();
    }
    QPlainTextEdit::inputMethodEvent(event);
}

void CodeEditor::setSecondaryCarets(const QVector<SecondaryCaret> &carets)
{
    if (carets == secondaryCarets_) {
        return;
    }
    secondaryCarets_ = carets;
    // The selections ride on the same extra-selection list everything else
    // does; the bars are painted in paintEvent, which this repaints for.
    highlightCurrentLine();
    viewport()->update();
}

void CodeEditor::paintEvent(QPaintEvent *event)
{
    QPlainTextEdit::paintEvent(event);
    if (!secondaryCarets_.isEmpty()) {
        QPainter painter(viewport());
        const QColor caretColor = palette().color(QPalette::Text);
        const int width = qMax(1, cursorWidth());
        for (const SecondaryCaret &caret : secondaryCarets_) {
            QTextCursor cursor(document());
            cursor.setPosition(qBound(0, caret.head, document()->characterCount() - 1));
            const QRect rect = cursorRect(cursor);
            painter.fillRect(QRect(rect.left(), rect.top(), width, rect.height()), caretColor);
        }
    }

    // F2-11: inlay hints, off unless the user turned them on
    // (code.toggleInlayHints) — a hint is text the server invented, not
    // text in the file, so it defaults to not being drawn at all.
    if (inlayHintsEnabled_ && !inlayHints_.isEmpty()) {
        QPainter painter(viewport());
        QFont hintFont = font();
        hintFont.setPointSizeF(hintFont.pointSizeF() * 0.85);
        painter.setFont(hintFont);
        const QColor hintColor = tinted(palette().color(QPalette::Text), 100, 100);
        const QColor hintBackground = tinted(palette().color(QPalette::Base), 100, 108);
        const int maxPosition = document()->characterCount() - 1;
        for (const InlayHintSpan &hint : inlayHints_) {
            if (hint.position < 0 || hint.position > maxPosition) {
                continue;
            }
            QTextCursor cursor(document());
            cursor.setPosition(hint.position);
            const QRect rect = cursorRect(cursor);
            const QString text = (hint.paddingLeft ? QStringLiteral(" ") : QString())
              + hint.label + (hint.paddingRight ? QStringLiteral(" ") : QString());
            const QRect textRect(rect.left(), rect.top(),
                                 painter.fontMetrics().horizontalAdvance(text) + 4,
                                 rect.height());
            painter.fillRect(textRect, hintBackground);
            painter.setPen(hintColor);
            painter.drawText(textRect, Qt::AlignVCenter | Qt::AlignLeft,
                             QStringLiteral(" ") + text);
        }
    }

    // C10-followup: one pill per lens, after its line's text. Rebuilt every
    // paint, keyed by index into codeLenses_ for mousePressEvent's hit test.
    codeLensClickRects_.clear();
    if (!codeLenses_.isEmpty()) {
        QPainter painter(viewport());
        QFont lensFont = font();
        lensFont.setPointSizeF(lensFont.pointSizeF() * 0.85);
        painter.setFont(lensFont);
        const QColor lensColor = tinted(palette().color(QPalette::Text), 100, 100);
        const QColor lensBg = tinted(palette().color(QPalette::Base), 100, 108);
        const int maxBlock = document()->blockCount() - 1;
        for (int i = 0; i < codeLenses_.size(); ++i) {
            const CodeLensSpan &lens = codeLenses_.at(i);
            if (lens.line < 0 || lens.line > maxBlock) {
                continue;
            }
            const QTextBlock block = document()->findBlockByNumber(lens.line);
            if (!block.isValid() || !block.isVisible()) {
                continue;
            }
            const QRect lineRect = blockBoundingGeometry(block).translated(contentOffset()).toRect();
            const int textEnd = lineRect.left() + fontMetrics().horizontalAdvance(block.text());
            const QString text = QStringLiteral(" ") + lens.label + QStringLiteral(" ");
            const QRect textRect(textEnd + 12, lineRect.top(),
                                 painter.fontMetrics().horizontalAdvance(text), lineRect.height());
            painter.fillRect(textRect, lensBg);
            painter.setPen(lensColor);
            painter.drawText(textRect, Qt::AlignVCenter | Qt::AlignLeft, text);
            if (lens.clickable) {
                codeLensClickRects_.insert(i, textRect);
            }
        }
    }

    // D3-7: the stopped frame's values, after the line's text and past
    // whatever a code lens already put there. Which value belongs on which
    // line is `dap_core::inline_values`' answer, arriving through
    // `EditorTabs`; this paints where it was told to.
    if (!inlineValues_.isEmpty()) {
        QPainter painter(viewport());
        QFont valueFont = font();
        valueFont.setItalic(true);
        valueFont.setPointSizeF(valueFont.pointSizeF() * 0.85);
        painter.setFont(valueFont);
        const QColor valueColor = tinted(palette().color(QPalette::Text), 100, 120);
        const int maxBlock = document()->blockCount() - 1;
        for (const InlineValueSpan &value : inlineValues_) {
            if (value.line < 0 || value.line > maxBlock) {
                continue;
            }
            const QTextBlock block = document()->findBlockByNumber(value.line);
            if (!block.isValid() || !block.isVisible()) {
                continue;
            }
            const QRect lineRect = blockBoundingGeometry(block).translated(contentOffset()).toRect();
            const int textEnd = lineRect.left() + fontMetrics().horizontalAdvance(block.text());
            const QRect textRect(textEnd + 24, lineRect.top(),
                                 painter.fontMetrics().horizontalAdvance(value.text) + 8,
                                 lineRect.height());
            painter.setPen(valueColor);
            painter.drawText(textRect, Qt::AlignVCenter | Qt::AlignLeft, value.text);
        }
    }

    // Show-whitespace-characters task: off by default, like inlay hints
    // above, and for the same reason — a glyph that isn't in the file
    // should cost nothing to a user who never turned it on.
    if (whitespaceOptions_.enabled || whitespaceOptions_.eolMarkers) {
        paintWhitespace();
    }
}

bool CodeEditor::viewportEvent(QEvent *event)
{
    if (event->type() == QEvent::ToolTip) {
        const QPair<int, int> span = identifierAt(static_cast<QHelpEvent *>(event)->pos());
        if (span.first >= 0) {
            hoverPending_ = true;
            emit hoverRequested(span.first);
        }
        // Accepted either way: the default handler would only offer this
        // widget's (empty) static tooltip, and a server's answer arrives
        // later, on hoverReady.
        event->accept();
        return true;
    }
    return QPlainTextEdit::viewportEvent(event);
}

void CodeEditor::cancelHover()
{
    if (!hoverPending_) {
        return;
    }
    hoverPending_ = false;
    emit hoverCanceled();
}

void CodeEditor::mousePressEvent(QMouseEvent *event)
{
    // C10-followup: checked first so a lens click can't also place a caret.
    for (auto it = codeLensClickRects_.cbegin();
         event->button() == Qt::LeftButton && it != codeLensClickRects_.cend(); ++it) {
        if (it.value().contains(event->pos())) {
            event->accept();
            emit codeLensClicked(it.key());
            return;
        }
    }

    // F1-15. Checked before Ctrl+Click so the two gestures cannot both fire
    // on an Alt+Ctrl+Click; adding a caret is the more specific one.
    if (event->button() == Qt::LeftButton && event->modifiers().testFlag(Qt::AltModifier)) {
        const int position = cursorForPosition(event->pos()).position();
        event->accept();
        if (event->modifiers().testFlag(Qt::ShiftModifier)) {
            columnAnchor_ = position;
            columnDragging_ = true;
            emit columnSelectRequested(position, position);
        } else {
            emit caretAddRequested(position);
        }
        return;
    }
    // A plain click puts the caret somewhere, which is an answer to "where
    // is the caret" — so any extra ones stop meaning anything.
    if (event->button() == Qt::LeftButton && hasSecondaryCarets()) {
        emit secondaryCaretsDropped();
    }

    if (event->button() == Qt::LeftButton && event->modifiers().testFlag(Qt::ControlModifier)) {
        const QPair<int, int> span = identifierAt(event->pos());
        if (span.first >= 0) {
            clearHoverSpan();
            // Accepted before the base class runs, so a Ctrl+Click that
            // navigates does not also drag out a selection.
            event->accept();
            emit declarationRequested(span.first);
            return;
        }
    }
    QPlainTextEdit::mousePressEvent(event);
}

void CodeEditor::leaveEvent(QEvent *event)
{
    clearHoverSpan();
    cancelHover();
    QPlainTextEdit::leaveEvent(event);
}

void CodeEditor::wheelEvent(QWheelEvent *event)
{
    if (!(event->modifiers() & Qt::ControlModifier)) {
        QPlainTextEdit::wheelEvent(event);
        return;
    }

    const int steps = event->angleDelta().y() / 120;
    if (steps == 0) {
        return;
    }
    QFont zoomed = font();
    zoomed.setPointSize(std::clamp(zoomed.pointSize() + steps, 6, 72));
    setFont(zoomed);
    event->accept();
}

void CodeEditor::setCurrentLineColor(const QString &hex)
{
    currentLineColor_ = hex;
    highlightCurrentLine();
}

QColor CodeEditor::currentLineBandColor() const
{
    return currentLineColor_.isEmpty() ? tinted(palette().color(QPalette::Base), 145, 108)
                                       : QColor(currentLineColor_);
}

void CodeEditor::setMatchSelections(const QVector<QPair<int, int>> &matches, int currentMatch)
{
    matchSelections_ = matches;
    currentMatch_ = currentMatch;
    // Match highlights ride on the same extra-selection list as the
    // current-line band, so both are (re)applied from one place.
    highlightCurrentLine();
}

void CodeEditor::setDiagnosticSpans(const QVector<DiagnosticSpan> &spans)
{
    diagnosticSpans_ = spans;
    // Same one place every other extra selection is (re)applied from.
    highlightCurrentLine();
}

void CodeEditor::setOccurrenceSpans(const QVector<OccurrenceSpan> &spans)
{
    occurrenceSpans_ = spans;
    highlightCurrentLine();
}

void CodeEditor::setDiffSelections(const QVector<DiffLineBackground> &backgrounds,
                                   const QVector<DiffInlineSpan> &spans)
{
    diffBackgrounds_ = backgrounds;
    diffSpans_ = spans;
    highlightCurrentLine();
}

void CodeEditor::setInlayHints(const QVector<InlayHintSpan> &hints)
{
    inlayHints_ = hints;
    viewport()->update();
}

void CodeEditor::setInlayHintsEnabled(bool enabled)
{
    inlayHintsEnabled_ = enabled;
    viewport()->update();
}

void CodeEditor::setInlineValues(const QVector<InlineValueSpan> &values)
{
    if (inlineValues_ == values) {
        return;
    }
    inlineValues_ = values;
    viewport()->update();
}

void CodeEditor::setCodeLenses(const QVector<CodeLensSpan> &lenses)
{
    codeLenses_ = lenses;
    viewport()->update();
}

void CodeEditor::setWhitespaceOptions(const WhitespaceOptions &options)
{
    whitespaceOptions_ = options;
    viewport()->update();
}

void CodeEditor::setMinimapOptions(const MinimapOptions &options)
{
    const bool visibilityChanged = minimap_->options().enabled != options.enabled;
    minimap_->setOptions(options);
    minimap_->setVisible(options.enabled);
    if (visibilityChanged) {
        // The strip's own width changes with visibility, so the viewport
        // margin and the strip's own geometry — both set from resizeEvent,
        // the same "widen for a column that comes and goes" trick
        // runMarkerWidth uses — have to be recomputed, not just repainted.
        updateLineNumberAreaWidth(0);
        layoutMinimap();
        e2eMark(QStringLiteral("{\"ev\":\"minimap_visible\",\"enabled\":%1}")
                  .arg(options.enabled ? QLatin1String("true") : QLatin1String("false")));
    }
}

int CodeEditor::minimapWidth() const
{
    return minimap_->options().enabled ? Minimap::preferredWidth() : 0;
}

void CodeEditor::setWhitespaceClassifier(WhitespaceClassifier classifier)
{
    whitespaceClassifier_ = std::move(classifier);
}

void CodeEditor::setEditorTabWidth(int columns)
{
    tabWidthColumns_ = qMax(1, columns);
    refreshTabStopDistance();
}

void CodeEditor::refreshTabStopDistance()
{
    setTabStopDistance(fontMetrics().horizontalAdvance(QLatin1Char(' ')) * tabWidthColumns_);
}

namespace {

// A small filled dot, centered in [start, end)'s cell — the space glyph.
void paintSpaceGlyph(QPainter &painter, const QRect &start, const QRect &end)
{
    const int cx = (start.left() + qMax(end.left(), start.left() + 2)) / 2;
    const int cy = start.center().y();
    const int r = qMax(1, start.height() / 10);
    painter.drawEllipse(QPoint(cx, cy), r, r);
}

// A right-pointing arrow spanning [start, end)'s cell — the tab glyph. The
// cell's width already reflects `setTabStopDistance` (Qt's own layout, not
// anything computed here), so the arrow visually ends where the tab does.
void paintTabGlyph(QPainter &painter, const QRect &start, const QRect &end)
{
    const int y = start.center().y();
    const int x1 = start.left() + 2;
    const int x2 = qMax(x1 + 4, end.left() - 3);
    painter.drawLine(x1, y, x2, y);
    const QPolygon arrow{QPoint(x2, y - 3), QPoint(x2, y + 3), QPoint(x2 + 3, y)};
    painter.drawPolygon(arrow);
}

} // namespace

void CodeEditor::paintWhitespace()
{
    QTextBlock block = firstVisibleBlock();
    if (!block.isValid()) {
        return;
    }
    const int firstBlockNumber = block.blockNumber();
    QStringList lines;
    QVector<int> blockNumbers;
    int top = qRound(blockBoundingGeometry(block).translated(contentOffset()).top());
    const int viewportBottom = viewport()->rect().bottom();
    while (block.isValid() && top <= viewportBottom) {
        if (block.isVisible()) {
            lines.append(block.text());
            blockNumbers.append(block.blockNumber());
        }
        top += qRound(blockBoundingRect(block).height());
        block = block.next();
    }
    if (blockNumbers.isEmpty()) {
        return;
    }
    const int lastBlockNumber = blockNumbers.last();

    if (whitespaceOptions_.enabled && whitespaceClassifier_) {
        // Simple "recompute on revision or visible-range change" cache
        // (documented on whitespaceCache*_ in the header): cheap to check,
        // and it turns "one classifier call per paint" into "one per
        // scroll step or edit".
        const int revision = document()->revision();
        if (revision != whitespaceCacheRevision_ || firstBlockNumber != whitespaceCacheFirstBlock_
            || lastBlockNumber != whitespaceCacheLastBlock_) {
            whitespaceCache_ = whitespaceClassifier_(lines.join(QLatin1Char('\n')));
            whitespaceCacheRevision_ = revision;
            whitespaceCacheFirstBlock_ = firstBlockNumber;
            whitespaceCacheLastBlock_ = lastBlockNumber;
        }

        QPainter painter(viewport());
        const QColor glyphColor = tinted(palette().color(QPalette::Text), 100, 145);
        painter.setPen(glyphColor);
        painter.setBrush(glyphColor);
        const int maxPosition = document()->characterCount() - 1;
        for (const WhitespaceSpan &span : std::as_const(whitespaceCache_)) {
            const bool categoryOn = (span.category == 0 && whitespaceOptions_.leading)
              || (span.category == 1 && whitespaceOptions_.inner)
              || (span.category == 2 && whitespaceOptions_.trailing);
            if (!categoryOn) {
                continue;
            }
            const QTextBlock lineBlock =
              document()->findBlockByNumber(firstBlockNumber + span.line);
            if (!lineBlock.isValid() || !lineBlock.isVisible()) {
                continue;
            }
            const int startPos = qBound(0, lineBlock.position() + span.column, maxPosition);
            const int endPos = qBound(0, startPos + 1, maxPosition);
            QTextCursor startCursor(document());
            startCursor.setPosition(startPos);
            QTextCursor endCursor(document());
            endCursor.setPosition(endPos);
            const QRect startRect = cursorRect(startCursor);
            const QRect endRect = cursorRect(endCursor);
            if (span.isTab) {
                paintTabGlyph(painter, startRect, endRect);
            } else {
                paintSpaceGlyph(painter, startRect, endRect);
            }
        }
    }

    if (whitespaceOptions_.eolMarkers) {
        QPainter painter(viewport());
        painter.setPen(tinted(palette().color(QPalette::Text), 100, 145));
        const int maxPosition = document()->characterCount() - 1;
        for (int blockNumber : std::as_const(blockNumbers)) {
            const QTextBlock lineBlock = document()->findBlockByNumber(blockNumber);
            if (!lineBlock.isValid() || !lineBlock.isVisible()) {
                continue;
            }
            const int endPos = qBound(0, lineBlock.position() + lineBlock.length() - 1, maxPosition);
            QTextCursor cursor(document());
            cursor.setPosition(endPos);
            const QRect rect = cursorRect(cursor);
            const QRect markerRect(rect.right() + 2, rect.top(),
                                   painter.fontMetrics().horizontalAdvance(QChar(0xB6)) + 2,
                                   rect.height());
            painter.drawText(markerRect, Qt::AlignVCenter | Qt::AlignLeft, QString(QChar(0xB6)));
        }
    }
}

void CodeEditor::highlightCurrentLine()
{
    QList<QTextEdit::ExtraSelection> selections;

    QTextEdit::ExtraSelection line;
    line.format.setBackground(currentLineBandColor());
    // Without this the band stops at the end of the text on that line.
    line.format.setProperty(QTextFormat::FullWidthSelection, true);
    line.cursor = textCursor();
    line.cursor.clearSelection();
    selections.append(line);

    // Diff backgrounds sit over the current-line band and under everything
    // that marks a *position* (matches, occurrences, carets).
    selections.append(diffSelections(document(), diffBackgrounds_, diffSpans_));

    const QColor matchColor = tinted(palette().color(QPalette::Base), 190, 135);
    const QColor currentMatchColor = tinted(palette().color(QPalette::Base), 260, 175);
    for (int i = 0; i < matchSelections_.size(); ++i) {
        QTextEdit::ExtraSelection match;
        match.format.setBackground(i == currentMatch_ ? currentMatchColor : matchColor);
        match.cursor = textCursor();
        match.cursor.setPosition(matchSelections_[i].first);
        match.cursor.setPosition(matchSelections_[i].second, QTextCursor::KeepAnchor);
        selections.append(match);
    }

    if (hoverSpan_.first >= 0) {
        QTextEdit::ExtraSelection hover;
        hover.format.setFontUnderline(true);
        hover.format.setUnderlineStyle(QTextCharFormat::SingleUnderline);
        hover.cursor = textCursor();
        hover.cursor.setPosition(hoverSpan_.first);
        hover.cursor.setPosition(hoverSpan_.second, QTextCursor::KeepAnchor);
        selections.append(hover);
    }

    for (const SecondaryCaret &caret : secondaryCarets_) {
        if (caret.anchor == caret.head) {
            continue;
        }
        QTextEdit::ExtraSelection secondary;
        secondary.format.setBackground(palette().color(QPalette::Highlight));
        secondary.format.setForeground(palette().color(QPalette::HighlightedText));
        secondary.cursor = textCursor();
        secondary.cursor.setPosition(caret.anchor);
        secondary.cursor.setPosition(caret.head, QTextCursor::KeepAnchor);
        selections.append(secondary);
    }

    const QColor readColor = tinted(palette().color(QPalette::Base), 205, 190);
    const QColor writeColor = tinted(palette().color(QPalette::Base), 230, 165);
    for (const OccurrenceSpan &span : occurrenceSpans_) {
        QTextEdit::ExtraSelection occurrence;
        occurrence.format.setBackground(span.isWrite ? writeColor : readColor);
        occurrence.cursor = textCursor();
        occurrence.cursor.setPosition(span.start);
        occurrence.cursor.setPosition(span.end, QTextCursor::KeepAnchor);
        selections.append(occurrence);
    }

    for (const DiagnosticSpan &span : diagnosticSpans_) {
        QTextEdit::ExtraSelection diagnostic;
        diagnostic.format.setUnderlineStyle(QTextCharFormat::SpellCheckUnderline);
        diagnostic.format.setUnderlineColor(span.color);
        diagnostic.cursor = textCursor();
        diagnostic.cursor.setPosition(span.start);
        diagnostic.cursor.setPosition(span.end, QTextCursor::KeepAnchor);
        selections.append(diagnostic);
    }

    setExtraSelections(selections);
    lineNumberArea_->update();
    minimap_->update();
}

void CodeEditor::changeEvent(QEvent *event)
{
    QPlainTextEdit::changeEvent(event);

    // MainWindow swaps the editor palette when the theme changes; the band
    // colour is derived from it, so it has to be recomputed here.
    if (event->type() == QEvent::PaletteChange) {
        highlightCurrentLine();
    }
    // Show-whitespace-characters task: the tab stop distance is computed
    // from the font's own space width, so a font change (Settings > Editor,
    // live preview) leaves it stale until this recomputes it.
    if (event->type() == QEvent::FontChange) {
        refreshTabStopDistance();
    }
}

void CodeEditor::lineNumberAreaContextMenuEvent(QContextMenuEvent *event)
{
    QMenu menu(this);
    QAction *collapse = menu.addAction(tr("Collapse All"));
    QAction *expand = menu.addAction(tr("Expand All"));
    QAction *chosen = menu.exec(lineNumberArea_->mapToGlobal(event->pos()));
    if (chosen == collapse) {
        collapseAll();
    } else if (chosen == expand) {
        expandAll();
    }
}

bool CodeEditor::foldStartingAt(int blockNumber, FoldRange *out) const
{
    const auto it = foldStarts_.constFind(blockNumber);
    if (it == foldStarts_.constEnd()) {
        return false;
    }
    *out = it.value();
    return true;
}

void CodeEditor::setChangeMarkers(const QVector<ChangeMarker> &markers)
{
    changeMarkers_.clear();
    for (const ChangeMarker &marker : markers) {
        changeMarkers_.insert(marker.block, marker);
    }
    lineNumberArea_->update();
    minimap_->update();
}

bool CodeEditor::changeMarkerAt(int blockNumber, ChangeMarker *out) const
{
    const auto it = changeMarkers_.constFind(blockNumber);
    if (it == changeMarkers_.constEnd()) {
        return false;
    }
    *out = it.value();
    return true;
}

void CodeEditor::setBlameAnnotations(const QVector<BlameAnnotation> &annotations)
{
    blameAnnotations_.clear();
    for (const BlameAnnotation &annotation : annotations) {
        blameAnnotations_.insert(annotation.block, annotation.text);
    }
    lineNumberArea_->update();
}

void CodeEditor::setBlameEnabled(bool enabled)
{
    if (blameEnabled_ == enabled) {
        return;
    }
    blameEnabled_ = enabled;
    updateLineNumberAreaWidth(0);
    lineNumberArea_->update();
}

void CodeEditor::setBlocksVisible(int fromBlockExclusive, int toBlockInclusive, bool visible)
{
    setLinesVisible(this, fromBlockExclusive, toBlockInclusive, visible);
    lineNumberArea_->update();
}

void CodeEditor::toggleFold(int blockNumber)
{
    FoldRange range;
    if (!foldStartingAt(blockNumber, &range)) {
        return;
    }

    const int idx = collapsedRanges_.indexOf(range);
    const bool collapsing = idx < 0;
    setBlocksVisible(range.startBlock, range.endBlock, !collapsing);

    if (collapsing) {
        collapsedRanges_.append(range);
    } else {
        collapsedRanges_.removeAt(idx);
    }
}

void CodeEditor::collapseAll()
{
    for (auto it = foldStarts_.constBegin(); it != foldStarts_.constEnd(); ++it) {
        if (!collapsedRanges_.contains(it.value())) {
            toggleFold(it.value().anchorBlock);
        }
    }
}

void CodeEditor::expandAll()
{
    for (auto it = foldStarts_.constBegin(); it != foldStarts_.constEnd(); ++it) {
        if (collapsedRanges_.contains(it.value())) {
            toggleFold(it.value().anchorBlock);
        }
    }
}

void CodeEditor::ensureBlockVisible(int blockNumber)
{
    QVector<FoldRange> stillCollapsed;
    bool expandedAny = false;
    for (const FoldRange &range : collapsedRanges_) {
        // The fold's header line stays visible while collapsed, so only the
        // blocks *after* it are actually hidden.
        if (blockNumber > range.startBlock && blockNumber <= range.endBlock) {
            setBlocksVisible(range.startBlock, range.endBlock, true);
            expandedAny = true;
        } else {
            stillCollapsed.append(range);
        }
    }
    if (!expandedAny) {
        return;
    }

    collapsedRanges_ = stillCollapsed;
    // Expanding an enclosing range above also revealed any fold nested
    // inside it that the target line isn't in — re-hide those, so only the
    // folds standing between the cursor and visibility get opened.
    for (const FoldRange &range : collapsedRanges_) {
        setBlocksVisible(range.startBlock, range.endBlock, false);
    }
}

void CodeEditor::setFoldRanges(const QVector<FoldRange> &ranges)
{
    foldRanges_ = ranges;

    // Index by start block once per tree update, rather than scanning per
    // painted gutter line. Only the first range starting on a given line is
    // kept, which is what the previous linear scan returned for nested
    // constructs opening on the same line.
    foldStarts_.clear();
    foldStarts_.reserve(foldRanges_.size());
    for (const FoldRange &range : foldRanges_) {
        if (range.endBlock > range.startBlock && !foldStarts_.contains(range.anchorBlock)) {
            foldStarts_.insert(range.anchorBlock, range);
        }
    }

    // Edits can reshape or remove a previously-collapsed region; restore
    // visibility for any collapsed range that no longer matches an actual
    // fold, rather than leaving text permanently hidden (Task C: fold
    // state is view-only and must never orphan hidden text).
    QVector<FoldRange> stillCollapsed;
    for (const FoldRange &collapsed : collapsedRanges_) {
        if (foldRanges_.contains(collapsed)) {
            stillCollapsed.append(collapsed);
        } else {
            setBlocksVisible(collapsed.startBlock, collapsed.endBlock, true);
        }
    }
    collapsedRanges_ = stillCollapsed;

    lineNumberArea_->update();
}

} // namespace ui_shell
