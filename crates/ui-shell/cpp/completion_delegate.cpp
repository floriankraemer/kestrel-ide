#include "completion_delegate.h"

#include "symbol_icon.h"

#include <QFont>
#include <QFontMetrics>
#include <QIcon>
#include <QPainter>
#include <QPalette>
#include <QStyle>
#include <QStyleOptionViewItem>
#include <QVariantList>

#include <algorithm>

namespace ui_shell {

namespace {

// Reuses the Structure panel's own kind glyphs (`symbol_icon.h`) rather than
// a second icon set — completion's kind names (`lsp_core::completion::
// kind_name`) and the Structure panel's `FfiSymbolKind` overlap on exactly
// the kinds worth telling apart at a glance; the rest (keyword, snippet,
// text, module, file, folder, ...) paint with no icon, same as an
// unresolved symbol kind already does.
QIcon iconForKind(const QString &kind)
{
    if (kind == QLatin1String("method")) {
        return symbolKindIcon(FfiSymbolKind::Method);
    }
    if (kind == QLatin1String("function")) {
        return symbolKindIcon(FfiSymbolKind::Function);
    }
    if (kind == QLatin1String("constructor")) {
        return symbolKindIcon(FfiSymbolKind::Constructor);
    }
    if (kind == QLatin1String("field")) {
        return symbolKindIcon(FfiSymbolKind::Field);
    }
    if (kind == QLatin1String("class")) {
        return symbolKindIcon(FfiSymbolKind::Class);
    }
    if (kind == QLatin1String("interface")) {
        return symbolKindIcon(FfiSymbolKind::Interface);
    }
    if (kind == QLatin1String("struct")) {
        return symbolKindIcon(FfiSymbolKind::Struct);
    }
    if (kind == QLatin1String("enum")) {
        return symbolKindIcon(FfiSymbolKind::Enum);
    }
    if (kind == QLatin1String("enum member")) {
        return symbolKindIcon(FfiSymbolKind::EnumMember);
    }
    if (kind == QLatin1String("property")) {
        return symbolKindIcon(FfiSymbolKind::Property);
    }
    if (kind == QLatin1String("constant")) {
        return symbolKindIcon(FfiSymbolKind::Constant);
    }
    return {};
}

constexpr int kIconSize = 16;
constexpr int kPadding = 4;
constexpr int kDetailGap = 12;

} // namespace

void CompletionItemDelegate::paint(QPainter *painter, const QStyleOptionViewItem &option,
                                    const QModelIndex &index) const
{
    QStyleOptionViewItem opt = option;
    initStyleOption(&opt, index);
    // The label is painted by hand below (for the bold-highlight runs), so
    // strip it from the option before the base class paints the
    // background/selection/icon frame.
    opt.text.clear();
    QStyledItemDelegate::paint(painter, opt, index);

    painter->save();
    const QRect rect = option.rect;
    int x = rect.left() + kPadding;

    const QIcon icon = iconForKind(index.data(KindRole).toString());
    if (!icon.isNull()) {
        const QRect iconRect(x, rect.top() + (rect.height() - kIconSize) / 2, kIconSize, kIconSize);
        icon.paint(painter, iconRect);
    }
    x += kIconSize + kPadding;

    const QString label = index.data(Qt::DisplayRole).toString();
    const QString detail = index.data(DetailRole).toString();
    const bool deprecated = index.data(DeprecatedRole).toBool();
    QVariantList positions = index.data(MatchPositionsRole).toList();

    const bool isSelected = option.state.testFlag(QStyle::State_Selected);
    QColor textColor = isSelected ? option.palette.color(QPalette::HighlightedText)
                                   : option.palette.color(QPalette::Text);
    painter->setPen(textColor);

    QFont plainFont = option.font;
    plainFont.setStrikeOut(deprecated);
    QFont boldFont = plainFont;
    boldFont.setBold(true);

    const QRect textRect(x, rect.top(), rect.width() - (x - rect.left()) - kPadding, rect.height());
    int textX = textRect.left();
    const int textY = textRect.top();
    const int textHeight = textRect.height();
    for (int i = 0; i < label.length(); ++i) {
        const bool highlighted = positions.contains(i);
        painter->setFont(highlighted ? boldFont : plainFont);
        const QFontMetrics metrics(highlighted ? boldFont : plainFont);
        const QString ch = label.mid(i, 1);
        painter->drawText(QRect(textX, textY, metrics.horizontalAdvance(ch) + 1, textHeight),
                           Qt::AlignVCenter | Qt::AlignLeft, ch);
        textX += metrics.horizontalAdvance(ch);
    }

    if (!detail.isEmpty()) {
        painter->setFont(option.font);
        QColor detailColor = textColor;
        detailColor.setAlpha(160);
        painter->setPen(detailColor);
        const QFontMetrics metrics(option.font);
        const QString elided =
          metrics.elidedText(detail, Qt::ElideLeft,
                              textRect.right() - textX - kDetailGap);
        painter->drawText(QRect(textX + kDetailGap, textY, textRect.right() - textX - kDetailGap,
                                 textHeight),
                           Qt::AlignVCenter | Qt::AlignRight, elided);
    }

    painter->restore();
}

QSize CompletionItemDelegate::sizeHint(const QStyleOptionViewItem &option,
                                        const QModelIndex &index) const
{
    QSize hint = QStyledItemDelegate::sizeHint(option, index);
    hint.setHeight(std::max(hint.height(), kIconSize + kPadding));
    return hint;
}

} // namespace ui_shell
