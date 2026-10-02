#pragma once

// Item delegate that bolds the characters a search matched.
//
// Header-only and deliberately tiny: it renders a QTextDocument built from
// the item's display text plus a list of character positions carried on a
// custom role. Which characters matched is decided in Rust (the fuzzy
// matcher reports them); this only paints them, so it stays a view detail
// per CLAUDE.md's humble-view rule.

#include <QAbstractTextDocumentLayout>
#include <QApplication>
#include <QPainter>
#include <QStyleOptionViewItem>
#include <QStyledItemDelegate>
#include <QTextDocument>
#include <QVariantList>

// Character offsets (into the item's display text) to highlight.
inline constexpr int kMatchPositionsRole = Qt::UserRole + 10;
// Secondary text painted greyed after the display text (a symbol's kind and
// file, an action's shortcut). Optional: a row without it paints as before.
inline constexpr int kDetailRole = Qt::UserRole + 11;

class HighlightDelegate : public QStyledItemDelegate
{
public:
    using QStyledItemDelegate::QStyledItemDelegate;

    void paint(QPainter *painter,
               const QStyleOptionViewItem &option,
               const QModelIndex &index) const override
    {
        const QVariantList positions = index.data(kMatchPositionsRole).toList();
        const QString detail = index.data(kDetailRole).toString();
        if (positions.isEmpty() && detail.isEmpty()) {
            QStyledItemDelegate::paint(painter, option, index);
            return;
        }

        QStyleOptionViewItem styled = option;
        initStyleOption(&styled, index);
        const QString text = styled.text;
        styled.text.clear();

        QStyle *style = styled.widget ? styled.widget->style() : QApplication::style();
        style->drawControl(QStyle::CE_ItemViewItem, &styled, painter, styled.widget);

        QTextDocument document;
        document.setDefaultFont(styled.font);
        document.setDocumentMargin(0);

        const QRect textRect =
          style->subElementRect(QStyle::SE_ItemViewItemText, &styled, styled.widget);
        QString html = highlighted(text, positions);
        if (!detail.isEmpty()) {
            // Elide the detail to the room the name leaves, so a long path
            // ends in an ellipsis instead of being cut off at the edge.
            const QFontMetrics metrics(styled.font);
            const int gap = metrics.horizontalAdvance(QStringLiteral("    "));
            const int room = textRect.width() - metrics.horizontalAdvance(text) - gap;
            const QColor grey = styled.state & QStyle::State_Selected
                                  ? styled.palette.color(QPalette::HighlightedText)
                                  : styled.palette.color(QPalette::Disabled, QPalette::Text);
            html += QStringLiteral("&nbsp;&nbsp;&nbsp;&nbsp;<span style=\"color:%1\">%2</span>")
                      .arg(grey.name(),
                           metrics.elidedText(detail, Qt::ElideMiddle, qMax(room, 0))
                             .toHtmlEscaped());
        }
        document.setHtml(html);
        painter->save();
        // Centred on the row, like the plain-text path and the icon beside it.
        painter->translate(textRect.left(),
                           textRect.top() + (textRect.height() - document.size().height()) / 2);
        QAbstractTextDocumentLayout::PaintContext context;
        context.palette = styled.palette;
        context.palette.setColor(
          QPalette::Text,
          styled.state & QStyle::State_Selected
            ? styled.palette.color(QPalette::HighlightedText)
            : styled.palette.color(QPalette::Text));
        context.clip = QRectF(0, 0, textRect.width(), textRect.height());
        document.documentLayout()->draw(painter, context);
        painter->restore();
    }

private:
    // Wrap every highlighted character in <b>. Adjacent positions collapse
    // into one run so a contiguous match isn't split into per-character tags.
    static QString highlighted(const QString &text, const QVariantList &positions)
    {
        QList<int> offsets;
        offsets.reserve(positions.size());
        for (const QVariant &position : positions) {
            const int offset = position.toInt();
            if (offset >= 0 && offset < text.size()) {
                offsets.append(offset);
            }
        }
        std::sort(offsets.begin(), offsets.end());
        offsets.erase(std::unique(offsets.begin(), offsets.end()), offsets.end());

        QString html;
        int next = 0;
        for (int i = 0; i < offsets.size();) {
            int runEnd = i;
            while (runEnd + 1 < offsets.size() && offsets[runEnd + 1] == offsets[runEnd] + 1) {
                ++runEnd;
            }
            const int start = offsets[i];
            const int length = offsets[runEnd] - start + 1;
            html += text.mid(next, start - next).toHtmlEscaped();
            html += QStringLiteral("<b>") + text.mid(start, length).toHtmlEscaped()
                    + QStringLiteral("</b>");
            next = start + length;
            i = runEnd + 1;
        }
        html += text.mid(next).toHtmlEscaped();
        return html;
    }
};
