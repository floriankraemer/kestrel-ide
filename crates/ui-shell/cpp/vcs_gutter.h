#pragma once

#include <QColor>
#include <QString>
#include <functional>

class QPoint;
class QWidget;

namespace ui_shell {

// One change marker's kind, view-local for the same reason FoldRange
// (code_editor.h) is: converted from `FfiHunkKind` by whoever owns the
// mapping (EditorTabs), so CodeEditor stays decoupled from the cxx-qt
// generated header.
enum class ChangeMarkerKind
{
    Added,
    Removed,
    Modified
};

// One line's change marker in the gutter's vertical strip (F3-16).
// `hunkIndex` is the position of the hunk this line belongs to in whatever
// `VcsService::hunks` last returned for the file — what a click needs to ask
// for a revert, a diff or a stage, without CodeEditor knowing anything about
// hunks beyond "paint this colour here, and tell me the index if clicked".
// How much of a marker's hunk is already in the index (R6, IDEA's three
// states) — `FfiHunkStageState`, converted by EditorTabs for the same
// decoupling reason ChangeMarkerKind is.
enum class ChangeMarkerState
{
    Unstaged,
    Staged,
    Both
};

struct ChangeMarker
{
    int block;
    ChangeMarkerKind kind;
    int hunkIndex;
    ChangeMarkerState state = ChangeMarkerState::Unstaged;

    bool operator==(const ChangeMarker &other) const
    {
        return block == other.block && kind == other.kind && hunkIndex == other.hunkIndex
          && state == other.state;
    }
};

// The strip's colour for one marker kind and stage state. Kept here rather
// than inline in CodeEditor's paint loop so the same table is used by any
// future consumer (a minimap, a changes-panel row) without a second table.
// The three states are shades of the theme's own marker colour, never a
// fourth colour: unstaged is the marker colour itself, staged is lighter
// (already "done", quieter), both is darker (needs a look — part of what
// the gutter shows is not in the index).
QColor changeMarkerColor(ChangeMarkerKind kind,
                         ChangeMarkerState state = ChangeMarkerState::Unstaged);

// The hunk popup (F3-16/R6): Revert / Show Diff / Stage Hunk / Unstage Hunk
// / Stage File, shown synchronously at `globalPos`. Each callback runs when
// its entry is chosen; a null callback omits that entry rather than showing
// it disabled — the caller only builds the ones it can perform for `path`.
//
// `stageHunk`/`unstageHunk` both being offered regardless of whether the
// hunk is already (partially) staged is a deliberate simplification:
// `VcsService::stageHunk`/`unstageHunk` are no-ops when there is nothing on
// their side left to change (`vcs_core::Repository::stage_hunk_matching`/
// `unstage_hunk_matching` return `Ok(false)`), so offering both costs
// nothing beyond a menu entry that does nothing on a given click — cheaper
// than a bridge round trip to classify the hunk before deciding which of
// the two to grey out.
struct HunkPopupActions
{
    // The `HEAD`-side lines this hunk removed, `\n`-joined, shown inline at
    // the top of the popup (R6); empty for a pure addition. Read from
    // `VcsService::hunkRemovedText`, never recomputed here.
    QString removedText;
    // Shown as the popup's header so the user knows which of the two stage
    // entries below will actually do something.
    ChangeMarkerState state = ChangeMarkerState::Unstaged;
    std::function<void()> revert;
    std::function<void()> showDiff;
    std::function<void()> stageHunk;
    std::function<void()> unstageHunk;
    std::function<void()> stageFile;
};

void showHunkPopup(QWidget *parent, const QPoint &globalPos, const HunkPopupActions &actions);

} // namespace ui_shell
