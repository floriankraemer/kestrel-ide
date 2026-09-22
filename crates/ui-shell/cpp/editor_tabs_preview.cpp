#include "editor_tabs.h"

#include "code_editor.h"
#include "e2e_mark.h"
#include "markdown_preview_panel.h"

#include <QEvent>
#include <QShortcut>

// The in-tab half of the preview (ADR-0033's dock is the other half): a
// sixth leg of EditorTabs, alongside the pane tree, the language-server
// leg, and the VCS/run/debug ones, for the same reason each of those is
// its own translation unit.
//
// The whole design in one sentence: view mode is a MarkdownPreviewPanel
// parented to the tab's CodeEditor and shown over it, so the tab's page
// widget never stops being the editor.
//
// That is not a stylistic choice. `forEachEditor`, `editorForPath`,
// `openPaths`, `saveAllModified` and `hasUnsavedChanges` all reach a page
// through `qobject_cast<QPlainTextEdit *>(group->widget(i))` and silently
// skip anything else, so a tab whose page had been swapped for a preview
// would be invisible to Save All and to the quit-time unsaved-changes
// prompt — silent data loss, not a cosmetic bug. FindBar (find_bar.cpp)
// already floats over an editor exactly this way.
//
// For the same reason the editor is *never* made read-only to keep
// keystrokes out of the buffer while the preview is up: `saveEditor`
// returns "nothing to save" for a read-only editor, which would re-create
// that data loss by another route. Focus is the only mechanism used.

namespace ui_shell {

namespace {

// Installed on the editor, and does two things for the overlay floating
// over it.
//
// It keeps the overlay at the editor's rect — an event filter rather than a
// resizeEvent override, because the widget being resized is the editor and
// the widget being moved is not.
//
// And it swallows key events while the overlay is up. That is not belt and
// braces: focus sits in the overlay's read-only QTextBrowser, which handles
// the keys it knows (scrolling, copy) and *ignores* the rest — and an
// ignored key event propagates up the parent chain, where the editor is,
// which would type into the buffer behind a preview that is showing the
// unedited text. Swallowing here is the narrowest fix; making the editor
// read-only instead would make `saveEditor` treat it as having nothing to
// save (see this file's header). QShortcut fires on ShortcutOverride rather
// than KeyPress, so the toggle and Escape still work.
class OverlayGuard : public QObject
{
public:
    explicit OverlayGuard(QWidget *overlay)
      : QObject(overlay)
      , overlay_(overlay)
    {
    }

protected:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        if (event->type() == QEvent::Resize) {
            if (auto *widget = qobject_cast<QWidget *>(watched)) {
                overlay_->setGeometry(widget->rect());
            }
        }
        if (overlay_->isVisible()
            && (event->type() == QEvent::KeyPress || event->type() == QEvent::KeyRelease)) {
            return true;
        }
        return QObject::eventFilter(watched, event);
    }

private:
    QWidget *overlay_;
};

// The overlay for `editor`, or nullptr when this tab has never been in view
// mode. Direct children only: the panel's own QTextBrowser is a descendant,
// not a second overlay, but a future nested panel would be found here by
// accident without the flag.
MarkdownPreviewPanel *overlayFor(const QWidget *editor)
{
    return editor ? editor->findChild<MarkdownPreviewPanel *>(QString(),
                                                              Qt::FindDirectChildrenOnly)
                  : nullptr;
}

} // namespace

void EditorTabs::togglePreviewMode()
{
    auto *editor = qobject_cast<CodeEditor *>(currentEditor());
    if (!editor || !previewProvider_) {
        return;
    }
    const quint64 tabId = currentTabId();
    const QString path = docManager_->previewPath(tabId);
    // Which file types have a preview at all is a plugin's contribution,
    // resolved in `app_core::preview` — asked here, never decided here.
    if (path.isEmpty() || !previewProvider_->hasPreview(path)) {
        return;
    }

    MarkdownPreviewPanel *overlay = overlayFor(editor);
    if (!overlay) {
        overlay = new MarkdownPreviewPanel(previewProvider_, editor);
        // A parented child QWidget is transparent; without these two the
        // source text shows through the rendered document. FindBar's
        // constructor hits the identical trap and says so.
        overlay->setAutoFillBackground(true);
        overlay->setAttribute(Qt::WA_StyledBackground, true);
        overlay->setFocusPolicy(Qt::StrongFocus);
        editor->installEventFilter(new OverlayGuard(overlay));
        // Escape leaves view mode, the way it dismisses every other
        // transient surface in the editor. Scoped to the overlay so it
        // never shadows Escape while the user is typing in the buffer.
        auto *escape = new QShortcut(QKeySequence(Qt::Key_Escape), overlay);
        escape->setContext(Qt::WidgetWithChildrenShortcut);
        connect(escape, &QShortcut::activated, this, [this]() {
            if (previewModeActive(currentTabId())) {
                togglePreviewMode();
            }
        });
        overlay->setOpenFileHandler([this](const QString &target, int line) {
            if (line >= 0) {
                openFileAtLine(target, line, 0);
            } else {
                openFile(target);
            }
        });
    }

    const bool entering = !overlay->isVisible();
    if (entering) {
        overlay->setGeometry(editor->rect());
        overlay->setCurrentTab(tabId, path, editor->toPlainText());
        overlay->show();
        overlay->raise();
        overlay->setFocus();
    } else {
        overlay->hide();
        editor->setFocus();
    }

    e2eMark(QStringLiteral("{\"ev\":\"preview_mode\",\"tab_id\":%1,\"on\":%2}")
              .arg(tabId)
              .arg(entering ? QLatin1String("true") : QLatin1String("false")));

    // The window re-decides what the Preview *dock* should show: while this
    // tab renders itself, the dock stands down rather than rendering the
    // same document a second time at a different width.
    if (activeTabChanged_) {
        activeTabChanged_();
    }
}

bool EditorTabs::previewModeActive(quint64 tabId) const
{
    const MarkdownPreviewPanel *overlay = overlayFor(editorForTab(tabId));
    return overlay != nullptr && overlay->isVisible();
}

void EditorTabs::refreshPreviewMode(CodeEditor *editor)
{
    MarkdownPreviewPanel *overlay = overlayFor(editor);
    if (!overlay || !overlay->isVisible()) {
        return;
    }
    const quint64 tabId = editor->property("tabId").toULongLong();
    overlay->setCurrentTab(tabId, docManager_->previewPath(tabId), editor->toPlainText());
}

} // namespace ui_shell
