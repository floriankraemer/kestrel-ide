#pragma once

#include <QWidget>

class QLabel;
class QPlainTextEdit;

namespace ui_shell {

// R8: the read-only preview pane Find in Files and Find Usages share —
// selecting a result row shows the file around the match here instead of
// jumping the editor immediately, the same "look before you leap" shape
// every other IDE's search results give.
//
// Plain `QPlainTextEdit`, not the full `CodeEditor`: that widget's gutter,
// folding, minimap and whitespace rendering are all wired through
// `EditorTabs` (theme service, `editorOps_`), none of which a transient
// preview needs — a read-only monospace view with the match selected is
// the whole job.
class SearchPreviewPane : public QWidget
{
public:
    explicit SearchPreviewPane(QWidget *parent = nullptr);

    // Loads `path` from disk and selects the match on `line` (1-based)
    // between byte columns `start`/`end`, scrolled into view. Clears the
    // pane (rather than showing stale content) when `path` can't be read —
    // it may have changed on disk since the search ran.
    void showMatch(const QString &path, int line, int start, int end);
    void clearPreview();

private:
    QLabel *pathLabel_ = nullptr;
    QPlainTextEdit *editor_ = nullptr;
};

} // namespace ui_shell
