#include "ai_chat_panel.h"

#include "flow_layout.h"
#include "theme.h"
#include "ui_tokens.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAbstractItemView>
#include <QAbstractTextDocumentLayout>
#include <QAction>
#include <QBrush>
#include <QButtonGroup>
#include <QClipboard>
#include <QComboBox>
#include <QCompleter>
#include <QFont>
#include <QFrame>
#include <QGuiApplication>
#include <QHBoxLayout>
#include <QInputDialog>
#include <QKeyEvent>
#include <QLabel>
#include <QLayoutItem>
#include <QLineEdit>
#include <QListWidget>
#include <QListWidgetItem>
#include <QLocale>
#include <QMenu>
#include <QMessageBox>
#include <QModelIndex>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QRect>
#include <QScrollArea>
#include <QScrollBar>
#include <QSignalBlocker>
#include <QSize>
#include <QSplitter>
#include <QStringList>
#include <QStringListModel>
#include <QTextBrowser>
#include <QTextCursor>
#include <QTextDocument>
#include <QTimer>
#include <QToolButton>
#include <QVBoxLayout>
#include <QWidget>

#include <cstddef>
#include <functional>
#include <utility>

namespace ui_shell {

namespace {

// Item data role carrying a conversation id on a history row.
constexpr int kConversationIdRole = Qt::UserRole;

// How long typing has to settle before an @-mention query goes out. Same
// value SearchEverywhereDialog uses, for the same reason: one query per
// pause rather than one per keystroke.
constexpr int kMentionDebounceMs = 60;

// File suggestions offered for one @-mention. A completion popup is a
// top-hits list; the Search Results dock is where an exhaustive list lives.
constexpr quint32 kMentionLimit = 20;

// Slack added to a bubble's measured document height so the last line is
// never clipped by rounding, and to the popup width so the last glyph is not.
constexpr int kHeightPadding = 6;
constexpr int kPopupWidthPadding = 8;

// Delete every widget a rebuilt layout used to hold. deleteLater rather than
// delete because a rebuild can be triggered from inside a chip's own clicked
// handler, and destroying the sender mid-signal is a crash.
void clearLayout(QLayout *layout)
{
    while (QLayoutItem *item = layout->takeAt(0)) {
        if (QWidget *widget = item->widget()) {
            widget->hide();
            widget->deleteLater();
        }
        delete item;
    }
}

// The composer. A QPlainTextEdit subclass rather than an event filter
// because QCompleter re-delivers key events by calling the widget's
// `event()` directly, which no installed filter ever sees — CodeEditor
// overrides keyPressEvent for the same reason.
class ComposerEdit : public QPlainTextEdit
{
public:
    using QPlainTextEdit::QPlainTextEdit;

    std::function<bool(QKeyEvent *)> keyHook;

protected:
    void keyPressEvent(QKeyEvent *event) override
    {
        if (keyHook && keyHook(event)) {
            event->accept();
            return;
        }
        QPlainTextEdit::keyPressEvent(event);
    }
};

// The composer's live counter. `exact` is the whole reason FfiTokenUsage
// carries a flag rather than only a number (ADR-0021: "labels it an estimate
// rather than presenting a guess as a measurement"), so an estimate is
// rendered with a leading tilde and says so in its tooltip.
QString tokenText(const FfiTokenUsage &usage)
{
    const QLocale locale;
    const QString count = locale.toString(static_cast<qulonglong>(usage.context_tokens));
    const QString shown = usage.exact ? count : QStringLiteral("~%1").arg(count);
    if (usage.budget > 0) {
        return QObject::tr("%1 / %2 tokens")
          .arg(shown, locale.toString(static_cast<qulonglong>(usage.budget)));
    }
    return QObject::tr("%1 tokens").arg(shown);
}

// A one-line row that has to read as subordinate to a real message: an
// agent run is a transcript, not a log dump.
QLabel *subordinateLabel(const QString &text, const QColor &color)
{
    auto *label = new QLabel(text);
    label->setWordWrap(true);
    label->setTextInteractionFlags(Qt::TextSelectableByMouse);
    QFont font = label->font();
    font.setPointSizeF(font.pointSizeF() * 0.9);
    label->setFont(font);
    label->setStyleSheet(QStringLiteral("color: %1;").arg(color.name()));
    return label;
}

/// A combo box that asks for its contents the first time — and every time —
/// it is opened.
///
/// The model catalogue is fetched over the network, and nothing should
/// contact a provider because a panel was built or a combo repainted. Qt
/// offers no "about to show popup" signal, so opening it is the only hook,
/// and `AiChat::refreshModels` is the one that decides whether a request is
/// actually needed.
class LazyComboBox : public QComboBox
{
public:
    using QComboBox::QComboBox;

    void setPopulator(std::function<void()> populate) { populate_ = std::move(populate); }

