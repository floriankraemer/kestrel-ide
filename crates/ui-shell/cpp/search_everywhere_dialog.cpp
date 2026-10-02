#include "search_everywhere_dialog.h"

#include "e2e_mark.h"
#include "highlight_delegate.h"
#include "icon_cache.h"
#include "search_results_panel.h"

#include <QAction>
#include <QFileInfo>
#include <QFont>
#include <QKeyEvent>
#include <QLineEdit>
#include <QListWidget>
#include <QTabBar>
#include <QTimer>
#include <QVBoxLayout>
#include <QVariantList>

namespace {

// Item data roles on a result row.
constexpr int kPathRole = Qt::UserRole;
constexpr int kLineRole = Qt::UserRole + 1;
constexpr int kActionRole = Qt::UserRole + 2;

// How long typing has to settle before a query goes out. Short enough to feel
// instant, long enough that a fast typist sends one query rather than ten.
constexpr int kDebounceMs = 60;

// Rows requested per tier. The popup is a top-hits list, not an exhaustive
// results view — that is what the Search Results dock is for.
constexpr quint32 kResultLimit = 30;

} // namespace

SearchEverywhereDialog::SearchEverywhereDialog(SearchModel *searchModel,
                                               OpenAt openAt,
                                               SearchResultsPanel *resultsPanel,
                                               QWidget *parent)
  : QDialog(parent)
  , searchModel_(searchModel)
  , openAt_(std::move(openAt))
  , resultsPanel_(resultsPanel)
{
    setWindowTitle(tr("Search Everywhere"));
    resize(720, 460);

    queryEdit_ = new QLineEdit(this);
    queryEdit_->setPlaceholderText(tr("Search for files, symbols, text or actions..."));
    queryEdit_->setClearButtonEnabled(true);

    tabs_ = new QTabBar(this);
    tabs_->addTab(tr("All"));
    tabs_->addTab(tr("Classes"));
    tabs_->addTab(tr("Files"));
    tabs_->addTab(tr("Symbols"));
    tabs_->addTab(tr("Text"));
    tabs_->addTab(tr("Actions"));
    tabs_->setExpanding(false);
    tabs_->setDrawBase(false);

    results_ = new QListWidget(this);
    results_->setUniformItemSizes(true);
    results_->setItemDelegate(new HighlightDelegate(results_));

    auto *layout = new QVBoxLayout(this);
    layout->addWidget(queryEdit_);
    layout->addWidget(tabs_);
    layout->addWidget(results_, 1);

    debounce_ = new QTimer(this);
    debounce_->setSingleShot(true);
    debounce_->setInterval(kDebounceMs);
    connect(debounce_, &QTimer::timeout, this, &SearchEverywhereDialog::runQuery);

    connect(queryEdit_, &QLineEdit::textChanged, this, &SearchEverywhereDialog::scheduleQuery);
    connect(tabs_, &QTabBar::currentChanged, this, [this](int) { runQuery(); });
    connect(results_, &QListWidget::itemActivated, this, &SearchEverywhereDialog::activate);
    connect(results_, &QListWidget::itemClicked, this, &SearchEverywhereDialog::activate);

    connect(searchModel_, &SearchModel::resultsBatch, this, &SearchEverywhereDialog::appendHits);
    // One mark per query that actually completed — superseded generations
    // are filtered out here exactly as the batches are, which is what lets a
    // test count deliveries and prove stale results were dropped rather than
    // rendered.
    connect(searchModel_, &SearchModel::queryFinished, this, [this](quint64 generation) {
        if (generation != generation_) {
            return;
        }
        e2eMark(QStringLiteral("{\"ev\":\"search_results\",\"count\":%1}")
                  .arg(results_->count()));
    });
    // Opening a folder starts an index build that outlasts the user's
    // patience, so a query typed in that window comes back "still being
    // built". Re-run it once the index lands instead of leaving that notice
    // on screen until the user types again.
    connect(searchModel_, &SearchModel::indexReady, this, [this]() {
        if (isVisible()) {
            runQuery();
        }
    });
    connect(searchModel_,
            &SearchModel::queryFailed,
            this,
            [this](quint64 generation, const QString &message) {
                if (generation != generation_) {
                    return;
                }
                results_->clear();
                auto *item = new QListWidgetItem(message, results_);
                item->setFlags(Qt::NoItemFlags);
            });
}

void SearchEverywhereDialog::popup(Tier tier)
{
    tabs_->setCurrentIndex(static_cast<int>(tier));
    queryEdit_->clear();
    results_->clear();
    show();
    raise();
    activateWindow();
    queryEdit_->setFocus();
    e2eMark(QStringLiteral("{\"ev\":\"dialog_shown\",\"name\":\"search_everywhere\"}"));
    // An empty query still has something to show: recent files and actions.
    runQuery();
}

void SearchEverywhereDialog::done(int result)
{
    QDialog::done(result);
    e2eMark(QStringLiteral("{\"ev\":\"dialog_closed\",\"name\":\"search_everywhere\","
                            "\"accepted\":%1}")
              .arg(result == QDialog::Accepted ? QLatin1String("true") : QLatin1String("false")));
}

void SearchEverywhereDialog::scheduleQuery()
{
    debounce_->start();
}

void SearchEverywhereDialog::runQuery()
{
    debounce_->stop();
    results_->clear();
    lastSection_ = -1;
    ++generation_;
    searchModel_->searchEverywhere(
      queryEdit_->text(), tierFilter(), generation_, kResultLimit);
}

