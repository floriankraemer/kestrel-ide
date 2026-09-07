#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QComboBox;
class QLabel;
class QToolButton;

namespace ui_shell {

// The row of controls above a diff, in JetBrains' order: previous / next
// change, the difference count, the viewer (side-by-side / unified), the
// whitespace mode, the highlighting mode, collapse-unchanged and
// synchronize-scrolling toggles.
//
// Pure chrome: it reports what the user picked and nothing more. Every
// option's *meaning* lives in `editor_core::diff` (the two modes cross the
// seam as the `Ffi*` enums this widget hands straight back) or in the view
// it is applied to.
class DiffToolbar : public QWidget
{
    Q_OBJECT

public:
    enum class Viewer
    {
        SideBySide,
        Unified,
    };

    explicit DiffToolbar(QWidget *parent = nullptr);

    Viewer viewer() const;
    FfiWhitespaceMode whitespaceMode() const;
    FfiHighlightMode highlightMode() const;
    bool collapseUnchanged() const;
    bool syncScroll() const;

    void setDifferenceCount(int count);

signals:
    void previousRequested();
    void nextRequested();
    // The viewer combo, or any option that changes what is computed or how
    // it is laid out.
    void viewerChanged();
    void optionsChanged();

protected:
    void showEvent(QShowEvent *event) override;

private:
    QToolButton *previous_ = nullptr;
    QToolButton *next_ = nullptr;
    QLabel *count_ = nullptr;
    QComboBox *viewer_ = nullptr;
    QComboBox *whitespace_ = nullptr;
    QComboBox *highlight_ = nullptr;
    QToolButton *collapse_ = nullptr;
    QToolButton *sync_ = nullptr;
};

} // namespace ui_shell
