#pragma once

#include <QPair>
#include <QVector>
#include <QWidget>

class QCheckBox;
class QLabel;
class QLineEdit;
class QObject;
class QEvent;
class QKeyEvent;
class QToolButton;
class QPushButton;
class DocumentManager;

namespace ui_shell {

class CodeEditor;

// In-editor Find (Ctrl+F) / Replace (Ctrl+R) bar, floated over the top-right
// of one CodeEditor rather than inserted above it — that keeps the editor
// itself the tab page widget, so the `tabId` dynamic property and every
// `qobject_cast<QPlainTextEdit *>(currentWidget())` lookup keep working
// untouched.
//
// Humble view: it decides nothing about what matches. Every span and every
// replacement string comes from `DocumentManager::findMatches` /
// `replacementEdits` (i.e. `editor_core::search`); this class only paints,
// scrolls, splices, and counts.
class FindBar : public QWidget
{
    Q_OBJECT

public:
    FindBar(CodeEditor *editor, DocumentManager *documents);

    // Ctrl+F / Ctrl+R. Both seed the query from the editor's selection when
    // there is one; `showReplace` additionally reveals the replace row.
    void showFind();
    void showReplace();

    // F3 / Shift+F3. No-ops while the bar is hidden or has no matches.
    void findNext();
    void findPrevious();

    void closeBar();

protected:
    bool eventFilter(QObject *watched, QEvent *event) override;
    void keyPressEvent(QKeyEvent *event) override;

private:
    void open(bool withReplace);
    // Re-runs the search over the editor's current text and repaints the
    // highlights. Called on every query/option/document change.
    void refresh();
    void step(int delta);
    void selectMatch(int index);
    void replaceCurrent();
    void replaceAll();
    // Both of the above: ask for the splice list and hand it to
    // `EditorTabs::applyEditsTo`, so either gesture is one Ctrl+Z. `index`
    // is the match to replace, or -1 for all of them; the ordering is
    // `editor_core::search`'s.
    void splice(int index);
    void reposition();
    void setPatternValid(bool valid, const QString &message);

    CodeEditor *editor_;
    DocumentManager *documents_;

    QLineEdit *queryEdit_ = nullptr;
    QLineEdit *replaceEdit_ = nullptr;
    QCheckBox *regexCheck_ = nullptr;
    QCheckBox *caseCheck_ = nullptr;
    QLabel *counterLabel_ = nullptr;
    QWidget *replaceRow_ = nullptr;
    QToolButton *closeButton_ = nullptr;
    QPushButton *replaceAllButton_ = nullptr;

    QVector<QPair<int, int>> matches_;
    int current_ = -1;
    // Set while this class is the one editing the document, so the
    // textChanged handler doesn't re-enter refresh() mid-replace.
    bool applying_ = false;
};

} // namespace ui_shell
