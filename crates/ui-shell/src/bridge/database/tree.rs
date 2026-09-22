//! Translation only: `db_core::tree::TreeRow` -> `ffi::FfiDbTreeRow`. Every
//! rule (grouping, filters, sort, which actions a row offers) lives in
//! `db_core::tree` — this module just crosses its answer over the FFI
//! seam, the same split `bridge::containers::service`'s `to_ffi_status`/
//! `to_ffi_age_unit` already draw for the Containers dock.

use cxx_qt_lib::QString;

use db_core::schema::ObjectKind;
use db_core::tree::{ActionSet, FolderKind, RowKind, TreeRow};

use crate::bridge::ffi::{FfiDbRowActions, FfiDbTreeRow};

/// A row's own kind, as the stable string the view keys its icon lookup
/// and context menu off — never a translated word (ADR-0049).
fn kind_id(kind: RowKind) -> &'static str {
    match kind {
        RowKind::Folder(folder) => match folder {
            FolderKind::Tables => "folder-tables",
            FolderKind::Views => "folder-views",
            FolderKind::MaterializedViews => "folder-materialized-views",
            FolderKind::Procedures => "folder-procedures",
            FolderKind::Functions => "folder-functions",
            FolderKind::Sequences => "folder-sequences",
            FolderKind::Types => "folder-types",
        },
        RowKind::Object(kind) => match kind {
            ObjectKind::Catalog => "catalog",
            ObjectKind::Schema => "schema",
            ObjectKind::Table => "table",
            ObjectKind::View => "view",
            ObjectKind::MaterializedView => "materialized-view",
            ObjectKind::Column => "column",
            ObjectKind::Index => "index",
            ObjectKind::Constraint => "constraint",
            ObjectKind::Trigger => "trigger",
            ObjectKind::Routine => "routine",
            ObjectKind::Sequence => "sequence",
            ObjectKind::Type => "type",
            ObjectKind::Role => "role",
            ObjectKind::User => "user",
            ObjectKind::Collection => "collection",
            ObjectKind::Field => "field",
            ObjectKind::Keyspace => "keyspace",
            ObjectKind::KeyNamespace => "key-namespace",
            ObjectKind::Key(_) => "key",
            ObjectKind::Group => "group",
        },
    }
}

fn to_ffi_actions(actions: ActionSet) -> FfiDbRowActions {
    FfiDbRowActions {
        can_open_console: actions.contains(ActionSet::OPEN_CONSOLE),
        can_edit_data: actions.contains(ActionSet::EDIT_DATA),
        can_go_to_ddl: actions.contains(ActionSet::GO_TO_DDL),
        can_copy_name: actions.contains(ActionSet::COPY_NAME),
        can_copy_qualified_name: actions.contains(ActionSet::COPY_QUALIFIED_NAME),
        can_refresh: actions.contains(ActionSet::REFRESH),
        can_rename: actions.contains(ActionSet::RENAME),
        can_drop: actions.contains(ActionSet::DROP),
        can_truncate: actions.contains(ActionSet::TRUNCATE),
        can_comment: actions.contains(ActionSet::COMMENT),
        can_generate_ddl: actions.contains(ActionSet::GENERATE_DDL),
        can_er_diagram: actions.contains(ActionSet::ER_DIAGRAM),
        can_export_data: actions.contains(ActionSet::EXPORT_DATA),
        can_import_data: actions.contains(ActionSet::IMPORT_DATA),
        can_copy_table: actions.contains(ActionSet::COPY_TABLE),
        // Source-level-only actions (F5b.3): never set from an object's
        // own `ActionSet` — see `service.rs::rows`'s own root row.
        can_dump: false,
        can_compare: false,
        can_delete_key: actions.contains(ActionSet::DELETE_KEY),
        can_ttl_set: actions.contains(ActionSet::TTL_SET),
        can_create_table: actions.contains(ActionSet::CREATE_TABLE),
        can_modify_table: actions.contains(ActionSet::MODIFY_TABLE),
        can_add_column: actions.contains(ActionSet::ADD_COLUMN),
        can_create_index: actions.contains(ActionSet::CREATE_INDEX),
        can_create_user: actions.contains(ActionSet::CREATE_USER),
    }
}

/// One row, prefixed with its own data source id so the dock's single
/// flat `rows()` list can hold every connected source's tree at once
/// without their node ids colliding — `"<source_id>:<node_id>"`, `:`
/// chosen because neither a source id nor `TreeRow::node_id`'s `/`-joined
/// path ever contains one.
pub fn to_ffi_row(source_id: &str, row: &TreeRow) -> FfiDbTreeRow {
    FfiDbTreeRow {
        source_id: QString::from(source_id),
        node_id: QString::from(format!("{source_id}:{}", row.node_id)),
        depth: row.depth as i32,
        kind: QString::from(kind_id(row.kind)),
        label: QString::from(row.label.as_str()),
        detail: QString::from(row.detail.as_str()),
        expandable: row.expandable,
        loaded: row.loaded,
        actions: to_ffi_actions(row.actions),
    }
}

/// The reverse of [`to_ffi_row`]'s id prefix: `(source_id, node_id)`, or
/// `None` for a malformed id (never produced by this bridge itself, but a
/// stale id from a tab or a completion request is still just data).
pub fn parse_node_id(id: &str) -> Option<(&str, &str)> {
    id.split_once(':')
}

#[cfg(test)]
mod tests {
    use super::*;
    use db_core::schema::{Node, ObjectKind};
    use db_core::tree::{flatten, FlattenOptions};

    #[test]
    fn to_ffi_row_prefixes_the_node_id_with_the_source_id() {
        let node = Node::leaf("users", ObjectKind::Table);
        let rows = flatten(&[node], &FlattenOptions::default());
        let users_row = rows.iter().find(|r| r.label == "users").unwrap();
        let ffi_row = to_ffi_row("ds-1", users_row);
        assert_eq!(ffi_row.node_id.to_string(), "ds-1:Tables/users");
        assert_eq!(ffi_row.source_id.to_string(), "ds-1");
        assert_eq!(ffi_row.kind.to_string(), "table");
    }

    #[test]
    fn to_ffi_row_carries_the_actions_bitset_as_discrete_bools() {
        let node = Node::leaf("users", ObjectKind::Table);
        let rows = flatten(&[node], &FlattenOptions::default());
        let users_row = rows.iter().find(|r| r.label == "users").unwrap();
        let ffi_row = to_ffi_row("ds-1", users_row);
        assert!(ffi_row.actions.can_go_to_ddl);
        assert!(ffi_row.actions.can_drop);
    }

    #[test]
    fn a_folder_row_carries_a_folder_kind_id() {
        let node = Node::leaf("users", ObjectKind::Table);
        let rows = flatten(&[node], &FlattenOptions::default());
        let folder_row = rows.iter().find(|r| r.label == "Tables").unwrap();
        let ffi_row = to_ffi_row("ds-1", folder_row);
        assert_eq!(ffi_row.kind.to_string(), "folder-tables");
        assert!(!ffi_row.actions.can_drop);
    }

    #[test]
    fn parse_node_id_splits_on_the_first_colon_only() {
        assert_eq!(
            parse_node_id("ds-1:Tables/users"),
            Some(("ds-1", "Tables/users"))
        );
        assert_eq!(parse_node_id("no-colon-here"), None);
    }
}
