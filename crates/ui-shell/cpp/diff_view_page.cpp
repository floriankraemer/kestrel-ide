#include "diff_view_page.h"

#include "diff_divider.h"
#include "diff_toolbar.h"
#include "diff_view.h"
#include "e2e_mark.h"
#include "ui_tokens.h"

#include <QHBoxLayout>
#include <QLabel>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

const char *whitespaceName(FfiWhitespaceMode mode)
{
    switch (mode) {
    case FfiWhitespaceMode::TrimEnds:
        return "trim_ends";
    case FfiWhitespaceMode::IgnoreAll:
        return "ignore_all";
    case FfiWhitespaceMode::IgnoreAllAndBlankLines:
        return "ignore_all_and_blank_lines";
    case FfiWhitespaceMode::Exact:
    default:
        return "exact";
    }
}

const char *highlightName(FfiHighlightMode mode)
{
    switch (mode) {
    case FfiHighlightMode::Chars:
        return "chars";
    case FfiHighlightMode::Lines:
        return "lines";
    case FfiHighlightMode::None:
        return "none";
    case FfiHighlightMode::Words:
    default:
        return "words";
    }
}

} // namespace

DiffViewPage::DiffViewPage(DiffView *diffView,
                           const QString &leftLabel,
                           const QString &rightLabel,
                           DiffRecompute recompute,
                           QWidget *parent)
  : QWidget(parent)
  , toolbar_(new DiffToolbar(this))
  , leftHeader_(new QLabel(leftLabel, this))
  , rightHeader_(new QLabel(rightLabel, this))
  , diffView_(diffView)
  , recompute_(std::move(recompute))
{
    connect(toolbar_, &DiffToolbar::previousRequested, diffView_, &DiffView::selectPreviousHunk);
    connect(toolbar_, &DiffToolbar::nextRequested, diffView_, &DiffView::selectNextHunk);
    connect(toolbar_, &DiffToolbar::optionsChanged, this, &DiffViewPage::refresh);

    // One label over each pane, separated by the divider's own width so the
    // names sit over the text they name.
    auto *header = new QWidget(this);
    header->setObjectName(QStringLiteral("diffPaneHeader"));
    auto *headerLayout = new QHBoxLayout(header);
    headerLayout->setContentsMargins(tokens::kSp2, tokens::kSp1, tokens::kSp2, tokens::kSp1);
    headerLayout->setSpacing(0);
    headerLayout->addWidget(leftHeader_, 1);
    headerLayout->addSpacing(DiffDivider::kWidth);
    headerLayout->addWidget(rightHeader_, 1);

    diffView_->setParent(this);
    diffView_->divider()->onApplyHunk = [this](int index) {
        if (applyHandler_ && index >= 0 && static_cast<std::size_t>(index) < data_.hunks.size()) {
            applyHandler_(data_.hunks[static_cast<std::size_t>(index)]);
        }
    };

    auto *layout = new QVBoxLayout(this);
    layout->setContentsMargins(0, 0, 0, 0);
    layout->setSpacing(0);
    layout->addWidget(toolbar_);
    layout->addWidget(header);
    layout->addWidget(diffView_, 1);

    refresh();
}

void DiffViewPage::refresh()
{
    if (refreshing_ || !recompute_) {
        return;
    }
    refreshing_ = true;
    const FfiWhitespaceMode whitespace = toolbar_->whitespaceMode();
    const FfiHighlightMode highlight = toolbar_->highlightMode();
    data_ = recompute_(whitespace, highlight);
    diffView_->setOptions(toolbar_->collapseUnchanged(), toolbar_->syncScroll(), highlight);
    diffView_->setDiff(data_.hunks, data_.spans, data_.rows);
    toolbar_->setDifferenceCount(static_cast<int>(data_.hunks.size()));
    e2eMark(QStringLiteral("{\"ev\":\"diff_recomputed\",\"hunks\":%1,\"whitespace\":\"%2\","
                           "\"highlight\":\"%3\"}")
              .arg(data_.hunks.size())
              .arg(QLatin1String(whitespaceName(whitespace)),
                   QLatin1String(highlightName(highlight))));
    refreshing_ = false;
}

void DiffViewPage::setApplyHandler(std::function<void(const FfiHunk &)> handler)
{
    applyHandler_ = std::move(handler);
    diffView_->divider()->setEditable(static_cast<bool>(applyHandler_));
}

} // namespace ui_shell
