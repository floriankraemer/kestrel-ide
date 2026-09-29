#pragma once

#include <QIcon>

class QPainter;
class QRectF;

namespace ui_shell {

// Red bulb with a "!" for an offer that fixes a problem, yellow for an
// optional one (`lsp_core::bulb_kind`). Colours come from the active theme's
// SemanticColors.
enum class BulbKind
{
    Fix,
    Intention,
};

// The one place a bulb is drawn: the editor gutter's icon and the intentions
// menu's per-row icons both go through here. A lightbulb (glass and base)
// scaled into `rect`; nothing here decides when a bulb is shown.
void paintBulb(QPainter &painter, const QRectF &rect, BulbKind kind);

// A crisp icon of the bulb at `logicalSize`, for the given screen DPR.
QIcon bulbIcon(BulbKind kind, int logicalSize, qreal devicePixelRatio);

// A transparent icon of the same size, so rows without a bulb keep their
// text aligned with rows that have one.
QIcon blankBulbIcon(int logicalSize, qreal devicePixelRatio);

} // namespace ui_shell
