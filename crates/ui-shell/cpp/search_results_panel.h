#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>
#include <QVector>
#include <QWidget>

#include <functional>

class QCheckBox;
class QComboBox;
class QLabel;
class QLineEdit;
class QTreeWidget;
class QTreeWidgetItem;

namespace ui_shell {
class SearchPreviewPane;
class EditorTabs;
} // namespace ui_shell

// The Search Results dock: project-wide text search results grouped by file,
// plus the previewed project-wide replace built on top of them.
//
// Humble view per CLAUDE.md's hard rule — matching, ranking and rewriting all
// happen in `index-core`; this builds widgets, forwards the query, and turns
// a double-click into a caret jump. Opening a hit goes through the callback
// the main window supplies rather than a tab-widget pointer, so this file
// stays independent of the editor's internals — `editorTabs` is used only to
// resolve the "Open Files"/"Current File" scope choices to a path list,
// which `index_core::SearchScope` needs to be handed as plain paths (R8).
class SearchResultsPanel : public QWidget
{
public:
    // `openAt(path, line, column)` jumps the editor to a match.
    using OpenAt = std::function<void(const QString &, int, int)>;

    SearchResultsPanel(SearchModel *searchModel, ui_shell::EditorTabs *editorTabs, OpenAt openAt,
                        QWidget *parent);

    // Wired to the "Find in Files..." action.
    void focusQuery();

    // Run `text` as a project-wide search — how Search Everywhere hands a
    // query off when the user wants the full result set rather than the
    // popup's top hits.
    void searchFor(const QString &text);

private:
    // One checked match, as `replaceAll` collected it from the tree. Kept in
    // plain C++ types rather than an `rust::Vec<FfiFileReplacement>` because
    // the same set crosses the seam twice — once for the preview, once (a
    // subset, after the user un-ticks a file) for the real write — and a
    // `Vec` is not something this code should copy.
    struct PendingReplacement
    {
        QString path;
        quint32 line;
        quint32 start;
        quint32 end;
    };

    void runSearch();
    void appendHits(quint64 generation, const ::rust::Vec<FfiSearchHit> &hits);
    void replaceAll();
    void onReplacePreviewReady(const QStringList &paths);
    void onReplacePreviewFailed(const QString &message);
    void openMatch(QTreeWidgetItem *item, int column);
    void previewSelection();
    // The scope combo's current choice, resolved to the path list `search`
    // takes — empty means "the whole project" (R8; see `search`'s doc
    // comment in `ffi.rs` for why the bridge takes a resolved list rather
    // than an enum).
    QStringList resolveScopePaths() const;
    QTreeWidgetItem *fileGroup(const QString &path);
    static ::rust::Vec<FfiFileReplacement> toFfiEdits(const QVector<PendingReplacement> &edits);

    SearchModel *searchModel_;
    ui_shell::EditorTabs *editorTabs_;
    OpenAt openAt_;
    QLineEdit *queryEdit_ = nullptr;
    QLineEdit *maskEdit_ = nullptr;
    QComboBox *scopeCombo_ = nullptr;
    QCheckBox *regexCheck_ = nullptr;
    QCheckBox *caseCheck_ = nullptr;
    QCheckBox *wholeWordCheck_ = nullptr;
    QLineEdit *replaceEdit_ = nullptr;
    QTreeWidget *results_ = nullptr;
    ui_shell::SearchPreviewPane *preview_ = nullptr;
    QLabel *statusLabel_ = nullptr;
    QString pendingReplaceStatus_;
    // The query id the panel is currently displaying; batches from an older
    // generation are dropped rather than mixed in.
    quint64 generation_ = 0;
    int matchCount_ = 0;

    // F3-15: the matches a Replace All is previewing, and the pattern
    // question it will answer once the dialog confirms — between
    // `previewReplacements` and `replacePreviewReady`/`Failed`, this is the
    // only place that gesture's state lives.
    QVector<PendingReplacement> pendingEdits_;
    QString pendingPattern_;
    QString pendingReplacement_;
    bool pendingIsRegex_ = false;
    bool pendingCaseSensitive_ = false;
};
