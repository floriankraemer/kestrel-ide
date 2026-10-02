//! The Editing settings page's row <-> `app_config::editing::EditingSettings`
//! conversions, split out of `settings.rs` (file-size gate).

use std::collections::HashMap;

use cxx_qt_lib::QString;

use crate::bridge::ffi::FfiEditingRow;

pub(crate) fn to_ffi_editing_row(
    language_id: &str,
    language_name: &str,
    settings: &app_config::editing::EditingSettings,
) -> FfiEditingRow {
    FfiEditingRow {
        language_id: QString::from(language_id),
        language_name: QString::from(language_name),
        tab_width: settings.tab_width,
        has_use_spaces: settings.use_spaces.is_some(),
        use_spaces: settings.use_spaces.unwrap_or(false),
        has_trim_trailing_whitespace: settings.trim_trailing_whitespace.is_some(),
        trim_trailing_whitespace: settings.trim_trailing_whitespace.unwrap_or(false),
        has_insert_final_newline: settings.insert_final_newline.is_some(),
        insert_final_newline: settings.insert_final_newline.unwrap_or(false),
        has_format_on_save: settings.format_on_save.is_some(),
        format_on_save: settings.format_on_save.unwrap_or(false),
        has_wrap_column: settings.wrap_column.is_some(),
        wrap_column: settings.wrap_column.unwrap_or(0),
        has_soft_wrap: settings.soft_wrap.is_some(),
        soft_wrap: settings.soft_wrap.unwrap_or(false),
        default_encoding: QString::from(settings.default_encoding.as_str()),
        line_endings: QString::from(settings.line_endings.as_str()),
    }
}

pub(crate) fn from_ffi_editing_row(row: &FfiEditingRow) -> app_config::editing::EditingSettings {
    app_config::editing::EditingSettings {
        tab_width: row.tab_width,
        use_spaces: row.has_use_spaces.then_some(row.use_spaces),
        trim_trailing_whitespace: row
            .has_trim_trailing_whitespace
            .then_some(row.trim_trailing_whitespace),
        insert_final_newline: row
            .has_insert_final_newline
            .then_some(row.insert_final_newline),
        format_on_save: row.has_format_on_save.then_some(row.format_on_save),
        wrap_column: row.has_wrap_column.then_some(row.wrap_column),
        soft_wrap: row.has_soft_wrap.then_some(row.soft_wrap),
        default_encoding: row.default_encoding.to_string(),
        line_endings: row.line_endings.to_string(),
        languages: HashMap::new(),
    }
}