    void showPopup() override
    {
        if (populate_) {
            populate_();
        }
        QComboBox::showPopup();
    }

private:
    std::function<void()> populate_;
};

} // namespace

AiChatPanel::AiChatPanel(AiChat *chat, SearchModel *searchModel, QWidget *parent)
  : QWidget(parent)
  , chat_(chat)
  , searchModel_(searchModel)
{
    // ---- Header: provider, mode, new chat, history -----------------------
    providerCombo_ = new QComboBox(this);
    // Not AdjustToContents: a provider label long enough to widen the combo
    // past its share of a narrow dock would push the model picker off the
    // row. The full text is one click away in the popup, which is sized to
    // its contents regardless.
    providerCombo_->setSizeAdjustPolicy(QComboBox::AdjustToMinimumContentsLengthWithIcon);
    providerCombo_->setMinimumContentsLength(8);
    providerCombo_->setToolTip(tr("Which provider the next message goes to."));

    // Editable, because the catalogue is a convenience and never a gate: a
    // model the endpoint does not list — a preview id, a fine-tune — must
    // stay typeable, which is the behaviour Settings > AI Providers has
    // always had for this field.
    auto *modelCombo = new LazyComboBox(this);
    modelCombo->setPopulator([this]() { chat_->refreshModels(); });
    modelCombo_ = modelCombo;
    modelCombo_->setEditable(true);
    modelCombo_->setInsertPolicy(QComboBox::NoInsert);
    modelCombo_->setSizeAdjustPolicy(QComboBox::AdjustToMinimumContentsLengthWithIcon);
    modelCombo_->setMinimumContentsLength(8);

    askButton_ = new QPushButton(tr("Ask"), this);
    agentButton_ = new QPushButton(tr("Agent"), this);
    for (QPushButton *button : {askButton_, agentButton_}) {
        button->setCheckable(true);
        button->setFocusPolicy(Qt::NoFocus);
    }
    modeGroup_ = new QButtonGroup(this);
    modeGroup_->setExclusive(true);
    modeGroup_->addButton(askButton_);
    modeGroup_->addButton(agentButton_);
    askButton_->setToolTip(tr("Answer questions; you press Apply on a code block."));
    agentButton_->setToolTip(tr("Let the model use tools, under the approval policy."));

    auto *newChatButton = new QPushButton(tr("New Chat"), this);
    historyButton_ = new QPushButton(tr("History"), this);
    historyButton_->setCheckable(true);

    // Two rows, not one: the dock is routinely docked narrow, and one row
    // holding two combos and four buttons overflows it — which in a dock
    // means a horizontal scrollbar and buttons off the edge, not a tidy
    // squeeze. The pickers get a row of their own and the mode and history
    // buttons get theirs.
    auto *pickers = new QHBoxLayout;
    pickers->addWidget(providerCombo_, 1);
    pickers->addWidget(modelCombo_, 1);

    auto *buttons = new QHBoxLayout;
    buttons->addWidget(askButton_);
    buttons->addWidget(agentButton_);
    buttons->addWidget(newChatButton);
    buttons->addWidget(historyButton_);
    buttons->addStretch(1);

    auto *header = new QVBoxLayout;
    header->addLayout(pickers);
    header->addLayout(buttons);

    // ---- History sidebar -------------------------------------------------
    historyList_ = new QListWidget(this);
    historyList_->setContextMenuPolicy(Qt::CustomContextMenu);
    historyList_->setVisible(false);

    // ---- Transcript ------------------------------------------------------
    // A QVBoxLayout inside a QScrollArea, not a QListWidget: a bubble that
    // grows on every delta and later sprouts an Apply row is size-hint
    // bookkeeping in an item view and free in a layout.
    transcriptBody_ = new QWidget;
    transcriptLayout_ = new QVBoxLayout(transcriptBody_);
    transcriptLayout_->setContentsMargins(tokens::kSp2, tokens::kSp2, tokens::kSp2, tokens::kSp2);
    transcriptLayout_->setSpacing(8);
    transcriptLayout_->addStretch(1);

    transcriptScroll_ = new QScrollArea(this);
    transcriptScroll_->setWidget(transcriptBody_);
    transcriptScroll_->setWidgetResizable(true);
    transcriptScroll_->setFrameShape(QFrame::NoFrame);
    transcriptScroll_->setHorizontalScrollBarPolicy(Qt::ScrollBarAlwaysOff);

    // ---- Attachment chips ------------------------------------------------
    chipsBar_ = new QWidget(this);
    chipsLayout_ = new FlowLayout(chipsBar_, 4);

    // ---- Composer --------------------------------------------------------
    auto *composer = new ComposerEdit(this);
    composer->setPlaceholderText(
      tr("Ask about the attached context.  @ mentions a file  ·  Ctrl+Enter sends"));
    composer->setMaximumHeight(140);
    composer->keyHook = [this](QKeyEvent *event) { return handleComposerKey(event); };
    composer_ = composer;

    sendButton_ = new QPushButton(tr("Send"), this);
    sendButton_->setDefault(true);
    stopButton_ = new QPushButton(tr("Stop"), this);
    stopButton_->setEnabled(false);
    stopButton_->setToolTip(tr("Abandon the request or agent run without applying pending work."));

    auto *composerButtons = new QVBoxLayout;
    composerButtons->addWidget(sendButton_);
    composerButtons->addWidget(stopButton_);
    composerButtons->addStretch(1);

    auto *composerRow = new QHBoxLayout;
    composerRow->addWidget(composer_, 1);
    composerRow->addLayout(composerButtons);

    tokenLabel_ = new QLabel(this);
    tokenLabel_->setAlignment(Qt::AlignRight | Qt::AlignVCenter);

    auto *body = new QWidget(this);
    auto *bodyLayout = new QVBoxLayout(body);
    bodyLayout->setContentsMargins(0, 0, 0, 0);
    bodyLayout->addWidget(transcriptScroll_, 1);
    bodyLayout->addWidget(chipsBar_);
    bodyLayout->addLayout(composerRow);
    bodyLayout->addWidget(tokenLabel_);

    auto *splitter = new QSplitter(Qt::Horizontal, this);
    splitter->addWidget(historyList_);
    splitter->addWidget(body);
    splitter->setStretchFactor(1, 1);
    splitter->setCollapsible(1, false);

    auto *root = new QVBoxLayout(this);
    root->setContentsMargins(tokens::kSp1, tokens::kSp1, tokens::kSp1, tokens::kSp1);
    root->addLayout(header);
    root->addWidget(splitter, 1);

    // ---- @-mention completer --------------------------------------------
    // UnfilteredPopupCompletion is the point, exactly as in CodeEditor:
    // which files match and in what order is `index-core`'s fuzzy ranking,
    // arriving through SearchModel. QCompleter must not re-filter it.
    mentionModel_ = new QStringListModel(this);
    mentionCompleter_ = new QCompleter(mentionModel_, this);
    mentionCompleter_->setCompletionMode(QCompleter::UnfilteredPopupCompletion);
    mentionCompleter_->setWidget(composer_);
    connect(mentionCompleter_,
            qOverload<const QString &>(&QCompleter::activated),
            this,
            &AiChatPanel::acceptMention);

    mentionDebounce_ = new QTimer(this);
    mentionDebounce_->setSingleShot(true);
    mentionDebounce_->setInterval(kMentionDebounceMs);
    connect(mentionDebounce_, &QTimer::timeout, this, &AiChatPanel::runMentionQuery);

    // ---- Wiring ----------------------------------------------------------
    connect(providerCombo_, &QComboBox::activated, this, [this](int index) {
        const QString id = providerCombo_->itemData(index).toString();
        if (!id.isEmpty()) {
            reportResult(chat_->setActiveProvider(id));
        }
    });
    connect(modelCombo_, &QComboBox::activated, this, [this](int index) {
        reportResult(chat_->setModel(modelCombo_->itemText(index)));
    });
    // A typed id is a choice too, and the only way to reach a model the
    // endpoint does not list.
    connect(modelCombo_->lineEdit(), &QLineEdit::editingFinished, this, [this]() {
        const QString typed = modelCombo_->currentText().trimmed();
        if (typed != chat_->currentModel()) {
            reportResult(chat_->setModel(typed));
        }
    });
    connect(askButton_, &QPushButton::clicked, this, [this]() {
        reportResult(chat_->setMode(QStringLiteral("ask")));
    });
    connect(agentButton_, &QPushButton::clicked, this, [this]() {
        reportResult(chat_->setMode(QStringLiteral("agent")));
    });
    connect(newChatButton, &QPushButton::clicked, this, [this]() {
        chat_->newConversation();
        clearApprovalCard();
        rebuildTranscript();
        reloadAttachments();
        reloadTokenUsage();
        updateBusyState();
    });
    connect(historyButton_, &QPushButton::clicked, this, &AiChatPanel::toggleHistory);
    connect(historyList_, &QListWidget::itemClicked, this, [this](QListWidgetItem *item) {
        const QString id = item->data(kConversationIdRole).toString();
        if (id.isEmpty()) {
            return;
        }
        clearApprovalCard();
        reportResult(chat_->loadConversation(id));
        rebuildTranscript();
        reloadAttachments();
        reloadTokenUsage();
        updateBusyState();
    });
    connect(historyList_,
            &QListWidget::customContextMenuRequested,
            this,
            &AiChatPanel::showHistoryMenu);

    connect(sendButton_, &QPushButton::clicked, this, &AiChatPanel::sendMessage);
    connect(stopButton_, &QPushButton::clicked, this, [this]() {
        chat_->cancelRequest();
        clearApprovalCard();
        updateBusyState();
    });
    connect(composer_, &QPlainTextEdit::textChanged, this, &AiChatPanel::onComposerTextChanged);

    connect(chat_, &AiChat::messageStarted, this, &AiChatPanel::onMessageStarted);
    connect(chat_, &AiChat::deltaReceived, this, &AiChatPanel::onDeltaReceived);
    connect(chat_, &AiChat::messageFinished, this, &AiChatPanel::onMessageFinished);
    connect(chat_, &AiChat::chatFailed, this, &AiChatPanel::onChatFailed);
    connect(chat_, &AiChat::attachmentsChanged, this, &AiChatPanel::reloadAttachments);
    connect(chat_, &AiChat::providersChanged, this, &AiChatPanel::reloadProviders);
    connect(chat_, &AiChat::modelsChanged, this, &AiChatPanel::reloadModels);
    connect(chat_, &AiChat::tokenUsageChanged, this, &AiChatPanel::reloadTokenUsage);
    connect(chat_, &AiChat::conversationsChanged, this, &AiChatPanel::reloadConversations);
    connect(chat_, &AiChat::toolCallPending, this, &AiChatPanel::onToolCallPending);
    connect(chat_, &AiChat::toolCallFinished, this, &AiChatPanel::onToolCallFinished);
    connect(chat_, &AiChat::runFinished, this, &AiChatPanel::onRunFinished);

    connect(searchModel_, &SearchModel::resultsBatch, this, &AiChatPanel::onSearchBatch);

    reloadProviders();
    reloadConversations();
    reloadAttachments();
    reloadTokenUsage();
    rebuildTranscript();
    updateBusyState();
}

void AiChatPanel::setApplyHandler(ApplyHandler handler)
{
    applyHandler_ = std::move(handler);
}

void AiChatPanel::setCurrentTextProvider(CurrentTextProvider provider)
{
    currentTextProvider_ = std::move(provider);
}

QString AiChatPanel::currentText() const
{
    return currentTextProvider_ ? currentTextProvider_() : QString();
}

void AiChatPanel::focusComposer()
{
    composer_->setFocus();
}

void AiChatPanel::attachAndFocus()
{
    // Deliberately attaches nothing: the selection lives in the editor, so
    // the Ctrl+L handler calls `attachSelection` (which it can compose and
    // this panel cannot) and then calls this to bring the panel forward.
    show();
    raise();
    focusComposer();
}

// ---------------------------------------------------------------- header --

void AiChatPanel::reloadProviders()
{
    const QSignalBlocker blocker(providerCombo_);
    providerCombo_->clear();
    int activeRow = -1;
    const ::rust::Vec<FfiAiProvider> providers = chat_->providers();
    for (std::size_t i = 0; i < providers.size(); ++i) {
        const FfiAiProvider &provider = providers[i];
        // The model has its own control beside this one, so repeating it
        // here would only make the header line longer.
        QString label = provider.label;
        if (!provider.key_present) {
            label = tr("%1 (no API key)").arg(label);
        }
        providerCombo_->addItem(label, provider.id);
        const int row = providerCombo_->count() - 1;
        if (!provider.key_present) {
            // Greyed but still selectable: the point is that the user can
            // pick it and read *why* it will not work, rather than finding
            // the entry missing and wondering where their provider went. The
            // sentence explaining what to do is Settings > AI Providers's.
            providerCombo_->setItemData(row, QBrush(semanticColors().muted), Qt::ForegroundRole);
            providerCombo_->setItemData(
              row,
              tr("The environment variable holding this provider's key is not set in this "
                 "process. Settings > AI Providers says which one it is."),
              Qt::ToolTipRole);
        }
        if (provider.active) {
            activeRow = row;
        }
    }
    if (activeRow >= 0) {
        providerCombo_->setCurrentIndex(activeRow);
    }

    // The mode toggle follows the same refresh: `mode()` is the authority,
    // never the button that happens to be down.
    const QString mode = chat_->mode();
    askButton_->setChecked(mode != QStringLiteral("agent"));
    agentButton_->setChecked(mode == QStringLiteral("agent"));

    // The provider that changed owns the catalogue, so the models on screen
    // belong to the old one. The new one's are fetched when the user opens
    // the box, not now: nothing should contact a third party because a
    // combo repainted.
    reloadModels();
}

void AiChatPanel::reloadModels()
{
    const QSignalBlocker blocker(modelCombo_);
    const QString current = chat_->currentModel();
    modelCombo_->clear();
    const ::rust::Vec<FfiAiModel> models = chat_->models();
    for (std::size_t i = 0; i < models.size(); ++i) {
        // The entry is the id, not the provider's display name: the box is
        // editable, so whatever it shows is also what a user can type and
        // what `setModel` is handed. The display name rides along as the
        // tooltip rather than becoming a second spelling of the choice.
        modelCombo_->addItem(models[i].id);
        modelCombo_->setItemData(modelCombo_->count() - 1, models[i].label, Qt::ToolTipRole);
    }
    const int row = modelCombo_->findText(current);
    if (row >= 0) {
        modelCombo_->setCurrentIndex(row);
    } else {
        // A model the catalogue does not list — not yet fetched, a fetch
        // that failed, or an id the user typed — is still what the next
        // message goes to, so it is what the box says.
        modelCombo_->setEditText(current);
    }
    // The sentence is `ai-chat-core`'s; this shows it and does not write it.
    modelCombo_->setToolTip(chat_->modelsStatus());
}

void AiChatPanel::reloadConversations()
{
    historyList_->clear();
    const ::rust::Vec<FfiConversation> conversations = chat_->conversations();
    for (const FfiConversation &conversation : conversations) {
        auto *item = new QListWidgetItem(conversation.title, historyList_);
        item->setData(kConversationIdRole, conversation.id);
        item->setToolTip(tr("%1 · %2 messages")
                           .arg(conversation.updated)
                           .arg(static_cast<uint>(conversation.message_count)));
    }
}

void AiChatPanel::toggleHistory()
{
    historyList_->setVisible(historyButton_->isChecked());
}

void AiChatPanel::showHistoryMenu(const QPoint &pos)
{
    QListWidgetItem *item = historyList_->itemAt(pos);
    if (!item) {
        return;
    }
    const QString id = item->data(kConversationIdRole).toString();
    const QString title = item->text();

    QMenu menu(this);
    QAction *rename = menu.addAction(tr("Rename…"));
    QAction *remove = menu.addAction(tr("Delete"));
    QAction *chosen = menu.exec(historyList_->viewport()->mapToGlobal(pos));
    if (chosen == rename) {
        bool accepted = false;
        const QString next =
          QInputDialog::getText(this, tr("Rename Conversation"), tr("Title:"), QLineEdit::Normal,
                                 title, &accepted);
        if (accepted) {
            reportResult(chat_->renameConversation(id, next));
        }
    } else if (chosen == remove) {
        const QMessageBox::StandardButton confirm = QMessageBox::question(
          this, tr("Delete Conversation"),
          tr("Delete \"%1\"? Its transcript is removed from disk.").arg(title));
        if (confirm == QMessageBox::Yes) {
            reportResult(chat_->deleteConversation(id));
        }
    }
}

// ------------------------------------------------------------ transcript --

void AiChatPanel::clearTranscript()
{
    bubbles_.clear();
    approvalCard_ = nullptr;
    // Everything except the trailing stretch, which keeps a short transcript
    // pinned to the top of the viewport.
    while (transcriptLayout_->count() > 1) {
        QLayoutItem *item = transcriptLayout_->takeAt(0);
        if (QWidget *widget = item->widget()) {
            widget->hide();
            widget->deleteLater();
        }
        delete item;
    }
}

void AiChatPanel::addTranscriptWidget(QWidget *widget)
{
    widget->setParent(transcriptBody_);
    transcriptLayout_->insertWidget(transcriptLayout_->count() - 1, widget);
}

QFrame *AiChatPanel::makeBubbleFrame(const QString &role, QTextBrowser **browserOut)
{
    const bool user = role == QStringLiteral("user");
    auto *frame = new QFrame;
    frame->setFrameShape(QFrame::StyledPanel);
    frame->setStyleSheet(
      QStringLiteral("QFrame { border: 1px solid %1; border-radius: %2px; }")
        .arg(semanticColors().muted.name())
        .arg(tokens::kRadiusControl));

    auto *layout = new QVBoxLayout(frame);
    layout->setContentsMargins(tokens::kSp2, tokens::kSp1, tokens::kSp2, tokens::kSp1);
    layout->setSpacing(2);

    auto *who = subordinateLabel(user ? tr("You") : tr("Assistant"), semanticColors().muted);
    who->setStyleSheet(who->styleSheet() + QStringLiteral(" border: none;"));
    layout->addWidget(who);

    auto *browser = new QTextBrowser(frame);
    // SECURITY (ADR-0021): assistant output is untrusted text. Both switches
    // are needed — setOpenLinks(false) stops QTextBrowser navigating itself,
    // setOpenExternalLinks(false) stops it handing a URL to the desktop. No
    // anchorClicked handler is connected, so a link the model emits is inert.
    browser->setOpenLinks(false);
    browser->setOpenExternalLinks(false);
    browser->setReadOnly(true);
    browser->setFrameShape(QFrame::NoFrame);
    browser->setStyleSheet(QStringLiteral("border: none;"));
    browser->setVerticalScrollBarPolicy(Qt::ScrollBarAlwaysOff);
    browser->setHorizontalScrollBarPolicy(Qt::ScrollBarAsNeeded);
    browser->setTextInteractionFlags(Qt::TextSelectableByMouse | Qt::TextSelectableByKeyboard);
    // The bubble grows with its content instead of scrolling internally:
    // the transcript's single scrollbar is the one the user reasons about.
    connect(browser->document()->documentLayout(),
            &QAbstractTextDocumentLayout::documentSizeChanged,
            browser,
            [browser](const QSizeF &size) {
                browser->setFixedHeight(static_cast<int>(size.height()) + kHeightPadding);
            });
    layout->addWidget(browser);

    *browserOut = browser;
    return frame;
}

AiChatPanel::Bubble &AiChatPanel::bubbleAt(quint64 index, const QString &role)
{
    auto existing = bubbles_.find(index);
    if (existing != bubbles_.end()) {
        return *existing;
    }
    Bubble bubble;
    bubble.frame = makeBubbleFrame(role, &bubble.browser);
    addTranscriptWidget(bubble.frame);
    return *bubbles_.insert(index, bubble);
}

void AiChatPanel::appendErrorBubble(const QString &message)
{
    auto *frame = new QFrame;
    frame->setFrameShape(QFrame::StyledPanel);
    // Distinct styling rather than a modal: a failed request must not steal
    // the caret from someone mid-sentence.
    frame->setStyleSheet(QStringLiteral("QFrame { border: 1px solid %1; border-radius: %2px; }")
                           .arg(semanticColors().error.name())
                           .arg(tokens::kRadiusControl));
    auto *layout = new QVBoxLayout(frame);
    layout->setContentsMargins(tokens::kSp2, tokens::kSp1, tokens::kSp2, tokens::kSp1);
    // The wording is `ai-chat-core`'s — ChatError composes what a failure
    // means in English, this only picks the colour it is painted in.
    auto *label = subordinateLabel(message, semanticColors().error);
    label->setStyleSheet(label->styleSheet() + QStringLiteral(" border: none;"));
    layout->addWidget(label);
    addTranscriptWidget(frame);
}

void AiChatPanel::appendOutcomeRow(const QString &text)
{
    QLabel *label = subordinateLabel(text, semanticColors().muted);
    addTranscriptWidget(label);
}

void AiChatPanel::appendCodeBlockRow(quint64 messageIndex)
{
    const ::rust::Vec<FfiCodeBlock> blocks = chat_->codeBlocks(messageIndex);
    if (blocks.empty()) {
        return;
    }
    Bubble &bubble = bubbles_[messageIndex];
    if (bubble.applyRow) {
        bubble.applyRow->hide();
        bubble.applyRow->deleteLater();
        bubble.applyRow = nullptr;
    }

    auto *row = new QWidget;
    auto *rows = new QVBoxLayout(row);
    rows->setContentsMargins(tokens::kSp2, 0, tokens::kSp2, 0);
    rows->setSpacing(2);
    for (std::size_t i = 0; i < blocks.size(); ++i) {
        const FfiCodeBlock &block = blocks[i];
        const quint64 blockIndex = static_cast<quint64>(i);
        const QString text = block.text;
        // Whichever of the two the block carries; the label is only a
        // handle, so an unlabelled fence still gets a usable one.
        QString name = block.path;
        if (name.isEmpty()) {
            name = block.language;
        }
        if (name.isEmpty()) {
            name = tr("Code block %1").arg(static_cast<uint>(i + 1));
        }

        auto *line = new QHBoxLayout;
        line->addWidget(subordinateLabel(name, semanticColors().muted), 1);

        auto *apply = new QPushButton(tr("Apply"), row);
        apply->setToolTip(tr("Preview and apply this block through the refactoring preview."));
        connect(apply, &QPushButton::clicked, this, [this, messageIndex, blockIndex]() {
            // Nothing the model emitted is executed here. The handler runs
            // `prepareApply` in Rust and, when the plan reaches beyond the
            // open buffer, shows RefactorPreviewDialog first.
            if (applyHandler_) {
                applyHandler_(messageIndex, blockIndex);
            }
        });
        line->addWidget(apply);

        auto *copy = new QPushButton(tr("Copy"), row);
        connect(copy, &QPushButton::clicked, this, [text]() {
            QGuiApplication::clipboard()->setText(text);
        });
        line->addWidget(copy);

        rows->addLayout(line);
    }

    addTranscriptWidget(row);
    bubble.applyRow = row;
}

bool AiChatPanel::transcriptAtBottom() const
{
    const QScrollBar *bar = transcriptScroll_->verticalScrollBar();
    // A few pixels of slack: a scrollbar that has just been resized by an
    // arriving delta is rarely exactly at its maximum.
    return bar->value() >= bar->maximum() - 4;
}

void AiChatPanel::scrollToBottomSoon()
{
    // Deferred, because the bar's maximum only grows once the layout has
    // taken the new content into account.
    QTimer::singleShot(0, this, [this]() {
        QScrollBar *bar = transcriptScroll_->verticalScrollBar();
        bar->setValue(bar->maximum());
    });
}

void AiChatPanel::rebuildTranscript()
{
    clearTranscript();
    const ::rust::Vec<FfiChatMessage> messages = chat_->messages();
    for (std::size_t i = 0; i < messages.size(); ++i) {
        const FfiChatMessage &message = messages[i];
        const quint64 index = static_cast<quint64>(i);
        // `kind` is a declared vocabulary, not a judgement: the panel maps
        // each value to a shape, it never decides which one a turn is.
        if (message.kind == QStringLiteral("error")) {
            appendErrorBubble(message.text);
            continue;
        }
        if (message.kind == QStringLiteral("tool")) {
            appendOutcomeRow(message.text);
            continue;
        }
        Bubble &bubble = bubbleAt(index, message.role);
        bubble.markdown = message.text;
        bubble.browser->setMarkdown(bubble.markdown);
        if (!message.streaming && message.role != QStringLiteral("user")) {
            appendCodeBlockRow(index);
        }
    }
    scrollToBottomSoon();
}

// --------------------------------------------------------- streaming ------

void AiChatPanel::onMessageStarted(quint64 index)
{
    const bool pinned = transcriptAtBottom();
    Bubble &bubble = bubbleAt(index, QStringLiteral("assistant"));
    bubble.markdown.clear();
    bubble.browser->setMarkdown(QString());
    if (pinned) {
        scrollToBottomSoon();
    }
    updateBusyState();
}

void AiChatPanel::onDeltaReceived(quint64 index, const QString &text)
{
    // Measured BEFORE the append: a reader who scrolled up to re-read
    // something must never be yanked back down by the next token.
    const bool pinned = transcriptAtBottom();
    Bubble &bubble = bubbleAt(index, QStringLiteral("assistant"));
    bubble.markdown += text;
    // ponytail: re-renders the whole turn per delta — QTextBrowser has no
    // incremental Markdown append, and a half-arrived fence cannot be parsed
    // in isolation anyway. If a very long answer starts to stutter, coalesce
    // deltas behind a short timer before touching the document.
    bubble.browser->setMarkdown(bubble.markdown);
    if (pinned) {
        scrollToBottomSoon();
    }
}

void AiChatPanel::onMessageFinished(quint64 index)
{
    const bool pinned = transcriptAtBottom();
    appendCodeBlockRow(index);
    if (pinned) {
        scrollToBottomSoon();
    }
    updateBusyState();
}

void AiChatPanel::onChatFailed(const FfiResult &result)
{
    appendErrorBubble(result.message);
    clearApprovalCard();
    scrollToBottomSoon();
    updateBusyState();
}

// ------------------------------------------------------------- agent ------

void AiChatPanel::clearApprovalCard()
{
    if (approvalCard_) {
        approvalCard_->hide();
        approvalCard_->deleteLater();
        approvalCard_ = nullptr;
    }
    pendingCallId_.clear();
}

void AiChatPanel::onToolCallPending(const FfiToolCall &call)
{
    clearApprovalCard();
    // Copies, not references into the signal's argument: the buttons outlive
    // the emission.
    const QString callId = call.call_id;

    auto *card = new QFrame;
    card->setFrameShape(QFrame::StyledPanel);
    card->setStyleSheet(QStringLiteral("QFrame { border: 1px solid %1; border-radius: %2px; }")
                          .arg(semanticColors().warning.name())
                          .arg(tokens::kRadiusControl));
    auto *layout = new QVBoxLayout(card);
    layout->setContentsMargins(tokens::kSp2, tokens::kSp1, tokens::kSp2, tokens::kSp1);

    // The sentence is `ai-chat-core`'s summary of the call, painted verbatim.
    auto *summary = new QLabel(call.summary, card);
    summary->setWordWrap(true);
    summary->setTextInteractionFlags(Qt::TextSelectableByMouse);
    summary->setStyleSheet(QStringLiteral("border: none;"));
    layout->addWidget(summary);

    // The raw arguments behind a disclosure: always available, never in the
    // way. Plain text, never Markdown — this is JSON the model wrote, and
    // rendering it as rich text would let it style itself.
    auto *details = new QToolButton(card);
    details->setText(tr("Details"));
    details->setCheckable(true);
    details->setAutoRaise(true);
    details->setStyleSheet(QStringLiteral("border: none;"));
    layout->addWidget(details, 0, Qt::AlignLeft);

    auto *arguments = new QPlainTextEdit(card);
    arguments->setPlainText(call.arguments);
    arguments->setReadOnly(true);
    arguments->setMaximumHeight(160);
    arguments->setVisible(false);
    layout->addWidget(arguments);
    connect(details, &QToolButton::toggled, arguments, &QWidget::setVisible);

    auto *buttons = new QHBoxLayout;
    buttons->addStretch(1);
    auto *approve = new QPushButton(tr("Approve"), card);
    auto *always = new QPushButton(tr("Approve always"), card);
    always->setToolTip(tr("Allow this tool without asking for the rest of this conversation."));
    auto *deny = new QPushButton(tr("Deny"), card);
    buttons->addWidget(approve);
    buttons->addWidget(always);
    buttons->addWidget(deny);
    layout->addLayout(buttons);

    connect(approve, &QPushButton::clicked, this, [this, callId]() {
        const FfiResult result = chat_->approveTool(callId, false);
        clearApprovalCard();
        reportResult(result);
        updateBusyState();
    });
    connect(always, &QPushButton::clicked, this, [this, callId]() {
        const FfiResult result = chat_->approveTool(callId, true);
        clearApprovalCard();
        reportResult(result);
        updateBusyState();
    });
    connect(deny, &QPushButton::clicked, this, [this, callId]() {
        // An empty reason on purpose: what a bare denial says to the model
        // is `ai-chat-core`'s wording, not a sentence composed in the view.
        const FfiResult result = chat_->denyTool(callId, QString());
        clearApprovalCard();
        reportResult(result);
        updateBusyState();
    });

    addTranscriptWidget(card);
    approvalCard_ = card;
    pendingCallId_ = callId;
    scrollToBottomSoon();
    updateBusyState();
}

void AiChatPanel::onToolCallFinished(const FfiToolOutcome &outcome)
{
    const bool pinned = transcriptAtBottom();
    QString text = tr("%1 · %2").arg(outcome.tool, outcome.status);
    const QString detail = outcome.detail;
    if (!detail.isEmpty()) {
        text += tr(" — %1").arg(detail);
    }
    appendOutcomeRow(text);
    if (pinned) {
        scrollToBottomSoon();
    }
}

void AiChatPanel::onRunFinished(const FfiResult &result)
{
    const bool pinned = transcriptAtBottom();
    QString text = tr("Run finished after %1 steps.").arg(static_cast<uint>(chat_->runStepCount()));
    const QString message = result.message;
    if (!message.isEmpty()) {
        text += QStringLiteral(" ") + message;
    }
    QLabel *label =
      subordinateLabel(text, result.code == 0 ? semanticColors().muted : semanticColors().error);
    addTranscriptWidget(label);
    clearApprovalCard();
    if (pinned) {
        scrollToBottomSoon();
    }
    updateBusyState();
}

// ---------------------------------------------------- composer / chips ----

void AiChatPanel::sendMessage()
{
    const QString text = composer_->toPlainText().trimmed();
    if (text.isEmpty()) {
        return;
    }
    cancelMention();
    composer_->clear();
    const FfiResult result = chat_->sendMessage(text);
    if (result.code != 0) {
        // Put the text back rather than swallowing it — nothing is more
        // annoying than a refused send that also ate the question.
        composer_->setPlainText(text);
        reportResult(result);
        return;
    }
    // `messages()` already contains the user turn (and the in-flight
    // assistant one), so rebuilding is how the user turn gets on screen at
    // its real index — there is no `messageStarted` for it.
    rebuildTranscript();
    updateBusyState();
}

void AiChatPanel::reloadAttachments()
{
    clearLayout(chipsLayout_);
    const ::rust::Vec<FfiAttachment> attachments = chat_->attachments();
    for (std::size_t i = 0; i < attachments.size(); ++i) {
        const FfiAttachment &attachment = attachments[i];
        const quint64 index = static_cast<quint64>(i);

        auto *chip = new QFrame(chipsBar_);
        chip->setFrameShape(QFrame::StyledPanel);
        chip->setStyleSheet(QStringLiteral("QFrame { border: 1px solid %1; border-radius: %2px; }")
                              .arg(semanticColors().muted.name())
                              .arg(tokens::kRadiusPanel));
        auto *layout = new QHBoxLayout(chip);
        layout->setContentsMargins(6, 2, 4, 2);
        layout->setSpacing(4);

        // The per-chip cost is shown because the whole point of the chips
        // bar is that the user can see exactly what is about to be sent, and
        // "how much" is part of "what".
        auto *label = new QLabel(tr("%1  (%2 tok)")
                                    .arg(attachment.label)
                                    .arg(static_cast<uint>(attachment.tokens)),
                                  chip);
        label->setToolTip(attachment.detail);
        label->setStyleSheet(QStringLiteral("border: none;"));
        layout->addWidget(label);

        auto *remove = new QToolButton(chip);
        remove->setText(QStringLiteral("✕"));
        remove->setAutoRaise(true);
        remove->setStyleSheet(QStringLiteral("border: none;"));
        remove->setToolTip(tr("Remove this attachment"));
        connect(remove, &QToolButton::clicked, this, [this, index]() {
            chat_->removeAttachment(index);
        });
        layout->addWidget(remove);

        chipsLayout_->addWidget(chip);
    }
    // A bar with nothing in it is a strip of empty space above the composer.
    chipsBar_->setVisible(!attachments.empty());
    chipsBar_->updateGeometry();
}

void AiChatPanel::reloadTokenUsage()
{
    const FfiTokenUsage usage = chat_->tokenUsage();
    tokenLabel_->setText(tokenText(usage));
    tokenLabel_->setToolTip(usage.exact
                              ? tr("Counted by the provider's own tokenizer.")
                              : tr("An estimate — no tokenizer was reachable for this provider."));
    tokenLabel_->setStyleSheet(
      QStringLiteral("color: %1;")
        .arg((usage.exact ? semanticColors().muted : semanticColors().warning).name()));
}

void AiChatPanel::updateBusyState()
{
    const bool streaming = chat_->isStreaming();
    const bool awaitingApproval = !pendingCallId_.isEmpty();
    // A pending approval blocks the run, so it blocks the composer too:
    // typing a second question while the first is stuck on a decision is a
    // way to lose the question.
    composer_->setEnabled(!awaitingApproval);
    sendButton_->setEnabled(!awaitingApproval && !streaming);
    stopButton_->setEnabled(streaming || awaitingApproval);
}

void AiChatPanel::reportResult(const FfiResult &result)
{
    if (result.code != 0) {
        appendErrorBubble(result.message);
        scrollToBottomSoon();
    }
}

// ----------------------------------------------------------- @-mention ----

bool AiChatPanel::handleComposerKey(QKeyEvent *event)
{
    const bool enter = event->key() == Qt::Key_Return || event->key() == Qt::Key_Enter;

    if (mentionCompleter_->popup()->isVisible()) {
        if (enter || event->key() == Qt::Key_Tab) {
            const QModelIndex current = mentionCompleter_->popup()->currentIndex();
            if (current.isValid()) {
                acceptMention(current.data().toString());
                return true;
            }
        }
        if (event->key() == Qt::Key_Escape) {
            cancelMention();
            return true;
        }
    }

    // Ctrl+Enter sends; plain Enter is a newline, because a chat message
    // about code is routinely more than one line.
    if (enter && event->modifiers().testFlag(Qt::ControlModifier)) {
        sendMessage();
        return true;
    }
    return false;
}

void AiChatPanel::onComposerTextChanged()
{
    const int caret = composer_->textCursor().position();
    const QString text = composer_->toPlainText();
    // The unsent draft is part of what the next request will cost, so the
    // counter has to charge for it as it is typed. Rust owns the counting
    // and the caching (an unchanged string is not re-tokenised), which is
    // why this hands over the text rather than measuring anything here.
    chat_->setComposerText(text);
    int anchor = -1;
    // Walk back from the caret to the '@' that starts the mention. Any
    // whitespace before it means the caret is no longer inside one.
    for (int i = caret - 1; i >= 0; --i) {
        const QChar character = text.at(i);
        if (character == QLatin1Char('@')) {
            anchor = i;
            break;
        }
        if (character.isSpace()) {
            break;
        }
    }
    if (anchor < 0) {
        cancelMention();
        return;
    }
    mentionAnchor_ = anchor;
    mentionDebounce_->start();
}

void AiChatPanel::runMentionQuery()
{
    if (mentionAnchor_ < 0) {
        return;
    }
    const int caret = composer_->textCursor().position();
    const QString query = composer_->toPlainText().mid(mentionAnchor_ + 1, caret - mentionAnchor_ - 1);
    // The same generation-tagged protocol Search Everywhere uses: a newer
    // query cancels the running one and stale batches are dropped.
    ++mentionGeneration_;
    searchModel_->searchEverywhere(query, FfiTierFilter::Files, mentionGeneration_, kMentionLimit);
}

void AiChatPanel::onSearchBatch(quint64 generation, const ::rust::Vec<FfiSearchHit> &hits)
{
    if (mentionAnchor_ < 0 || generation != mentionGeneration_) {
        return;
    }
    QStringList paths;
    for (const FfiSearchHit &hit : hits) {
        const QString path = hit.path;
        if (!path.isEmpty() && !paths.contains(path)) {
            paths.append(path);
        }
    }
    mentionModel_->setStringList(paths);
    if (paths.isEmpty()) {
        mentionCompleter_->popup()->hide();
        return;
    }
    QAbstractItemView *popup = mentionCompleter_->popup();
    popup->setCurrentIndex(mentionCompleter_->completionModel()->index(0, 0));
    QRect anchor = composer_->cursorRect();
    anchor.setWidth(popup->sizeHintForColumn(0) + popup->verticalScrollBar()->sizeHint().width()
                    + kPopupWidthPadding);
    mentionCompleter_->complete(anchor);
}

void AiChatPanel::acceptMention(const QString &path)
{
    const int anchor = mentionAnchor_;
    if (anchor < 0 || path.isEmpty()) {
        return;
    }
    // Take the "@query" text back out: the attachment is recorded by its
    // chip, and leaving the mention behind would send the path twice.
    QTextCursor cursor = composer_->textCursor();
    const int caret = cursor.position();
    cursor.setPosition(anchor);
    cursor.setPosition(qMax(anchor, caret), QTextCursor::KeepAnchor);
    cursor.removeSelectedText();
    composer_->setTextCursor(cursor);

    cancelMention();
    // Whether this file may be attached at all — secret-shaped names, files
    // outside the project — is `ai-chat-core`'s ruling, arriving as a code.
    reportResult(chat_->attachFile(path));
}

void AiChatPanel::cancelMention()
{
    mentionAnchor_ = -1;
    mentionDebounce_->stop();
    mentionCompleter_->popup()->hide();
}

} // namespace ui_shell
