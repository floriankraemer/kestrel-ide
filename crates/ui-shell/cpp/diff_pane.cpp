#include "diff_pane.h"

#include "syntax_highlighter.h"
#include "theme.h"
#include "ui_tokens.h"

#include <QFontDatabase>
#include <QPaintEvent>
#include <QPainter>
#include <QResizeEvent>
#include <QScrollBar>
#include <QTextBlock>

#include <algorithm>

namespace ui_shell {

void setLinesVisible(QPlainTextEdit *edit, int fromExclusive, int toInclusive, bool visible)
{
    QTextBlock block = edit->document()->findBlockByNumber(fromExclusive).next();
    while (block.isValid() && block.blockNumber() <= toInclusive) {
        block.setVisible(visible);
        block.setLineCount(visible ? 1 : 0);
        block = block.next();
    }
    const QTextBlock startBlock = edit->document()->findBlockByNumber(fromExclusive);
    const QTextBlock endBlock = edit->document()->findBlockByNumber(toInclusive);
    if (startBlock.isValid() && endBlock.isValid()) {
        edit->document()->markContentsDirty(
          startBlock.position(), endBlock.position() + endBlock.length() - startBlock.position());
    }
    edit->viewport()->update();
}

int blockTopIn(const QPlainTextEdit *edit, int block)
{
    const QTextBlock textBlock = edit->document()->findBlockByNumber(block);
    if (!textBlock.isValid()) {
        return 0;
    }
    return edit->cursorRect(QTextCursor(textBlock)).top();
}

int firstVisibleBlockIn(const QPlainTextEdit *edit)
{
    return edit->cursorForPosition(QPoint(0, 0)).blockNumber();
}

int lastVisibleBlockIn(const QPlainTextEdit *edit)
{
    return edit->cursorForPosition(QPoint(0, edit->viewport()->height() - 1)).blockNumber();
}

FoldHint::FoldHint(int lineCount, QWidget *viewport)
  : QPushButton(QObject::tr("%1 unchanged lines").arg(lineCount), viewport)
{
    setFlat(true);
    setCursor(Qt::PointingHandCursor);
    setFocusPolicy(Qt::NoFocus);
    setToolTip(QObject::tr("Expand"));
    // A placeholder row, not a button: the gutter's shade edge to edge, the
    // label dimmed like a line number, no chrome of its own.
    const QColor ground = tinted(viewport->palette().color(QPalette::Base), 130, 106);
    QColor ink = viewport->palette().color(QPalette::Text);
    ink.setAlpha(150);
    setStyleSheet(QStringLiteral("QPushButton { background: %1; color: %2; border: none; "
                                 "border-radius: 0; padding: 0; margin: 0; }")
                    .arg(ground.name(QColor::HexArgb), ink.name(QColor::HexArgb)));
}

// No Q_OBJECT: forwards paint events to the pane, uses no signals/slots —
// the same shape as `CodeEditor`'s `LineNumberArea`.
class DiffPane::Gutter : public QWidget
{
public:
    explicit Gutter(DiffPane *pane)
      : QWidget(pane)
      , pane_(pane)
    {
    }

    QSize sizeHint() const override { return QSize(pane_->gutterWidth(), 0); }

protected:
    void paintEvent(QPaintEvent *event) override { pane_->paintGutter(event); }

private:
    DiffPane *pane_;
};

DiffPane::DiffPane(const QString &text, const QString &fileName, QWidget *parent)
  : QPlainTextEdit(text, parent)
  , gutter_(new Gutter(this))
{
    setReadOnly(true);
    setFont(QFontDatabase::systemFont(QFontDatabase::FixedFont));
    setLineWrapMode(QPlainTextEdit::NoWrap);
    setFrameStyle(QFrame::NoFrame);
    if (!fileName.isEmpty()) {
        new SyntaxHighlighter(document(), fileName);
    }
    connect(this, &QPlainTextEdit::blockCountChanged, this, [this](int) { updateGutterWidth(); });
    connect(this, &QPlainTextEdit::updateRequest, this, [this](const QRect &rect, int dy) {
        if (dy != 0) {
            gutter_->scroll(0, dy);
        } else {
            gutter_->update(0, rect.y(), gutter_->width(), rect.height());
        }
    });
    updateGutterWidth();
}

void DiffPane::setGutterLabels(const QVector<QString> &labels)
{
    labels_ = labels;
    updateGutterWidth();
    gutter_->update();
}

void DiffPane::setDiffSelections(const QVector<DiffLineBackground> &backgrounds,
                                 const QVector<DiffInlineSpan> &spans)
{
    setExtraSelections(diffSelections(document(), backgrounds, spans));
}

QString DiffPane::labelFor(int block) const
{
    if (block < labels_.size()) {
        return labels_[block];
    }
    return labels_.isEmpty() ? QString::number(block + 1) : QString();
}

int DiffPane::gutterWidth() const
{
    int widest = 0;
    if (labels_.isEmpty()) {
        widest = fontMetrics().horizontalAdvance(QString::number(std::max(1, blockCount())));
    } else {
        for (const QString &label : labels_) {
            widest = std::max(widest, fontMetrics().horizontalAdvance(label));
        }
    }
    return widest + 2 * tokens::kSp2;
}

void DiffPane::updateGutterWidth()
{
    setViewportMargins(gutterWidth(), 0, 0, 0);
    const QRect cr = contentsRect();
    gutter_->setGeometry(QRect(cr.left(), cr.top(), gutterWidth(), cr.height()));
}

void DiffPane::resizeEvent(QResizeEvent *event)
{
    QPlainTextEdit::resizeEvent(event);
    updateGutterWidth();
}

void DiffPane::paintGutter(QPaintEvent *event)
{
    const QColor base = palette().color(QPalette::Base);
    QColor digitColor = palette().color(QPalette::Text);
    digitColor.setAlpha(140); // dimmed: numbers are chrome, not content

    QPainter painter(gutter_);
    painter.fillRect(event->rect(), tinted(base, 130, 106));
    painter.setPen(digitColor);

    const int lineHeight = fontMetrics().height();
    const int width = gutter_->width() - tokens::kSp2;
    QTextBlock block = document()->findBlockByNumber(firstVisibleBlockIn(this));
    while (block.isValid()) {
        const int top = blockTopIn(this, block.blockNumber());
        if (top > event->rect().bottom()) {
            break;
        }
        if (block.isVisible() && top + lineHeight >= event->rect().top()) {
            painter.drawText(0, top, width, lineHeight, Qt::AlignRight,
                             labelFor(block.blockNumber()));
        }
        block = block.next();
    }
}

} // namespace ui_shell
