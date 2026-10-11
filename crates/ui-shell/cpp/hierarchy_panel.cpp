#include "hierarchy_panel.h"

#include "editor_tabs.h"

#include <QComboBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kPathRole = Qt::UserRole;
constexpr int kLineRole = Qt::UserRole + 1;
constexpr int kColumnRole = Qt::UserRole + 2;
// Marks a node's single placeholder child — never a real hierarchy item, so
// onItemDoubleClicked and every jump-target reader skip it via kPathRole
// being unset (QVariant() converts to an empty QString).
constexpr int kPlaceholderRole = Qt::UserRole + 3;

QString itemLabel(const FfiHierarchyItem &item, const QString &suffix)
{
    QString label = item.name;
    if (!item.detail.isEmpty()) {
        label += QStringLiteral(" (%1)").arg(item.detail);
    }
    if (!suffix.isEmpty()) {
        label += QStringLiteral(" — %1").arg(suffix);
    }
    return label;
}

} // namespace

HierarchyPanel::HierarchyPanel(LanguageService *languageService, EditorTabs *editorTabs,
                               QWidget *parent)
  : QWidget(parent)
  , languageService_(languageService)
  , editorTabs_(editorTabs)
{
    modeCombo_ = new QComboBox(this);
    modeCombo_->addItem(tr("Incoming Calls"));
    modeCombo_->addItem(tr("Outgoing Calls"));
    modeCombo_->addItem(tr("Supertypes"));
    modeCombo_->addItem(tr("Subtypes"));
    statusLabel_ = new QLabel(this);
    tree_ = new QTreeWidget(this);
    tree_->setHeaderHidden(true);

    auto *header = new QHBoxLayout;
    header->addWidget(modeCombo_);
    header->addWidget(statusLabel_, 1);

    auto *layout = new QVBoxLayout(this);
    layout->addLayout(header);
    layout->addWidget(tree_, 1);

    connect(modeCombo_, qOverload<int>(&QComboBox::currentIndexChanged), this,
            &HierarchyPanel::onModeChanged);
    connect(tree_, &QTreeWidget::itemExpanded, this, &HierarchyPanel::onItemExpanded);
    connect(tree_, &QTreeWidget::itemDoubleClicked, this, &HierarchyPanel::onItemDoubleClicked);

    connect(languageService_, &LanguageService::callHierarchyReady, this,
            &HierarchyPanel::onHierarchyPrepared);
    connect(languageService_, &LanguageService::typeHierarchyReady, this,
            &HierarchyPanel::onHierarchyPrepared);
    connect(languageService_, &LanguageService::incomingCallsReady, this,
            &HierarchyPanel::onIncomingCallsReady);
    connect(languageService_, &LanguageService::outgoingCallsReady, this,
            &HierarchyPanel::onOutgoingCallsReady);
    connect(languageService_, &LanguageService::supertypesReady, this,
            &HierarchyPanel::onSupertypesReady);
    connect(languageService_, &LanguageService::subtypesReady, this,
            &HierarchyPanel::onSubtypesReady);
}

void HierarchyPanel::showCallHierarchyAt(const QString &path, quint32 line, quint32 character)
{
    mode_ = Mode::IncomingCalls;
    modeCombo_->blockSignals(true);
    modeCombo_->setCurrentIndex(0);
    modeCombo_->blockSignals(false);
    rootPath_ = path;
    rootLine_ = line;
    rootCharacter_ = character;
    restartFromRoot();
}

void HierarchyPanel::showTypeHierarchyAt(const QString &path, quint32 line, quint32 character)
{
    mode_ = Mode::Supertypes;
    modeCombo_->blockSignals(true);
    modeCombo_->setCurrentIndex(2);
    modeCombo_->blockSignals(false);
    rootPath_ = path;
    rootLine_ = line;
    rootCharacter_ = character;
    restartFromRoot();
}

void HierarchyPanel::restartFromRoot()
{
    tree_->clear();
    expandTarget_ = nullptr;
    if (rootPath_.isEmpty()) {
        return;
    }
    statusLabel_->setText(tr("Loading…"));
    if (isCallMode()) {
        languageService_->requestCallHierarchy(rootPath_, rootLine_, rootCharacter_);
    } else {
        languageService_->requestTypeHierarchy(rootPath_, rootLine_, rootCharacter_);
    }
}

void HierarchyPanel::expandNode(QTreeWidgetItem *node)
{
    const QString path = node->data(0, kPathRole).toString();
    const quint32 line = node->data(0, kLineRole).toUInt();
    const quint32 column = node->data(0, kColumnRole).toUInt();
    if (path.isEmpty() || line == 0) {
        return;
    }
    expandTarget_ = node;
    // FfiHierarchyItem's line is 1-based (a jump target); the prepare
    // requests take LSP's own 0-based line, the same convention
    // EditorTabs::lspPositionAt's callers already send.
    if (isCallMode()) {
        languageService_->requestCallHierarchy(path, line - 1, column);
    } else {
        languageService_->requestTypeHierarchy(path, line - 1, column);
    }
}

