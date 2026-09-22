#pragma once

#include "ui-shell/src/bridge/ffi.cxxqt.h"

#include <QString>

class QWidget;

namespace ui_shell {

// F4.4's create/modify object dialogs: Create/Modify Table, Add/Modify/
// Drop Column, Create Index, Create User. Every one builds a `db_core::
// ddl` spec as compact JSON, previews the Rust-generated DDL through
// `DatabaseService::objectDdlPreview` and asks for confirmation *before*
// ever calling `runObjectDdl` (`db_core::ddl`'s own "never run what the
// user has not seen" rule) — humble view per CLAUDE.md: nothing here
// decides what SQL to run, only what fields to collect.
//
// Every function returns `true` when the statement actually ran (so a
// caller can refresh the affected node), `false` on cancel or failure —
// the dialog itself has already shown the failure message either way.

// Create Table (`nodeId` is a schema/catalog/keyspace root, or a source's
// own root when it has no schema concept, e.g. SQLite).
bool showCreateTableDialog(QWidget *parent, DatabaseService *databaseService,
                           const QString &nodeId);

// Add Column (`nodeId` is an existing table).
bool showAddColumnDialog(QWidget *parent, DatabaseService *databaseService,
                         const QString &nodeId);

// Modify (alter) an existing column's type (`nodeId` is a table).
bool showModifyColumnDialog(QWidget *parent, DatabaseService *databaseService,
                            const QString &nodeId);

// Drop an existing column (`nodeId` is a table).
bool showDropColumnDialog(QWidget *parent, DatabaseService *databaseService,
                          const QString &nodeId);

// Create Index (`nodeId` is a table).
bool showCreateIndexDialog(QWidget *parent, DatabaseService *databaseService,
                           const QString &nodeId);

// Create User/Role (`nodeId` is a schema/catalog/keyspace root, or a
// source's own root).
bool showCreateUserDialog(QWidget *parent, DatabaseService *databaseService,
                         const QString &nodeId);

} // namespace ui_shell
