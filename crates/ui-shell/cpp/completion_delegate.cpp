#include "completion_delegate.h"

#include "symbol_icon.h"

#include <QApplication>
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
// A detail longer than this (a full Java signature, a Rust generic soup)
// elides on the left rather than widening the popup for every row.
constexpr int kDetailMaxWidth = 240;

} // namespace

void CompletionItemDelegate::paint(QPainter *painter, const QStyleOptionViewItem &option,
                                    const QModelIndex &index) const
{
    QStyleOptionViewItem opt = option;
    initStyleOption(&opt, index);
    // The label is painted by hand below (for the bold-highlight runs), so
    // it must not also be painted by the base class's own text pass.
    // `QStyledItemDelegate::paint(painter, opt, index)` cannot be used for
    // that background/selection/icon-frame pass: it calls `initStyleOption`
    // *again* on its own copy of the option, straight from
    // `index.data(Qt::DisplayRole)` — clearing `opt.text` (and even the
    // `HasDisplay` feature bit) here has no effect on that second, internal
    // copy, so the base class painted the plain label a second time, right
    // under our own per-character run, every time this delegate has ever
    // run. Invisible for a plain left-anchored prefix match (both copies
    // land on the same pixels in the same font), it became an obvious
    // smear the moment a highlighted run does not start at character 0 in
    // a plain font: D5's hyphenated Maven/Gradle coordinates score as a
    // CamelHump match (`lsp_core::completion::camel_hump`), which bolds a
    // run in the *middle* of the label (e.g. "jupiter" inside
    // "junit-jupiter") — the hand-drawn bold run then drifts wider than
    // the base class's still-plain copy of the same characters underneath
    // it, and the two no longer line up. Driving the style directly with
    // our own already-cleared `opt` — the same primitive
    // `QStyledItemDelegate::paint` itself calls once it is done deriving —
    // paints the background/selection/icon frame without that second,
    // uncontrollable text derivation.
    opt.text.clear();
    opt.features &= ~QStyleOptionViewItem::HasDisplay;
    QStyle *style = opt.widget ? opt.widget->style() : QApplication::style();
    style->drawControl(QStyle::CE_ItemViewItem, &opt, painter, opt.widget);

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

// The base class measures the display text alone, but `paint` above draws
// an icon column, per-glyph bold for the matched positions and a detail on
// the right — so a popup sized from the base hint clipped every label it
// then painted (#322). Measure exactly what `paint` lays out; the detail is
// capped so one long signature cannot push the popup off the editor.
QSize CompletionItemDelegate::sizeHint(const QStyleOptionViewItem &option,
                                        const QModelIndex &index) const
{
    QSize hint = QStyledItemDelegate::sizeHint(option, index);
    hint.setHeight(std::max(hint.height(), kIconSize + kPadding));

    const QString label = index.data(Qt::DisplayRole).toString();
    const QString detail = index.data(DetailRole).toString();
    const QVariantList positions = index.data(MatchPositionsRole).toList();
    QFont boldFont = option.font;
    boldFont.setBold(true);
    const QFontMetrics plain(option.font);
    const QFontMetrics bold(boldFont);
    int width = kPadding + kIconSize + kPadding;
    for (int i = 0; i < label.length(); ++i) {
        width += (positions.contains(i) ? bold : plain).horizontalAdvance(label.mid(i, 1));
    }
    if (!detail.isEmpty()) {
        width += kDetailGap + std::min(plain.horizontalAdvance(detail), kDetailMaxWidth);
    }
    width += kPadding;
    hint.setWidth(std::max(hint.width(), width));
    return hint;
}

} // namespace ui_shell
