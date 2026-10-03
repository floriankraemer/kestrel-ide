#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// Settings > Analysis (the PHP tooling plan's B7/B9): one row per
// contributed analyzer — enabled, trigger, and its live detection status.
//
// Humble view (ADR-0002): which analyzers exist, what is worth persisting,
// and whether a row differs from what is saved are all `AnalysisEditor`
// calls into `settings-model`. The Status column is a one-time snapshot
// from `AnalysisService::analyzerRows` taken when the page is built — it
// is detection state, not a setting, and nothing here re-detects or starts
// anything.
// The sentence for an analyzer whose status is `NeedsConfig` ("Psalm needs
// a psalm.xml — run `vendor/bin/psalm --init`"); the row's own
// `statusText` for every other status. Shared with the status bar.
QString analyzerStatusText(const FfiAnalyzerRow &row);

QWidget *buildAnalysisSettingsPage(QWidget *parent, AnalysisEditor *editor,
                                   AnalysisService *analysisService);

} // namespace ui_shell
