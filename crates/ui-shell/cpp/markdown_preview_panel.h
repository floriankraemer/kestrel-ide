#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QHash>
#include <QString>
#include <QWidget>

#include <functional>

class QAction;
class QEvent;
class QLabel;
class QMenu;
class QTextBrowser;
class QUrl;
class AppSettings;
class PreviewProvider;

namespace ui_shell {

// The Preview dock (ADR-0033): renders the active tab's Markdown, with
// inline Mermaid diagrams, into a read-only `QTextBrowser`.
//
// Humble view (ADR-0002): every rule — what previews what, what a link
// resolves to, how a diagram becomes pixels, what the HTML even says —
// lives behind `PreviewProvider`, `markdown_preview` and `app_core::preview`.
// This class asks the active tab's content, requests a render, and paints
// whatever comes back. The only branches here are presentational: which of
// three already-decided widgets to show (empty state, browser, "no preview
// for this file type"), and whether a click landed on a link worth asking
// `previewLinkTarget` about.
//
// SECURITY (ADR-0021, same requirement `AiChatPanel` meets for assistant
// output): a Markdown file in an opened project is untrusted content.
// `setOpenLinks(false)` and `setOpenExternalLinks(false)` are both set —
// every click is instead routed through `previewLinkTarget`, which is the
// only thing allowed to decide a link opens a tab, and refuses every
// external scheme outright.
class MarkdownPreviewPanel : public QWidget
{
    Q_OBJECT

public:
    // Opening a relative link that resolved inside the project: hand the
    // path (and 1-based line, or -1) back rather than opening it directly —
    // the panel has no route to the editor, same reason `AiChatPanel::
    // ApplyHandler` exists.
    using OpenFileHandler = std::function<void(const QString &path, int line)>;
    // A link was refused, or an anchor scroll target named a heading —
    // shown in the status bar rather than a dialog, because a refused link
    // is routine, not an error.
    using StatusHandler = std::function<void(const QString &message)>;

    explicit MarkdownPreviewPanel(PreviewProvider *provider, QWidget *parent = nullptr);

    void setOpenFileHandler(OpenFileHandler handler);
    void setStatusHandler(StatusHandler handler);

    // Called by `EditorTabs`' active-tab and (debounced) content-changed
    // callbacks. `path` may be empty (no project, or a tab with no backing
    // file); an empty or unsupported path shows the empty state and
    // requests nothing.
    void setCurrentTab(quint64 tabId, const QString &path, const QString &content);

    // The editor scrolled: move the browser to the nearest anchor at or
    // before `firstVisibleLine`, guarded against the echo this triggers
    // back (see the `.cpp` for the guard).
    void syncToEditorLine(int firstVisibleLine);

    // Wired to `main_window.cpp`'s `EditorTabs`: the nearest anchor at or
    // before the browser's own current scroll position, so the editor can
    // sync the other way when the user scrolls the preview instead.
    int nearestSourceLine() const;

protected:
    // A theme switch re-renders: the highlight colours are baked into the
    // HTML the Rust side produced for the previous theme.
    void changeEvent(QEvent *event) override;

private slots:
    void onPreviewReady(quint64 tabId, quint64 revision);
    void onAnchorClicked(const QUrl &url);

private:
    void requestRender();
    void showEmptyState(const QString &message);

    PreviewProvider *provider_;
    QTextBrowser *browser_;
    QLabel *emptyState_;
    quint64 currentTabId_ = 0;
    QString currentPath_;
    QString currentContent_;
    // Set while this panel is applying its own scroll change so the
    // editor<->preview sync in `main_window.cpp` does not immediately echo
    // it back and fight the user's own scrolling.
    bool syncingScroll_ = false;
    OpenFileHandler openFileHandler_;
    StatusHandler statusHandler_;
};

class EditorTabs;

// Adds the `view.togglePreviewMode` action to `viewMenu`: the in-tab
// edit/view toggle, as opposed to `view.preview`, which opens the dock
// beside the editor. A free function here rather than inline in
// main_window.cpp for that file's line ceiling, the same split
// `wireProjectTreeViewAction` uses.
void wirePreviewModeAction(QMenu *viewMenu,
                           EditorTabs *editorTabs,
                           AppSettings *appSettings,
                           QHash<QString, QAction *> &actions);

} // namespace ui_shell
