#pragma once

#include <QStyledItemDelegate>

namespace ui_shell {

// R2: paints one completion row — a kind icon, the label with the typed
// prefix bolded, a right-aligned detail column, and a struck-through label
// for a deprecated item. Every one of those is data already on the row
// (`CodeEditor::showCompletions` sets the roles below from the
// `CompletionEntry` the bridge handed over); this class only lays them out
// and draws them; it decides no kind, no detail and no highlight of its
// own — a humble view over `lsp_core::completion`'s own answer, same as
// `CodeEditor` itself.
class CompletionItemDelegate : public QStyledItemDelegate
{
public:
    // One role per field `showCompletions` sets, past `Qt::UserRole` and
    // past `CodeEditor`'s own `kEntryIndexRole` (`Qt::UserRole + 1`).
    static constexpr int KindRole = Qt::UserRole + 2;
    static constexpr int DetailRole = Qt::UserRole + 3;
    static constexpr int DeprecatedRole = Qt::UserRole + 4;
    // A `QVariantList` of `int` — char indices into the label the typed
    // prefix matched (`lsp_core::completion::CompletionMatch::positions`).
    static constexpr int MatchPositionsRole = Qt::UserRole + 5;

    using QStyledItemDelegate::QStyledItemDelegate;

    void paint(QPainter *painter, const QStyleOptionViewItem &option,
               const QModelIndex &index) const override;
    QSize sizeHint(const QStyleOptionViewItem &option, const QModelIndex &index) const override;
};

} // namespace ui_shell
