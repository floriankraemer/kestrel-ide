//! The adapter's own refusals, as codes (ADR-0003 §4, range 1000–1099).
//!
//! Most failures crossing the seam belong to a Qt-free crate and arrive with
//! that crate's code already on them — `AppError`, `ChatError`, `VcsError`,
//! `RunError`, `SelectionError`. These are the ones that do not: "no project
//! is open", "no such console", "the settings file could not be written".
//! No domain crate has an opinion about them, because they are conditions of
//! *this* layer — a slot invoked when the state it needs is not there.
//!
//! They lived as twenty-five hand-written `code: 1` literals until ADR-0003
//! §4, which is to say they were indistinguishable from `AppError::NoSuchTab`
//! and from each other. Inventing a domain variant for each one instead would
//! have pushed view-shaped conditions into the domain to satisfy a numbering
//! scheme, which is the wrong direction: the adapter owns them, so the
//! adapter numbers them.
//!
//! Append-only within the range, like every other owner's block.

use cxx_qt_lib::QString;

use crate::bridge::ffi::FfiResult;

/// Success, when the result carries a message the view shows anyway — a
/// summary of what was attached, say. A success with nothing to say is
/// `FfiResult::default()`.
pub const CODE_OK: i32 = 0;

/// No project is open, so the slot has no root to work from.
pub const CODE_NO_PROJECT: i32 = 1000;
/// A run configuration id the adapter does not know.
pub const CODE_UNKNOWN_RUN_CONFIG: i32 = 1001;
/// A console id the adapter does not know.
pub const CODE_UNKNOWN_CONSOLE: i32 = 1002;
/// A run configuration with nothing to run.
pub const CODE_EMPTY_PROGRAM: i32 = 1003;
/// Reading or writing a settings file failed.
pub const CODE_SETTINGS_IO: i32 = 1004;
/// A terminal session id the adapter does not know, or a terminal operation
/// the transport refused.
pub const CODE_TERMINAL: i32 = 1005;
/// An attachment could not be read.
pub const CODE_ATTACHMENT_IO: i32 = 1006;
/// The operation was refused for a reason the message states in full, and
/// nothing branches on which one it was.
pub const CODE_REFUSED: i32 = 1007;

/// A before-launch task refused or failed: a cycle in the task graph, a task
/// naming a configuration that no longer exists, or a build that came back
/// non-zero (B2-2).
pub const CODE_BEFORE_LAUNCH: i32 = 1008;

/// A value the view passed across the seam is not usable — an empty host,
/// a port outside a port's range (D4-2). Distinct from a domain error:
/// nothing was attempted, because the request could not be formed.
pub const CODE_INVALID_ARGUMENT: i32 = 1009;

/// A named layout the adapter cannot find. Its own code rather than
/// `CODE_REFUSED` because the view does branch on it: a layout can disappear
/// between the menu being built and an entry being chosen (another window,
/// or a project closing under it), and that is a stale menu to rebuild, not
/// an error to put in front of the user.
pub const CODE_UNKNOWN_LAYOUT: i32 = 1010;

/// A failure with an adapter code and a finished sentence.
pub fn failure(code: i32, message: impl AsRef<str>) -> FfiResult {
    debug_assert!(
        (1000..=1099).contains(&code),
        "{code} is outside ui-shell's adapter range (ADR-0003 §4)"
    );
    FfiResult {
        code,
        message: QString::from(message.as_ref()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_adapter_code_is_unique_and_inside_the_range() {
        let codes = [
            CODE_NO_PROJECT,
            CODE_UNKNOWN_RUN_CONFIG,
            CODE_UNKNOWN_CONSOLE,
            CODE_EMPTY_PROGRAM,
            CODE_SETTINGS_IO,
            CODE_TERMINAL,
            CODE_ATTACHMENT_IO,
            CODE_REFUSED,
            CODE_BEFORE_LAUNCH,
        ];
        for code in codes {
            assert!(
                (1000..=1099).contains(&code),
                "{code} is outside ui-shell's 1000–1099 range (ADR-0003 §4)"
            );
        }
        let mut sorted = codes.to_vec();
        sorted.sort_unstable();
        let mut unique = sorted.clone();
        unique.dedup();
        assert_eq!(
            unique.len(),
            codes.len(),
            "two adapter refusals share a code: {sorted:?}"
        );
    }

    #[test]
    fn the_domain_ranges_do_not_overlap_the_adapters() {
        // The whole point of the ranges: a code says which failure it is
        // without the reader also knowing which QObject produced it. One
        // sample per owner is enough — each crate's own test covers the
        // rest of its block.
        let owners: [(&str, i32, i32, i32); 9] = [
            ("app-core", app_core::AppError::CODE_RESOURCE_OP, 1, 99),
            (
                "ai-chat-core",
                ai_chat_core::ChatError::CODE_CANCELLED,
                100,
                199,
            ),
            ("lsp-core", lsp_core::LspError::CODE_TIMEOUT, 600, 699),
            (
                "lsp-core",
                lsp_core::workspace_edit::EditError::CODE_MALFORMED,
                600,
                699,
            ),
            (
                "vcs-core",
                vcs_core::VcsError::CODE_GIT_NOT_INSTALLED,
                700,
                799,
            ),
            (
                "build-core",
                build_core::BuildError::CODE_NO_BUILDABLE_TOOLCHAIN,
                200,
                299,
            ),
            ("run-core", run_core::RunError::CODE_SPAWN, 800, 899),
            (
                "editor-core",
                editor_core::selection::SelectionError::CODE_TOO_MANY_CARETS,
                900,
                999,
            ),
            ("ui-shell", CODE_NO_PROJECT, 1000, 1099),
        ];
        for (owner, code, low, high) in owners {
            assert!(
                (low..=high).contains(&code),
                "{owner}'s {code} left its {low}–{high} range (ADR-0003 §4)"
            );
        }
    }

    #[test]
    fn the_codes_the_view_branches_on_match_the_crate_that_issues_them() {
        // `vcs_menu.cpp` acts on exactly these two — a force-delete offer
        // and a mark-as-safe offer — so their numbers are duplicated into
        // the bridge enum the view names. This is the gate that keeps the
        // duplicate honest.
        use crate::bridge::ffi::FfiVcsErrorCode;
        assert_eq!(
            i32::from(FfiVcsErrorCode::UnmergedBranch.repr),
            vcs_core::VcsError::CODE_UNMERGED_BRANCH
        );
        assert_eq!(
            i32::from(FfiVcsErrorCode::DubiousOwnership.repr),
            vcs_core::VcsError::CODE_DUBIOUS_OWNERSHIP
        );
    }
}
