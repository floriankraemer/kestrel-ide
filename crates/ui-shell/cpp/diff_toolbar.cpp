#include "diff_toolbar.h"

#include "e2e_mark.h"
#include "theme.h"
#include "ui_tokens.h"

#include <QComboBox>
#include <QHBoxLayout>
#include <QLabel>
#include <QShowEvent>
#include <QTimer>
#include <QToolButton>

namespace ui_shell {

namespace {

QToolButton *iconButton(const char *mask, const QString &toolTip, QWidget *parent)
{
    auto *button = new QToolButton(parent);
    button->setIcon(maskIcon(mask, chromePaletteForTheme(activeThemeName()).textDim));
    button->setIconSize(QSize(16, 16));
    button->setAutoRaise(true);
    button->setToolTip(toolTip);
    button->setFocusPolicy(Qt::NoFocus);
    return button;
}

QString rectJson(const QWidget *widget)
{
    const QRect rect(widget->mapToGlobal(QPoint(0, 0)), widget->size());
    return QStringLiteral("[%1,%2,%3,%4]")
      .arg(rect.x())
      .arg(rect.y())
      .arg(rect.width())
      .arg(rect.height());
}

} // namespace

DiffToolbar::DiffToolbar(QWidget *parent)
  : QWidget(parent)
{
    setObjectName(QStringLiteral("diffToolbar"));

    previous_ = iconButton(":/ui/icons/diff/arrow_up.a8", tr("Previous Change (Shift+F7)"), this);
    next_ = iconButton(":/ui/icons/diff/arrow_down.a8", tr("Next Change (F7)"), this);
    connect(previous_, &QToolButton::clicked, this, &DiffToolbar::previousRequested);
    connect(next_, &QToolButton::clicked, this, &DiffToolbar::nextRequested);

    count_ = new QLabel(this);
    count_->setObjectName(QStringLiteral("diffCount"));

    // Combo entries are in the same order as the `Ffi*` enums they map to,
    // so an index *is* the enum value — one place to keep in step.
    viewer_ = new QComboBox(this);
    viewer_->addItems({tr("Side-by-side viewer"), tr("Unified viewer")});
    viewer_->setToolTip(tr("Viewer"));
    connect(viewer_, &QComboBox::currentIndexChanged, this, &DiffToolbar::viewerChanged);

    whitespace_ = new QComboBox(this);
    whitespace_->addItems({tr("Do not ignore"), tr("Trim whitespaces"), tr("Ignore whitespaces"),
                           tr("Ignore whitespaces and empty lines")});
    whitespace_->setToolTip(tr("Ignore whitespace"));
    connect(whitespace_, &QComboBox::currentIndexChanged, this, &DiffToolbar::optionsChanged);

    highlight_ = new QComboBox(this);
    highlight_->addItems(
      {tr("Highlight words"), tr("Highlight characters"), tr("Highlight lines"),
       tr("Do not highlight")});
    highlight_->setToolTip(tr("Highlighting mode"));
    connect(highlight_, &QComboBox::currentIndexChanged, this, &DiffToolbar::optionsChanged);

    collapse_ = iconButton(":/ui/icons/diff/collapse.a8", tr("Collapse unchanged fragments"), this);
    collapse_->setCheckable(true);
    collapse_->setChecked(true);
    connect(collapse_, &QToolButton::toggled, this, &DiffToolbar::optionsChanged);

    sync_ = iconButton(":/ui/icons/diff/sync.a8", tr("Synchronize scrolling"), this);
    sync_->setCheckable(true);
    sync_->setChecked(true);
    connect(sync_, &QToolButton::toggled, this, &DiffToolbar::optionsChanged);

    auto *layout = new QHBoxLayout(this);
    layout->setContentsMargins(tokens::kSp2, tokens::kSp1, tokens::kSp2, tokens::kSp1);
    layout->setSpacing(tokens::kSp1);
    layout->addWidget(previous_);
    layout->addWidget(next_);
    layout->addWidget(count_);
    layout->addSpacing(tokens::kSp3);
    layout->addWidget(viewer_);
    layout->addWidget(whitespace_);
    layout->addWidget(highlight_);
    layout->addSpacing(tokens::kSp2);
    layout->addWidget(collapse_);
    layout->addWidget(sync_);
    layout->addStretch(1);

    setDifferenceCount(0);
}

DiffToolbar::Viewer DiffToolbar::viewer() const
{
    return viewer_->currentIndex() == 1 ? Viewer::Unified : Viewer::SideBySide;
}

FfiWhitespaceMode DiffToolbar::whitespaceMode() const
{
    return static_cast<FfiWhitespaceMode>(whitespace_->currentIndex());
}

FfiHighlightMode DiffToolbar::highlightMode() const
{
    return static_cast<FfiHighlightMode>(highlight_->currentIndex());
}

bool DiffToolbar::collapseUnchanged() const
{
    return collapse_->isChecked();
}

bool DiffToolbar::syncScroll() const
{
    return sync_->isChecked();
}

void DiffToolbar::setDifferenceCount(int count)
{
    count_->setText(count == 0 ? tr("No differences")
                    : count == 1 ? tr("1 difference")
                                 : tr("%1 differences").arg(count));
    previous_->setEnabled(count > 0);
    next_->setEnabled(count > 0);
}

void DiffToolbar::showEvent(QShowEvent *event)
{
    QWidget::showEvent(event);
    // Deferred to the next event-loop turn rather than read right here:
    // when this diff opens into the Diff dock — nested many layouts deep
    // in the main window's dock tree, unlike a freshly resized standalone
    // window — the `LayoutRequest`s that place this row at its real
    // position are still queued at `showEvent` time, and `mapToGlobal`
    // below would report where the row *used to* sit. `QTimer::singleShot`
    // with a 0ms delay runs after those posted events drain, by which
    // point layout has settled.
    QTimer::singleShot(0, this, [this] {
        e2eMark(QStringLiteral("{\"ev\":\"diff_toolbar_shown\",\"previous_rect\":%1,"
                               "\"next_rect\":%2,\"viewer_rect\":%3,\"whitespace_rect\":%4,"
                               "\"highlight_rect\":%5,\"collapse_rect\":%6,\"sync_rect\":%7}")
                  .arg(rectJson(previous_), rectJson(next_), rectJson(viewer_),
                       rectJson(whitespace_), rectJson(highlight_), rectJson(collapse_),
                       rectJson(sync_)));
    });
}

} // namespace ui_shell
