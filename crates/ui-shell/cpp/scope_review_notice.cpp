#include "scope_review_notice.h"

#include "e2e_mark.h"
#include "ui_tokens.h"
#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QCheckBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QEvent>
#include <QHBoxLayout>
#include <QLabel>
#include <QListWidget>
#include <QListWidgetItem>
#include <QMessageBox>
#include <QPushButton>
#include <QPoint>
#include <QRect>
#include <QResizeEvent>
#include <QScrollArea>
#include <QStringList>
#include <QTimer>
#include <QToolButton>
#include <QVBoxLayout>

namespace ui_shell {

namespace {

constexpr int kMargin = tokens::kPanelGap;
// The dialog lists one row per candidate; this caps it so a workspace with
// many gitignored folders gets a scrollbar instead of a dialog taller than
// the screen — the same reason `project_scope_settings_page.cpp` caps its
// own lists.
constexpr int kListMaxHeight = 220;

void warnOnFailure(QWidget *parent, const FfiResult &result)
{
    if (result.code != 0) {
        QMessageBox::warning(parent, QObject::tr("Project Scope"), result.message);
    }
}

// `[x,y,w,h]`, global coordinates — the same rect shape every other E2E
// marker in this codebase uses (`project_scope_settings_page.cpp`'s
// `rectOf`, `settings_dialog.cpp`'s `rectJson`).
QString rectJson(const QRect &rect)
{
    return QStringLiteral("[%1,%2,%3,%4]")
      .arg(rect.x())
      .arg(rect.y())
      .arg(rect.width())
      .arg(rect.height());
}

QRect globalRect(QWidget *widget)
{
    return QRect(widget->mapToGlobal(QPoint(0, 0)), widget->size());
}

// The review dialog: one checkbox row per candidate, pre-checked per
// `suggestExclude`. OK writes the answer through `commitScopeReview`;
// Cancel writes nothing, so the same candidates come back next project
// open — `ProjectTreeModel::scopeCandidates` is not cleared here, only by
// a successful commit or the next project open recomputing it.
class ScopeReviewDialog : public QDialog
{
public:
    ScopeReviewDialog(ProjectTreeModel *treeModel, QWidget *parent)
      : QDialog(parent)
      , treeModel_(treeModel)
    {
        setWindowTitle(tr("Review Ignored Folders"));

        auto *layout = new QVBoxLayout(this);
        auto *intro = new QLabel(
          tr("These folders are ignored by Git but not excluded from the project. "
             "Checked folders will be excluded; unchecked folders will not be offered again."),
          this);
        intro->setWordWrap(true);
        layout->addWidget(intro);

        list_ = new QListWidget(this);
        list_->setMaximumHeight(kListMaxHeight);
        for (const FfiScopeCandidate &candidate : treeModel_->scopeCandidates()) {
            auto *item = new QListWidgetItem(candidate.relative_path, list_);
            item->setFlags(item->flags() | Qt::ItemIsUserCheckable);
            item->setCheckState(candidate.suggest_exclude ? Qt::Checked : Qt::Unchecked);
        }
        layout->addWidget(list_);

        auto *buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel, this);
        layout->addWidget(buttons);
        connect(buttons, &QDialogButtonBox::accepted, this, &ScopeReviewDialog::commit);
        connect(buttons, &QDialogButtonBox::rejected, this, &QDialog::reject);

        // `dialog_shown`/`dialog_closed`, the same convention
        // `search_everywhere_dialog.cpp`/`settings_dialog.cpp` mark
        // themselves with. Modal `exec()` blocks the caller, so the rects
        // (one per row, `Qt::Checked`/`Qt::Unchecked` folded in so a flow
        // can assert the pre-checked state without a second read) have to
        // fire from a zero-delay timer once the modal loop is actually
        // running — `settings_dialog.cpp`'s own comment on why `exec()`
        // itself is too early.
        QPushButton *okButton = buttons->button(QDialogButtonBox::Ok);
        QListWidget *list = list_;
        QTimer::singleShot(0, this, [this, list, okButton]() {
            QStringList rows;
            for (int i = 0; i < list->count(); ++i) {
                QListWidgetItem *item = list->item(i);
                const QRect itemRect = list->visualItemRect(item);
                const QPoint origin = list->viewport()->mapToGlobal(itemRect.topLeft());
                rows << QStringLiteral("{\"path\":%1,\"checked\":%2,\"rect\":%3}")
                          .arg(e2eJson(item->text()))
                          .arg(item->checkState() == Qt::Checked ? QLatin1String("true")
                                                                  : QLatin1String("false"))
                          .arg(rectJson(QRect(origin, itemRect.size())));
            }
            e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"scope_review_dialog\","
                                    "\"ok_rect\":%1,\"rows\":[%2]}")
                      .arg(rectJson(globalRect(okButton)), rows.join(QLatin1Char(','))));
        });
    }

    void done(int result) override
    {
        QDialog::done(result);
        e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"scope_review_dialog\","
                                "\"accepted\":%1}")
                  .arg(result == QDialog::Accepted ? QLatin1String("true") : QLatin1String("false")));
    }

