#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QWidget>

class QLabel;
class QPushButton;

namespace ui_shell {

// One reusable info bar above the editor area (the jvm-build-tools plan's
// B4): text plus up to two buttons, shown or hidden entirely from
// `BuildToolsService::bannerKind()` — a trust prompt ("Load"/"Not now") or
// a reload prompt ("Reload"/"Dismiss"). `cpp/` maps the bridge's
// `FfiBannerKind` onto `tr()` text and never decides *when* to show one —
// that is `jvm_build_core::sync::decide`'s rule, already applied on the
// Rust side before `bannerChanged` fires.
class EditorBanner : public QWidget
{
    Q_OBJECT

public:
    explicit EditorBanner(BuildToolsService *buildToolsService, QWidget *parent);

protected:
    // The E2E rect mark is re-emitted whenever the buttons can have moved:
    // a mark taken the turn after `setVisible(true)` describes a banner
    // that may not be mapped yet (a 20px-wide "Load" at (40, 8) when the
    // window itself is still unshown), and the flow that clicks it must see
    // the geometry that is actually on screen.
    void showEvent(QShowEvent *event) override;
    void resizeEvent(QResizeEvent *event) override;

private:
    void refresh();
    void markGeometry();

    BuildToolsService *buildToolsService_;
    QLabel *label_;
    QPushButton *primaryButton_;
    QPushButton *secondaryButton_;
};

} // namespace ui_shell