void HierarchyPanel::onModeChanged(int index)
{
    mode_ = static_cast<Mode>(index);
    restartFromRoot();
}

void HierarchyPanel::onItemExpanded(QTreeWidgetItem *item)
{
    if (takePlaceholder(item)) {
        expandNode(item);
    }
}

void HierarchyPanel::onItemDoubleClicked(QTreeWidgetItem *item)
{
    const QString path = item->data(0, kPathRole).toString();
    if (path.isEmpty()) {
        return;
    }
    editorTabs_->openFileAtLine(path, static_cast<int>(item->data(0, kLineRole).toUInt()),
                                static_cast<int>(item->data(0, kColumnRole).toUInt()));
}

void HierarchyPanel::onHierarchyPrepared()
{
    const ::rust::Vec<FfiHierarchyItem> items =
      isCallMode() ? languageService_->callHierarchyItems() : languageService_->typeHierarchyItems();
    if (!expandTarget_) {
        // The root: rebuild the tree from scratch around whatever the
        // server resolved at rootLine_/rootCharacter_.
        tree_->clear();
        if (items.empty()) {
            statusLabel_->setText(tr("No hierarchy available here."));
            return;
        }
        statusLabel_->clear();
        for (const FfiHierarchyItem &item : items) {
            tree_->addTopLevelItem(addNode(nullptr, item, QString()));
        }
        // Item 0's own edges are fetched right away — server-side, this
        // prepare answer already scoped to exactly this node — so the
        // first expand is instant rather than a visible round trip. Left
        // collapsed rather than auto-expanded: forcing that would emit
        // itemExpanded and re-trigger expandNode's own re-prepare on the
        // same node, racing this fetch.
        expandTarget_ = tree_->topLevelItem(0);
        requestEdgesForTarget();
        return;
    }
    if (items.empty()) {
        // The re-prepare at this node's own position found nothing —
        // leave it childless (its placeholder was already removed by
        // onItemExpanded before this round trip started).
        expandTarget_ = nullptr;
        return;
    }
    requestEdgesForTarget();
}

void HierarchyPanel::requestEdgesForTarget()
{
    switch (mode_) {
        case Mode::IncomingCalls:
            languageService_->requestIncomingCalls(0);
            break;
        case Mode::OutgoingCalls:
            languageService_->requestOutgoingCalls(0);
            break;
        case Mode::Supertypes:
            languageService_->requestSupertypes(0);
            break;
        case Mode::Subtypes:
            languageService_->requestSubtypes(0);
            break;
    }
}

void HierarchyPanel::onIncomingCallsReady()
{
    if (!expandTarget_) {
        return;
    }
    for (const FfiIncomingCall &call : languageService_->incomingCalls()) {
        expandTarget_->addChild(
          addNode(nullptr, call.from, call.call_count == 1 ? tr("1 call")
                                                            : tr("%1 calls").arg(call.call_count)));
    }
    expandTarget_ = nullptr;
}

void HierarchyPanel::onOutgoingCallsReady()
{
    if (!expandTarget_) {
        return;
    }
    for (const FfiOutgoingCall &call : languageService_->outgoingCalls()) {
        expandTarget_->addChild(
          addNode(nullptr, call.to, call.call_count == 1 ? tr("1 call")
                                                          : tr("%1 calls").arg(call.call_count)));
    }
    expandTarget_ = nullptr;
}

void HierarchyPanel::onSupertypesReady()
{
    if (!expandTarget_) {
        return;
    }
    for (const FfiHierarchyItem &item : languageService_->supertypes()) {
        expandTarget_->addChild(addNode(nullptr, item, QString()));
    }
    expandTarget_ = nullptr;
}

void HierarchyPanel::onSubtypesReady()
{
    if (!expandTarget_) {
        return;
    }
    for (const FfiHierarchyItem &item : languageService_->subtypes()) {
        expandTarget_->addChild(addNode(nullptr, item, QString()));
    }
    expandTarget_ = nullptr;
}

QTreeWidgetItem *HierarchyPanel::addNode(QTreeWidgetItem *parent, const FfiHierarchyItem &item,
                                         const QString &suffix)
{
    auto *node = new QTreeWidgetItem(parent, QStringList{ itemLabel(item, suffix) });
    node->setData(0, kPathRole, item.path);
    node->setData(0, kLineRole, item.line);
    node->setData(0, kColumnRole, item.column);
    // Loading placeholder: gives the node an expand arrow before its own
    // edges are known, replaced by onItemExpanded/expandNode's round trip.
    auto *placeholder = new QTreeWidgetItem(node, QStringList{ tr("Loading…") });
    placeholder->setData(0, kPlaceholderRole, true);
    placeholder->setFlags(Qt::NoItemFlags);
    return node;
}

bool HierarchyPanel::takePlaceholder(QTreeWidgetItem *node)
{
    if (node->childCount() != 1) {
        return false;
    }
    QTreeWidgetItem *child = node->child(0);
    if (!child->data(0, kPlaceholderRole).toBool()) {
        return false;
    }
    node->removeChild(child);
    delete child;
    return true;
}

} // namespace ui_shell
