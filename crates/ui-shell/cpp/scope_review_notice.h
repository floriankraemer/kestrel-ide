#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

class QWidget;

namespace ui_shell {

// ADR-0064's "Found N ignored but not excluded folders" notification (T6):
// a non-modal bar, floated over the bottom-right of `parent`, that appears
// when `ProjectTreeModel::scopeCandidatesFound` fires and offers a
// "Review…" dialog listing every candidate as a checkbox row (pre-checked
// per `suggestExclude`). OK writes the answer through
// `ProjectTreeModel::commitScopeReview` — checked folders excluded,
// unchecked ones reviewed-not-excluded — in one save and rescope; Cancel or
// dismissing the bar persists nothing, so the same folders are offered
// again next project open.
//
// Humble view per CLAUDE.md's hard rule: every rule this notice acts on —
// which folders qualify, which checkbox starts ticked, what OK writes —
// is `ProjectTreeModel`'s; this only paints the bar/dialog and relays the
// two clicks.
//
// `parent` is the top-level window the bar floats over (not a dock widget):
// the notification is about the *project*, not about whichever panel
// happens to be focused when it fires.
void installScopeReviewNotice(QWidget *parent, ProjectTreeModel *treeModel);

} // namespace ui_shell
