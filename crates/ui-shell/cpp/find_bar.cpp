#include "find_bar.h"

#include "code_editor.h"
#include "e2e_mark.h"
#include "editor_tabs.h"
#include "ui_tokens.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QEvent>
#include <QFrame>
#include <QHBoxLayout>
#include <QKeyEvent>
#include <QLabel>
#include <QLineEdit>
#include <QPushButton>
#include <QResizeEvent>
#include <QScrollBar>
#include <QTextCursor>
#include <QToolButton>
#include <QVBoxLayout>

namespace ui_shell {

namespace {
// Gap between the bar and the editor's top-right corner.
constexpr int kMargin = tokens::kPanelGap;
// Below this the fields stop being usable; the bar keeps this width and lets
// the editor clip it rather than collapsing to nothing.
constexpr int kMinWidth = 340;
} // namespace

FindBar::FindBar(CodeEditor *editor, DocumentManager *documents)
  : QWidget(editor)
  , editor_(editor)
  , documents_(documents)
{
    // Floats over the text, so it has to paint its own opaque background —
    // an unstyled child QWidget is transparent and the code would show
    // through it. Palette roles keep it following the active theme.
    setObjectName(QStringLiteral("findBar"));
    setAutoFillBackground(true);
    setAttribute(Qt::WA_StyledBackground, true);
    setStyleSheet(QStringLiteral("#findBar { background-color: palette(window);"
                                 " border: 1px solid palette(mid); border-radius: %1px; }")
                    .arg(tokens::kRadiusPanel));

    queryEdit_ = new QLineEdit(this);
    queryEdit_->setPlaceholderText(tr("Find"));
    queryEdit_->setClearButtonEnabled(true);
    // Keeps the query readable when the bar is squeezed by a narrow split;
    // the buttons give up their space first.
    queryEdit_->setMinimumWidth(140);
    regexCheck_ = new QCheckBox(QStringLiteral(".*"), this);
    regexCheck_->setToolTip(tr("Regular expression"));
    caseCheck_ = new QCheckBox(QStringLiteral("Aa"), this);
    caseCheck_->setToolTip(tr("Match case"));
    counterLabel_ = new QLabel(this);
    auto *prevButton = new QToolButton(this);
    prevButton->setText(QStringLiteral("‹"));
    prevButton->setToolTip(tr("Previous match"));
    auto *nextButton = new QToolButton(this);
    nextButton->setText(QStringLiteral("›"));
    nextButton->setToolTip(tr("Next match"));
    closeButton_ = new QToolButton(this);
    closeButton_->setText(QStringLiteral("✕"));
    closeButton_->setToolTip(tr("Close"));

    auto *findRow = new QHBoxLayout();
    findRow->addWidget(queryEdit_, 1);
    findRow->addWidget(regexCheck_);
    findRow->addWidget(caseCheck_);
    findRow->addWidget(counterLabel_);
    findRow->addWidget(prevButton);
    findRow->addWidget(nextButton);
    findRow->addWidget(closeButton_);

    replaceEdit_ = new QLineEdit(this);
    replaceEdit_->setPlaceholderText(tr("Replace"));
    auto *replaceButton = new QPushButton(tr("Replace"), this);
    replaceAllButton_ = new QPushButton(tr("Replace All"), this);
    replaceRow_ = new QWidget(this);
    auto *replaceLayout = new QHBoxLayout(replaceRow_);
    replaceLayout->setContentsMargins(0, 0, 0, 0);
    replaceLayout->addWidget(replaceEdit_, 1);
    replaceLayout->addWidget(replaceButton);
    replaceLayout->addWidget(replaceAllButton_);
    replaceRow_->hide();

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(tokens::kSp2, tokens::kSp1, tokens::kSp2, tokens::kSp1);
    layout->addLayout(findRow);
    layout->addWidget(replaceRow_);

    connect(queryEdit_, &QLineEdit::textChanged, this, &FindBar::refresh);
    connect(regexCheck_, &QCheckBox::toggled, this, &FindBar::refresh);
    connect(caseCheck_, &QCheckBox::toggled, this, &FindBar::refresh);
    connect(prevButton, &QToolButton::clicked, this, &FindBar::findPrevious);
    connect(nextButton, &QToolButton::clicked, this, &FindBar::findNext);
    connect(closeButton_, &QToolButton::clicked, this, &FindBar::closeBar);
    connect(replaceButton, &QPushButton::clicked, this, &FindBar::replaceCurrent);
    connect(replaceAllButton_, &QPushButton::clicked, this, &FindBar::replaceAll);
    connect(queryEdit_, &QLineEdit::returnPressed, this, &FindBar::findNext);
    connect(replaceEdit_, &QLineEdit::returnPressed, this, &FindBar::replaceCurrent);

    // The invokables return an empty span list on a pattern that doesn't
    // compile and report the reason through this signal (ADR-0003: a typed
    // channel, never a sentinel value).
    connect(documents_, &DocumentManager::findPatternInvalid, this, [this](const QString &message) {
        if (isVisible()) {
            setPatternValid(false, message);
        }
    });
    // Typing in the editor (including an undo of a replace) invalidates the
    // spans, so re-run rather than paint stale highlights.
    connect(editor_->document(), &QTextDocument::contentsChanged, this, [this] {
        if (isVisible() && !applying_) {
            refresh();
        }
    });

    editor_->installEventFilter(this);
    hide();
}

void FindBar::showFind()
{
    open(false);
}

void FindBar::showReplace()
{
    open(true);
}

void FindBar::open(bool withReplace)
{
    replaceRow_->setVisible(withReplace);
    const QString selected = editor_->textCursor().selectedText();
    // A multi-line selection is a range the user marked, not a search term.
    if (!selected.isEmpty() && !selected.contains(QChar::ParagraphSeparator)) {
        queryEdit_->setText(selected);
    }
    show();
    reposition();
    refresh();
    queryEdit_->setFocus();
    queryEdit_->selectAll();
    // Same reasoning as `changes_panel_shown`: the bar floats wherever the
    // editor's viewport puts it, so an E2E flow reads the replace field's
    // and Replace All's on-screen rects here instead of guessing them.
    const QRect replaceRect(replaceEdit_->mapToGlobal(QPoint(0, 0)), replaceEdit_->size());
    const QRect allRect(replaceAllButton_->mapToGlobal(QPoint(0, 0)), replaceAllButton_->size());
    e2eMark(QStringLiteral("{\"ev\":\"find_bar_shown\",\"replace\":%1,"
                            "\"replace_rect\":[%2,%3,%4,%5],"
                            "\"replace_all_rect\":[%6,%7,%8,%9]}")
              .arg(withReplace ? QStringLiteral("true") : QStringLiteral("false"))
              .arg(replaceRect.x())
              .arg(replaceRect.y())
              .arg(replaceRect.width())
              .arg(replaceRect.height())
              .arg(allRect.x())
              .arg(allRect.y())
              .arg(allRect.width())
              .arg(allRect.height()));
}

void FindBar::closeBar()
{
    hide();
    editor_->setMatchSelections({}, -1);
    editor_->setFocus();
}

void FindBar::refresh()
{
    setPatternValid(true, QString());
    const rust::Vec<FfiTextMatch> found = documents_->findMatches(editor_->toPlainText(),
                                                                 queryEdit_->text(),
                                                                 regexCheck_->isChecked(),
                                                                 caseCheck_->isChecked());
    matches_.clear();
    matches_.reserve(static_cast<int>(found.size()));
    for (const FfiTextMatch &match : found) {
        matches_.append({static_cast<int>(match.start), static_cast<int>(match.end)});
    }

    // Keep the caret's place in the result set: the first match at or after
    // it, so editing then re-searching doesn't jump back to the top.
    current_ = -1;
    const int caret = editor_->textCursor().selectionStart();
    for (int i = 0; i < matches_.size(); ++i) {
        if (matches_[i].first >= caret) {
            current_ = i;
            break;
        }
    }
    if (current_ == -1 && !matches_.isEmpty()) {
        current_ = 0;
    }

    counterLabel_->setText(matches_.isEmpty()
                             ? tr("No results")
                             : tr("%1/%2").arg(current_ + 1).arg(matches_.size()));
    editor_->setMatchSelections(matches_, current_);
}

void FindBar::findNext()
{
    step(1);
}

void FindBar::findPrevious()
{
    step(-1);
}

void FindBar::step(int delta)
{
    if (!isVisible() || matches_.isEmpty()) {
        return;
    }
    const int size = matches_.size();
    current_ = ((current_ + delta) % size + size) % size;
    selectMatch(current_);
}

void FindBar::selectMatch(int index)
{
    if (index < 0 || index >= matches_.size()) {
        return;
    }
    QTextCursor cursor = editor_->textCursor();
    cursor.setPosition(matches_[index].first);
    cursor.setPosition(matches_[index].second, QTextCursor::KeepAnchor);
    // A match inside a collapsed fold would otherwise land on a hidden line.
    editor_->ensureBlockVisible(cursor.blockNumber());
    editor_->setTextCursor(cursor);
    editor_->centerCursor();
    counterLabel_->setText(tr("%1/%2").arg(index + 1).arg(matches_.size()));
    editor_->setMatchSelections(matches_, index);
}

void FindBar::splice(int index)
{
    const rust::Vec<FfiTextEdit> edits = documents_->replacementEdits(editor_->toPlainText(),
                                                                     queryEdit_->text(),
                                                                     replaceEdit_->text(),
                                                                     regexCheck_->isChecked(),
                                                                     caseCheck_->isChecked(),
                                                                     index);
    if (edits.empty()) {
        return;
    }
    applying_ = true;
    EditorTabs::applyEditsTo(editor_, edits);
    applying_ = false;
    refresh();
}

void FindBar::replaceCurrent()
{
    if (matches_.isEmpty() || current_ < 0) {
        return;
    }
    splice(current_);
}

void FindBar::replaceAll()
{
    splice(-1);
}

void FindBar::setPatternValid(bool valid, const QString &message)
{
    // Qt has no "invalid" widget state, so this is the conventional
    // stylesheet tint; the reason itself lives in the tooltip.
    queryEdit_->setStyleSheet(valid ? QString()
                                    : QStringLiteral("QLineEdit { background: #78302f; }"));
    queryEdit_->setToolTip(message);
    if (!valid) {
        counterLabel_->setText(tr("Invalid pattern"));
    }
}

void FindBar::reposition()
{
    adjustSize();
    // Never wider than the editor it floats over: in a narrow split the bar
    // would otherwise hang off both edges instead of shrinking.
    const int available = editor_->viewport()->width() - 2 * kMargin;
    resize(qMin(sizeHint().width(), qMax(kMinWidth, available)), sizeHint().height());
    const int right = editor_->viewport()->x() + editor_->viewport()->width();
    move(qMax(kMargin, right - width() - kMargin), kMargin);
}

bool FindBar::eventFilter(QObject *watched, QEvent *event)
{
    if (watched == editor_ && event->type() == QEvent::Resize && isVisible()) {
        reposition();
    }
    return QWidget::eventFilter(watched, event);
}

void FindBar::keyPressEvent(QKeyEvent *event)
{
    if (event->key() == Qt::Key_Escape) {
        closeBar();
        return;
    }
    if (event->key() == Qt::Key_Return || event->key() == Qt::Key_Enter) {
        if (event->modifiers().testFlag(Qt::ShiftModifier)) {
            findPrevious();
            return;
        }
    }
    QWidget::keyPressEvent(event);
}

} // namespace ui_shell
