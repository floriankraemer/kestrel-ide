#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// Every dialog F5b's Database dock / results toolbar entry points open —
// one file rather than one class per dialog (`db_export_dialog.*`, …):
// each is a small, single-use `QDialog` built ad hoc from a handful of
// widgets, and splitting them into their own translation units would
// mean six headers for six functions that share nothing but the
// `ExchangeService` they all call into. Humble view per CLAUDE.md: every
// choice (which formats support which options, whether a mapping is
// valid, dump-tool availability, compare results) is read from
// `ExchangeService`; these functions only lay out widgets and forward
// button clicks.

// Table/view row's "Export data…" (`db_core::tree::ActionSet::EXPORT_DATA`).
void showExportDataDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId,
                          const QString &objectPath);

// Results grid toolbar's "Export…"/"Copy as ▸" — the rows are already
// fetched, so this never touches a session. `rows` is `ResultProvider::
// rowValues`'s own typed JSON-per-row encoding (F4c), not rendered text —
// `ExchangeService` decodes it back into real `Value`s before writing.
void showExportRowsDialog(QWidget *parent, ExchangeService *exchange, const QStringList &columns,
                          const QStringList &rows);

// Table row's "Import data…" (`ActionSet::IMPORT_DATA`).
void showImportDataDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId,
                          const QString &targetTable);

// Table row's "Copy table to…" (`ActionSet::COPY_TABLE`).
void showCopyTableDialog(QWidget *parent, ExchangeService *exchange, const QString &srcSourceId,
                         const QString &srcTable, const ::rust::Vec<FfiDbSourceRow> &sources);

// Data source row's "Dump…".
void showDumpDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId);

// Data source row's "Restore…" (F6c): picks a dump file, previews the
// restore tool's argv, and confirms before writing into this source.
void showRestoreDialog(QWidget *parent, ExchangeService *exchange, const QString &sourceId);

// Table row's "ER Diagram" and the data source row's "ER Diagram" (whole
// schema) — `tableScope` empty means the whole schema.
void showErDiagramDialog(QWidget *parent, ExchangeService *exchange, DocumentManager *documentManager,
                         const QString &sourceId, const QString &tableScope);

// Data source row's "Compare structure with…".
void showSchemaCompareDialog(QWidget *parent, ExchangeService *exchange,
                             DocumentManager *documentManager, const QString &leftSourceId,
                             const ::rust::Vec<FfiDbSourceRow> &sources);

// Table row's "Compare data with…".
void showDataCompareDialog(QWidget *parent, ExchangeService *exchange,
                           DocumentManager *documentManager, const QString &leftSourceId,
                           const QString &leftTable, const ::rust::Vec<FfiDbSourceRow> &sources);

} // namespace ui_shell
