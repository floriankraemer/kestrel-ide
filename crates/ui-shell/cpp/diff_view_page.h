#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QWidget>

#include <functional>

class QLabel;
class QStackedWidget;

namespace ui_shell {

class DiffToolbar;
class DiffView;

// Everything one diff computation produced for a pair of texts. Crosses no
// seam itself — it is what the owner's `DiffRecompute` collects from the
// three bridge calls (`diffHunksBetween`, `diffSpansBetween`,
// `diffRowsBetween`), which are three because a `Vec` field on a shared
// cxx struct is not a shape it supports.
struct DiffData
{
    QString leftText;
    QString rightText;
    ::rust::Vec<FfiHunk> hunks;
    ::rust::Vec<FfiInlineSpan> spans;
    ::rust::Vec<FfiDiffRow> rows;
};

// Recompute the diff under the toolbar's current modes. Supplied by the
// owner, who alone knows where the two texts come from: a diff tab reads
// them back from its `DocumentManager` tab, the editable window reads the
// live editor.
using DiffRecompute = std::function<DiffData(FfiWhitespaceMode, FfiHighlightMode)>;

// The chrome around a `DiffView`: the toolbar, a header naming each side
// above its pane, and the view itself. Owns the `DiffView` it is
// constructed with.
//
// `refresh()` is the single path every option change and every live edit
// goes through — recompute, push into the view, update the count — so the
// whitespace combo, the highlighting combo and a keystroke in the editable
// pane can never leave the view showing different generations of the same
// diff.
class DiffViewPage : public QWidget
{
    Q_OBJECT

public:
    DiffViewPage(DiffView *diffView,
                 const QString &leftLabel,
                 const QString &rightLabel,
                 DiffRecompute recompute,
                 QWidget *parent = nullptr);

    DiffView *diffView() const { return diffView_; }

    void refresh();

    // Makes the divider's chevrons live: called with the hunk to replace on
    // the right side with the left side's lines. Only the editable
    // HEAD-vs-working-tree window sets one.
    void setApplyHandler(std::function<void(const FfiHunk &)> handler);

private:
    DiffToolbar *toolbar_;
    QLabel *leftHeader_;
    QLabel *rightHeader_;
    DiffView *diffView_;
    DiffRecompute recompute_;
    DiffData data_;
    std::function<void(const FfiHunk &)> applyHandler_;
    bool refreshing_ = false;
};

} // namespace ui_shell