// The active tab, as the tier filter the Rust side narrows on. The two
// enums are declared in the same order, so this is a straight cast.
FfiTierFilter SearchEverywhereDialog::tierFilter() const
{
    return static_cast<FfiTierFilter>(tabs_->currentIndex());
}

bool SearchEverywhereDialog::tierIsVisible(FfiHitKind kind) const
{
    switch (static_cast<Tier>(tabs_->currentIndex())) {
    case Tier::All:
        return true;
    case Tier::Classes:
        return kind == FfiHitKind::Symbol;
    case Tier::Files:
        return kind == FfiHitKind::File || kind == FfiHitKind::RecentFile;
    case Tier::Symbols:
        return kind == FfiHitKind::Symbol;
    case Tier::Text:
        return kind == FfiHitKind::Text;
    case Tier::Actions:
        return kind == FfiHitKind::Action;
    }
    return true;
}

QString SearchEverywhereDialog::sectionTitle(FfiHitKind kind)
{
    switch (kind) {
    case FfiHitKind::RecentFile:
        return tr("Recent Files");
    case FfiHitKind::File:
        return tr("Files");
    case FfiHitKind::Symbol:
        return tr("Symbols");
    case FfiHitKind::Text:
        return tr("Text");
    case FfiHitKind::Action:
        return tr("Actions");
    }
    return QString();
}

void SearchEverywhereDialog::appendHits(quint64 generation, const ::rust::Vec<FfiSearchHit> &hits)
{
    if (generation != generation_) {
        // A superseded query's results — dropping them is what keeps fast
        // typing from flickering older answers back into the list.
        return;
    }
    results_->setUpdatesEnabled(false);
    for (const FfiSearchHit &hit : hits) {
        if (!tierIsVisible(hit.kind)) {
            continue;
        }
        if (static_cast<int>(hit.kind) != lastSection_) {
            lastSection_ = static_cast<int>(hit.kind);
            auto *header = new QListWidgetItem(sectionTitle(hit.kind), results_);
            header->setFlags(Qt::NoItemFlags);
            QFont font = header->font();
            font.setBold(true);
            header->setFont(font);
        }

        const QString text = hit.text;
        const QString detail = hit.detail;
        auto *item = new QListWidgetItem(
          detail.isEmpty() ? text : tr("%1    %2").arg(text, detail), results_);
        item->setData(kPathRole, hit.path);
        // A null icon for an action row, which names no file — the Rust side
        // answers with no key at all rather than the pack's default.
        item->setIcon(ui_shell::fileIcon(QString(hit.path), ui_shell::smallIconPx(results_)));
        item->setData(kLineRole, hit.line);
        item->setData(kActionRole, hit.action_id);

        QVariantList positions;
        for (quint32 offset : hit.positions) {
            positions.append(static_cast<int>(offset));
        }
        item->setData(kMatchPositionsRole, positions);

        if (!results_->currentItem() || !(results_->currentItem()->flags() & Qt::ItemIsEnabled)) {
            results_->setCurrentItem(item);
        }
    }
    results_->setUpdatesEnabled(true);
}

void SearchEverywhereDialog::activate(QListWidgetItem *item)
{
    if (!item || !(item->flags() & Qt::ItemIsEnabled)) {
        return;
    }
    const QString actionId = item->data(kActionRole).toString();
    if (!actionId.isEmpty()) {
        QAction *action = actions_ ? actions_->value(actionId) : nullptr;
        accept();
        if (action) {
            action->trigger();
        }
        return;
    }
    const QString path = item->data(kPathRole).toString();
    if (path.isEmpty()) {
        return;
    }
    openAt_(path, item->data(kLineRole).toInt(), 0);
    accept();
}

void SearchEverywhereDialog::handoffToResultsPanel()
{
    if (!resultsPanel_ || queryEdit_->text().isEmpty()) {
        return;
    }
    resultsPanel_->searchFor(queryEdit_->text());
    accept();
}

void SearchEverywhereDialog::keyPressEvent(QKeyEvent *event)
{
    const bool isEnter = event->key() == Qt::Key_Return || event->key() == Qt::Key_Enter;
    if (isEnter && (event->modifiers() & Qt::ControlModifier)) {
        // Ctrl+Enter: stop looking at the top hits and put the whole result
        // set in the dock, where it can be worked through and replaced in.
        handoffToResultsPanel();
        return;
    }
    if (isEnter) {
        activate(results_->currentItem());
        return;
    }
    // Arrow keys drive the list while the caret stays in the query box, so
    // typing and choosing never need a focus change.
    if (event->key() == Qt::Key_Down || event->key() == Qt::Key_Up
        || event->key() == Qt::Key_PageDown || event->key() == Qt::Key_PageUp) {
        const int direction = (event->key() == Qt::Key_Down || event->key() == Qt::Key_PageDown)
                                ? 1
                                : -1;
        const int step = (event->key() == Qt::Key_PageDown || event->key() == Qt::Key_PageUp)
                           ? 10
                           : 1;
        int row = results_->currentRow();
        for (int taken = 0; taken < step; ++taken) {
            int next = row + direction;
            // Skip the non-selectable section headers.
            while (next >= 0 && next < results_->count()
                   && !(results_->item(next)->flags() & Qt::ItemIsEnabled)) {
                next += direction;
            }
            if (next < 0 || next >= results_->count()) {
                break;
            }
            row = next;
        }
        results_->setCurrentRow(row);
        return;
    }
    QDialog::keyPressEvent(event);
}
