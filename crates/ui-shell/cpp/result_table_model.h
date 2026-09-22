#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QAbstractTableModel>
#include <QVector>

namespace ui_shell {

// The first `QAbstractTableModel` in this tree (database-tools-plan
// F3.4): one result's rows, paged through `ResultProvider::rowPage` on
// demand via `canFetchMore`/`fetchMore` — a page landing in Rust
// (`rowsAppended`) only grows this model's row count, it never re-reads
// rows already fetched.
//
// Humble view: NULL rendering, primary-key marking, and what a cell's
// text says are `ResultProvider`'s/`db_core::value::Value::display`'s
// answers; this model only turns `FfiDbRow`'s `\u{1f}`-joined `cells`/
// `nulls` strings into per-cell `QVariant`s.
class ResultTableModel : public QAbstractTableModel
{
public:
    explicit ResultTableModel(ResultProvider *provider, QObject *parent = nullptr);

    // Switches this model to a fresh execution's result id, resetting
    // every cached page — called once per `executionStarted`.
    void setResultId(quint64 resultId);
    quint64 resultId() const { return resultId_; }

    // `rowsAppended`'s handler: the provider's own `rowCount` grew, so
    // this model tells its view about the new rows without touching what
    // is already on screen.
    void rowsAppended(quint64 first, quint64 count);

    int rowCount(const QModelIndex &parent = QModelIndex()) const override;
    int columnCount(const QModelIndex &parent = QModelIndex()) const override;
    QVariant data(const QModelIndex &index, int role = Qt::DisplayRole) const override;
    bool setData(const QModelIndex &index, const QVariant &value, int role = Qt::EditRole) override;
    Qt::ItemFlags flags(const QModelIndex &index) const override;
    QVariant headerData(int section, Qt::Orientation orientation,
                        int role = Qt::DisplayRole) const override;
    bool canFetchMore(const QModelIndex &parent) const override;
    void fetchMore(const QModelIndex &parent) override;

    // Re-reads the row count and drops every cached page — called after
    // addRow/cloneRow/deleteRows/revert, all of which change the buffer's
    // own row shape server-side in ways a single cached page cannot track
    // incrementally.
    void refreshRows();

    // Re-reads `provider_->isEditable`/`notEditableReason` — called once a
    // fresh result's editability decision lands (`ConsoleService::
    // editabilityChanged`), since that answer is not known yet when
    // `setResultId` first runs (F4.1's own async `Full`-level introspect).
    void refreshEditability();
    bool isEditable() const { return editable_; }
    QString notEditableReason() const { return notEditableReason_; }

    // The column name at `index` — `ValueEditorDialog`'s own lookup, so it
    // never has to re-derive column order itself.
    QString columnNameAt(int column) const;

private:
    struct Page
    {
        quint64 first;
        ::rust::Vec<FfiDbRow> rows;
    };

    const Page *pageFor(int row) const;
    void ensurePage(int row) const;

    ResultProvider *provider_;
    quint64 resultId_ = 0;
    quint64 rowCount_ = 0;
    ::rust::Vec<FfiDbColumn> columns_;
    // An LRU-free cache: pages fetched so far, in fetch order. A result
    // grid scrolls mostly forward, so evicting the oldest page once the
    // cache grows past a handful keeps memory bounded without needing a
    // real LRU (`ponytail`: revisit if random-access scrolling on a huge
    // result ever thrashes this).
    mutable QVector<Page> pages_;
    static constexpr int kPageSize = 200;
    static constexpr int kMaxCachedPages = 50;

    bool editable_ = false;
    QString notEditableReason_;
};

} // namespace ui_shell
