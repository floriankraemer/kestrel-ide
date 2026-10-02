#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QDialog>
#include <QHash>
#include <QString>

#include <functional>

class QAction;
class QLineEdit;
class QListWidget;
class QListWidgetItem;
class QTabBar;
class QTimer;

class SearchResultsPanel;

// JetBrains-style Search Everywhere: one popup over every search tier
// (recent files, actions, file names, symbols, full text), with tabs to
// narrow to a single tier.
//
// Humble view per CLAUDE.md's hard rule — every tier is searched and ranked
// in Rust and arrives as tier-tagged `FfiSearchHit` rows; this debounces
// typing, drops results from superseded queries, renders the rows, and
// activates whatever a row points at.
class SearchEverywhereDialog : public QDialog
{
    Q_OBJECT
public:
    // Which tier the popup opens filtered to. `All` shows every tier.
    // Declared in the same order as `FfiTierFilter`, which it maps onto.
    enum class Tier { All, Classes, Files, Symbols, Text, Actions };

    using OpenAt = std::function<void(const QString &, int, int)>;

    SearchEverywhereDialog(SearchModel *searchModel,
                           OpenAt openAt,
                           SearchResultsPanel *resultsPanel,
                           QWidget *parent);

    // The action registry the Actions tier triggers through. Set after
    // construction because the menus that fill it are built later.
    void setActions(const QHash<QString, QAction *> *actions) { actions_ = actions; }

    // Show the popup, cleared and focused, filtered to `tier`.
    void popup(Tier tier);

protected:
    void keyPressEvent(QKeyEvent *event) override;
    // Marker-stream reporting only (e2e_mark.h); the base behaviour is
    // unchanged.
    void done(int result) override;

private:
    void scheduleQuery();
    void runQuery();
    void appendHits(quint64 generation, const ::rust::Vec<FfiSearchHit> &hits);
    void activate(QListWidgetItem *item);
    void handoffToResultsPanel();
    FfiTierFilter tierFilter() const;
    bool tierIsVisible(FfiHitKind kind) const;
    QString sectionTitle(FfiHitKind kind) const;

    SearchModel *searchModel_;
    OpenAt openAt_;
    const QHash<QString, QAction *> *actions_ = nullptr;
    SearchResultsPanel *resultsPanel_;
    QLineEdit *queryEdit_ = nullptr;
    QTabBar *tabs_ = nullptr;
    QListWidget *results_ = nullptr;
    QTimer *debounce_ = nullptr;
    quint64 generation_ = 0;
    // Section header already emitted for a tier in the current query, so a
    // second batch from the same tier doesn't repeat it.
    int lastSection_ = -1;
};