private:
    void commit()
    {
        QStringList checked;
        QStringList unchecked;
        for (int i = 0; i < list_->count(); ++i) {
            QListWidgetItem *item = list_->item(i);
            (item->checkState() == Qt::Checked ? checked : unchecked) << item->text();
        }
        warnOnFailure(this, treeModel_->commitScopeReview(checked, unchecked));
        accept();
    }

    ProjectTreeModel *treeModel_;
    QListWidget *list_ = nullptr;
};

} // namespace

// The notice bar itself: a small opaque panel floated over the bottom-right
// of `parent`, the same "paints its own background, repositions on the
// parent's resize" shape `FindBar` uses for its own floating chrome.
class ScopeNoticeBar : public QWidget
{
public:
    ScopeNoticeBar(ProjectTreeModel *treeModel, QWidget *parent)
      : QWidget(parent)
      , treeModel_(treeModel)
    {
        setObjectName(QStringLiteral("scopeNoticeBar"));
        setAutoFillBackground(true);
        setAttribute(Qt::WA_StyledBackground, true);
        setStyleSheet(QStringLiteral("#scopeNoticeBar { background-color: palette(window);"
                                     " border: 1px solid palette(mid); border-radius: %1px; }")
                        .arg(tokens::kRadiusPanel));

        label_ = new QLabel(this);
        reviewButton_ = new QPushButton(tr("Review…"), this);
        closeButton_ = new QToolButton(this);
        closeButton_->setText(QStringLiteral("✕"));
        closeButton_->setToolTip(tr("Dismiss"));
        closeButton_->setAutoRaise(true);

        auto *layout = new QHBoxLayout(this);
        layout->setContentsMargins(tokens::kSp3, tokens::kSp2, tokens::kSp2, tokens::kSp2);
        layout->addWidget(label_);
        layout->addWidget(reviewButton_);
        layout->addWidget(closeButton_);

        connect(reviewButton_, &QPushButton::clicked, this, &ScopeNoticeBar::review);
        connect(closeButton_, &QToolButton::clicked, this, &ScopeNoticeBar::hide);

        parentWidget()->installEventFilter(this);
        hide();
    }

    void announce(quint32 count)
    {
        label_->setText(tr("Found %n ignored but not excluded folder(s)", nullptr, int(count)));
        reposition();
        show();
        raise();
        e2eMark(QStringLiteral("{\"ev\":\"scope_notice_shown\",\"count\":%1,\"review_rect\":%2}")
                  .arg(count)
                  .arg(rectJson(globalRect(reviewButton_))));
    }

protected:
    bool eventFilter(QObject *watched, QEvent *event) override
    {
        if (watched == parentWidget() && event->type() == QEvent::Resize && isVisible()) {
            reposition();
        }
        return QWidget::eventFilter(watched, event);
    }

private:
    void review()
    {
        ScopeReviewDialog dialog(treeModel_, window());
        dialog.exec();
        // Answered or cancelled, this offer is done for the session either
        // way — a cancelled candidate list simply reappears on the next
        // project open, not by leaving this bar up.
        hide();
    }

    void reposition()
    {
        adjustSize();
        move(parentWidget()->width() - width() - kMargin,
             parentWidget()->height() - height() - kMargin);
    }

    ProjectTreeModel *treeModel_;
    QLabel *label_ = nullptr;
    QPushButton *reviewButton_ = nullptr;
    QToolButton *closeButton_ = nullptr;
};

void installScopeReviewNotice(QWidget *parent, ProjectTreeModel *treeModel)
{
    auto *bar = new ScopeNoticeBar(treeModel, parent);
    QObject::connect(treeModel, &ProjectTreeModel::scopeCandidatesFound, bar,
                     [bar](quint32 count) { bar->announce(count); });
}

} // namespace ui_shell
