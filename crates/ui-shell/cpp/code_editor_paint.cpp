// CodeEditor's paint-only surface: paintEvent (text, secondary carets, inlay
// hints, code lenses, inline values, the wrap guide and whitespace glyphs)
// and highlightCurrentLine (every extra-selection layer — current line,
// diff, matches, hover, secondary-caret selections, occurrences, bracket
// pairs, diagnostics). Its own translation unit for the reason
// editing_actions.cpp was split out of main_window.cpp: this is CodeEditor's
// entire drawing surface, self-contained and the largest coherent chunk of
// the file, and moving it here keeps code_editor.cpp under the file-size
// ceiling without splitting the class itself.

#include "code_editor.h"

#include "diff_pane.h"
#include "theme.h"

#include <QPainter>
#include <QPalette>
#include <QPolygon>
#include <QTextBlock>
#include <QTextCursor>
#include <QTextDocument>
#include <QTextEdit>

#include <utility>

namespace ui_shell {

void CodeEditor::paintEvent(QPaintEvent *event)
{
    QPlainTextEdit::paintEvent(event);

    // R1: the wrap guide — a plain vertical line at the configured column,
    // the same IntelliJ-style "right margin" marker regardless of whether
    // soft wrap itself is on.
    if (wrapColumn_ > 0) {
        QPainter painter(viewport());
        const int x = qRound(contentOffset().x()
                             + fontMetrics().horizontalAdvance(QLatin1Char(' ')) * wrapColumn_);
        painter.setPen(tinted(palette().color(QPalette::Text), 100, 135));
        painter.drawLine(x, 0, x, viewport()->height());
    }

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

    // R1: the bracket pair under the caret — the ordinary pair colour when
    // it has a partner, the error colour when it does not.
    const QColor pairColor = tinted(palette().color(QPalette::Base), 175, 140);
    const QColor pairErrorColor = QColor(224, 90, 90, 150);
    for (const BracketPairSpan &span : bracketPairSpans_) {
        QTextEdit::ExtraSelection pair;
        pair.format.setBackground(span.matched ? pairColor : pairErrorColor);
        pair.cursor = textCursor();
        pair.cursor.setPosition(span.start);
        pair.cursor.setPosition(span.end, QTextCursor::KeepAnchor);
        selections.append(pair);
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

} // namespace ui_shell
