//! What a `WorkspaceEdit` means, and how one is applied to a document's text.
//!
//! Every decision here is a rule, so none of it may live in `bridge.rs` or
//! `cpp/` (`docs/architecture/layering.md`): the payload has two legal
//! shapes that must not be merged, an edit carrying a file create/rename/
//! delete has to be refused rather than half-applied, and lowering a
//! protocol range (0-based lines, UTF-16 characters) onto a byte offset is
//! exactly the kind of conversion the view keeps getting wrong.
//!
//! Deliberately *not* expressed in terms of `index_core::FileReplacement`:
//! that type is a single-line span (`line` plus byte offsets within it), and
//! an LSP range routinely spans lines — which is what every extract-method
//! edit does. Whole text in, whole text out is the only honest shape here.

use serde_json::Value;

/// One `TextEdit`: a half-open range in protocol units (0-based lines,
/// UTF-16 characters) and the text that replaces it. An empty range is a
/// pure insertion, which is legal and common.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextEdit {
    pub start_line: u32,
    pub start_character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub new_text: String,
}

impl TextEdit {
    /// Document order, comparing starts. Used to sort descending before
    /// applying, so each edit still addresses the text it was computed
    /// against.
    fn start(&self) -> (u32, u32) {
        (self.start_line, self.start_character)
    }

    fn end(&self) -> (u32, u32) {
        (self.end_line, self.end_character)
    }
}

/// Every edit a `WorkspaceEdit` makes to one document, plus the version the
/// server believed that document was on (`None` when it did not say, which
/// the protocol allows and means "don't care").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocumentEdits {
    pub uri: String,
    pub path: String,
    pub version: Option<i32>,
    pub edits: Vec<TextEdit>,
}

impl DocumentEdits {
    /// See [`ResourceOp::retranslate`] — same rule, one field.
    fn retranslate(&mut self, host: &process_exec::host::ExecHost) {
        if host.is_remote() {
            if let Some(p) = crate::manager::path_for(host, &self.uri) {
                self.path = p;
            }
        }
    }
}

/// W3-2: retranslate every [`DocumentEdits::path`] in `edits` through `host`
/// — the free-function counterpart of [`WorkspaceChanges::retranslate_paths`]
/// for [`parse_workspace_edit`]'s callers (`rename`, code actions), which
/// have no resource operations to also fix up.
pub fn retranslate_document_edits(
    edits: &mut [DocumentEdits],
    host: &process_exec::host::ExecHost,
) {
    for edit in edits {
        edit.retranslate(host);
    }
}

/// Why an edit cannot be applied at all. Every variant refuses the *whole*
/// edit, never a part of it: a half-applied extract-method is a corrupted
/// file, so this mirrors `index_core::replace_in_files`' rule of validating
/// every span in a file before touching any of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditError {
    /// The edit creates, renames or deletes a file. We advertise
    /// `resourceOperations: []`, so a conforming server never sends one;
    /// supporting them means moving open tabs and invalidating `TabId`s,
    /// which is its own change.
    ResourceOperation(String),
    /// The payload was not a `WorkspaceEdit` we could read at all.
    Malformed,
    /// Two edits in one document overlap, so the result would depend on the
    /// order they were applied in. The specification forbids it; a server
    /// that does it anyway is not obeyed.
    OverlappingEdits,
    /// A range names a line or character that is not in the document — the
    /// buffer moved under the server, or the server miscounted.
    RangeOutOfBounds,
    /// The document is not the one the edit was computed against.
    StaleVersion { uri: String },
}

impl EditError {
    pub const CODE_RESOURCE_OPERATION: i32 = 620;
    pub const CODE_MALFORMED: i32 = 621;
    pub const CODE_OVERLAPPING_EDITS: i32 = 622;
    pub const CODE_RANGE_OUT_OF_BOUNDS: i32 = 623;
    pub const CODE_STALE_VERSION: i32 = 624;

    /// The variant's stable numeric code (ADR-0003 §4). Shares `lsp-core`'s
    /// 600–699 range with [`crate::manager::LspError`], which holds 600–607;
    /// these start at 620 so the two can each grow without meeting.
    pub fn code(&self) -> i32 {
        match self {
            EditError::ResourceOperation(_) => Self::CODE_RESOURCE_OPERATION,
            EditError::Malformed => Self::CODE_MALFORMED,
            EditError::OverlappingEdits => Self::CODE_OVERLAPPING_EDITS,
            EditError::RangeOutOfBounds => Self::CODE_RANGE_OUT_OF_BOUNDS,
            EditError::StaleVersion { .. } => Self::CODE_STALE_VERSION,
        }
    }
}

impl std::fmt::Display for EditError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            EditError::ResourceOperation(kind) => write!(
                f,
                "this refactoring wants to {kind} a file, which is not supported yet"
            ),
            EditError::Malformed => write!(f, "the language server sent an edit we cannot read"),
            EditError::OverlappingEdits => {
                write!(f, "the language server sent overlapping edits")
            }
            EditError::RangeOutOfBounds => write!(
                f,
                "the edit does not fit the file — it changed after the request was made"
            ),
            EditError::StaleVersion { uri } => {
                write!(f, "{uri} changed after the request was made")
            }
        }
    }
}

impl std::error::Error for EditError {}

/// Parse a `WorkspaceEdit`.
///
/// `documentChanges` wins outright when present and the two are never merged:
/// the specification says a client advertising `documentChanges` support gets
/// that field, and a server that fills both fills them with the same edits.
/// Legacy `changes` is still read, for servers that ignore the capability.
///
/// Documents are returned in a stable order — `documentChanges` order as the
/// server sent it, `changes` sorted by URI, since a JSON object has none.
/// A file the server wants created, renamed or deleted as part of an edit.
///
/// These arrive interleaved with text edits inside `documentChanges`, and the
/// protocol is explicit that the array is applied **in order**. That ordering
/// is load-bearing: a server that renames a file to match a renamed type
/// sends the text edit first and the rename second, and performing the rename
/// early would leave the edit addressing a path that no longer exists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceOp {
    Create {
        uri: String,
        path: String,
        /// Replace an existing file. `overwrite` wins over `ignore_if_exists`
        /// when a server sets both, which the specification requires.
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        old_uri: String,
        old_path: String,
        new_uri: String,
        new_path: String,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        uri: String,
        path: String,
        /// Delete a directory and its contents. Refused unless the server
        /// asks for it, so a stray delete cannot take a tree with it.
        recursive: bool,
        ignore_if_not_exists: bool,
    },
}

impl ResourceOp {
    /// Every path this operation touches, for the confinement check the
    /// caller performs before anything is applied.
    pub fn paths(&self) -> Vec<&str> {
        match self {
            ResourceOp::Create { path, .. } | ResourceOp::Delete { path, .. } => vec![path],
            ResourceOp::Rename {
                old_path, new_path, ..
            } => vec![old_path, new_path],
        }
    }

    /// W3-2: `path`/`old_path`/`new_path` were computed by [`resource_op`]
    /// with the plain (host-unaware) `path_from_uri` — correct on
    /// `ExecHost::Local`, a bare Linux path on a WSL root. Recomputes them
    /// from `uri`/`old_uri`/`new_uri` through `host` instead. A no-op on
    /// `ExecHost::Local`.
    fn retranslate(&mut self, host: &process_exec::host::ExecHost) {
        if !host.is_remote() {
            return;
        }
        match self {
            ResourceOp::Create { path, uri, .. } | ResourceOp::Delete { path, uri, .. } => {
                if let Some(p) = crate::manager::path_for(host, uri) {
                    *path = p;
                }
            }
            ResourceOp::Rename {
                old_path,
                old_uri,
                new_path,
                new_uri,
                ..
            } => {
                if let Some(p) = crate::manager::path_for(host, old_uri) {
                    *old_path = p;
                }
                if let Some(p) = crate::manager::path_for(host, new_uri) {
                    *new_path = p;
                }
            }
        }
    }
}

/// One step of a `WorkspaceEdit`, in the order the server sent it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeStep {
    Op(ResourceOp),
    Edits(DocumentEdits),
}

/// A `WorkspaceEdit` that may create, rename or delete files as well as edit
/// them, with the server's ordering preserved.
///
/// [`parse_workspace_edit`] remains the text-only reading and still refuses
/// resource operations, because its callers apply edits directly to open
/// buffers and have nowhere to put a file rename. New callers that can
/// perform file operations use this instead.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WorkspaceChanges {
    pub steps: Vec<ChangeStep>,
}

impl WorkspaceChanges {
    /// The document edits, in order, ignoring resource operations.
    pub fn documents(&self) -> impl Iterator<Item = &DocumentEdits> {
        self.steps.iter().filter_map(|s| match s {
            ChangeStep::Edits(e) => Some(e),
            ChangeStep::Op(_) => None,
        })
    }

    /// The resource operations, in order.
    pub fn operations(&self) -> impl Iterator<Item = &ResourceOp> {
        self.steps.iter().filter_map(|s| match s {
            ChangeStep::Op(op) => Some(op),
            ChangeStep::Edits(_) => None,
        })
    }

    pub fn has_operations(&self) -> bool {
        self.operations().next().is_some()
    }

    /// W3-2: retranslate every step's path(s) through `host` — called once,
    /// by `LspManager::parse_workspace_changes`, right after parsing. A
    /// no-op on `ExecHost::Local`.
    pub fn retranslate_paths(&mut self, host: &process_exec::host::ExecHost) {
        for step in &mut self.steps {
            match step {
                ChangeStep::Edits(edits) => edits.retranslate(host),
                ChangeStep::Op(op) => op.retranslate(host),
            }
        }
    }
}

/// Read a `WorkspaceEdit`, keeping resource operations rather than refusing
/// them.
///
/// Order is preserved exactly as sent. The specification says the array is
/// applied in order, and real refactorings depend on it — "rename the type
/// and rename its file to match" is a text edit followed by a rename, and
/// reordering those breaks it.
///
/// The legacy `changes` map has no ordering and cannot express resource
/// operations at all, so it reads exactly as it does today.
pub fn parse_workspace_changes(value: &Value) -> Result<WorkspaceChanges, EditError> {
    if value.is_null() {
        return Ok(WorkspaceChanges::default());
    }
    let object = value.as_object().ok_or(EditError::Malformed)?;

    if let Some(changes) = object.get("documentChanges") {
        let items = changes.as_array().ok_or(EditError::Malformed)?;
        let mut steps = Vec::with_capacity(items.len());
        for item in items {
            match item.get("kind").and_then(Value::as_str) {
                Some(kind) => steps.push(ChangeStep::Op(
                    resource_op(kind, item).ok_or(EditError::Malformed)?,
                )),
                None => steps.push(ChangeStep::Edits(
                    document_edits(item).ok_or(EditError::Malformed)?,
                )),
            }
        }
        return Ok(WorkspaceChanges { steps });
    }

    Ok(WorkspaceChanges {
        steps: parse_workspace_edit(value)?
            .into_iter()
            .map(ChangeStep::Edits)
            .collect(),
    })
}

fn resource_op(kind: &str, item: &Value) -> Option<ResourceOp> {
    let options = item.get("options");
    let flag = |name: &str| {
        options
            .and_then(|o| o.get(name))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    };
    let to_path =
        |uri: &str| crate::diagnostics::path_from_uri(uri).unwrap_or_else(|| uri.to_string());
    match kind {
        "create" => {
            let uri = item.get("uri")?.as_str()?.to_string();
            Some(ResourceOp::Create {
                path: to_path(&uri),
                uri,
                overwrite: flag("overwrite"),
                ignore_if_exists: flag("ignoreIfExists"),
            })
        }
        "rename" => {
            let old_uri = item.get("oldUri")?.as_str()?.to_string();
            let new_uri = item.get("newUri")?.as_str()?.to_string();
            Some(ResourceOp::Rename {
                old_path: to_path(&old_uri),
                new_path: to_path(&new_uri),
                old_uri,
                new_uri,
                overwrite: flag("overwrite"),
                ignore_if_exists: flag("ignoreIfExists"),
            })
        }
        "delete" => {
            let uri = item.get("uri")?.as_str()?.to_string();
            Some(ResourceOp::Delete {
                path: to_path(&uri),
                uri,
                recursive: flag("recursive"),
                ignore_if_not_exists: flag("ignoreIfNotExists"),
            })
        }
        _ => None,
    }
}

pub fn parse_workspace_edit(value: &Value) -> Result<Vec<DocumentEdits>, EditError> {
    if value.is_null() {
        return Ok(Vec::new());
    }
    let object = value.as_object().ok_or(EditError::Malformed)?;

    if let Some(changes) = object.get("documentChanges") {
        let items = changes.as_array().ok_or(EditError::Malformed)?;
        let mut out = Vec::with_capacity(items.len());
        for item in items {
            // A resource operation is tagged by `kind`; a plain
            // `TextDocumentEdit` has no `kind` at all.
            if let Some(kind) = item.get("kind").and_then(Value::as_str) {
                return Err(EditError::ResourceOperation(kind.to_string()));
            }
            out.push(document_edits(item).ok_or(EditError::Malformed)?);
        }
        return Ok(out);
    }

    let Some(changes) = object.get("changes").and_then(Value::as_object) else {
        // A `WorkspaceEdit` with neither field is an empty edit, not an
        // error: some servers answer a rename that changes nothing this way.
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(changes.len());
    for (uri, edits) in changes {
        out.push(DocumentEdits {
            uri: uri.clone(),
            path: crate::diagnostics::path_from_uri(uri).unwrap_or_else(|| uri.clone()),
            version: None,
            edits: text_edits(edits).ok_or(EditError::Malformed)?,
        });
    }
    out.sort_by(|a, b| a.uri.cmp(&b.uri));
    Ok(out)
}

fn document_edits(item: &Value) -> Option<DocumentEdits> {
    let document = item.get("textDocument")?;
    let uri = document.get("uri")?.as_str()?;
    Some(DocumentEdits {
        uri: uri.to_string(),
        path: crate::diagnostics::path_from_uri(uri).unwrap_or_else(|| uri.to_string()),
        // `version` is present but null for "unversioned"; both spellings
        // mean the same thing here.
        version: document
            .get("version")
            .and_then(Value::as_i64)
            .map(|v| v as i32),
        edits: text_edits(item.get("edits")?)?,
    })
}

fn text_edits(value: &Value) -> Option<Vec<TextEdit>> {
    value.as_array()?.iter().map(text_edit).collect()
}

/// One `TextEdit`, or an `AnnotatedTextEdit` — which is a `TextEdit` plus an
/// `annotationId` naming a group the user could accept or reject
/// separately. We apply an edit whole, so the annotation is read past
/// rather than honoured.
fn text_edit(value: &Value) -> Option<TextEdit> {
    let range = value.get("range")?;
    let start = range.get("start")?;
    let end = range.get("end")?;
    Some(TextEdit {
        start_line: start.get("line")?.as_u64()? as u32,
        start_character: start.get("character")?.as_u64()? as u32,
        end_line: end.get("line")?.as_u64()? as u32,
        end_character: end.get("character")?.as_u64()? as u32,
        new_text: value.get("newText")?.as_str()?.to_string(),
    })
}

/// `edits`, sorted so the last edit in the document comes first.
///
/// Applying in that order means every edit still addresses the offsets it
/// was computed against, which is the same reason `FindBar::replaceAll`
/// splices its spans back to front. Sorting is done here, once, so no
/// caller — least of all the C++ that splices open buffers — has ordering
/// logic of its own to get wrong.
pub fn descending(mut edits: Vec<TextEdit>) -> Vec<TextEdit> {
    edits.sort_by(|a, b| b.start().cmp(&a.start()).then(b.end().cmp(&a.end())));
    edits
}

/// Apply `edits` to `text`, returning the new text.
///
/// All-or-nothing: every range is validated against the document before a
/// single character moves, so a file is either fully rewritten or left
/// exactly as it was.
pub fn apply_to_text(text: &str, edits: &[TextEdit]) -> Result<String, EditError> {
    let offsets = line_offsets(text);
    let mut resolved = Vec::with_capacity(edits.len());
    for edit in edits {
        let start = byte_offset(text, &offsets, edit.start_line, edit.start_character)
            .ok_or(EditError::RangeOutOfBounds)?;
        let end = byte_offset(text, &offsets, edit.end_line, edit.end_character)
            .ok_or(EditError::RangeOutOfBounds)?;
        if start > end {
            return Err(EditError::RangeOutOfBounds);
        }
        resolved.push((start, end, edit.new_text.as_str()));
    }

    // Last first, so earlier offsets stay valid as we go.
    resolved.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    // Overlap check, on the sorted list: each edit must end no later than
    // the next one begins. Two insertions at the same point do not overlap
    // (both ranges are empty), and are applied in the order the server sent.
    for pair in resolved.windows(2) {
        let (later_start, _, _) = pair[0];
        let (_, earlier_end, _) = pair[1];
        if earlier_end > later_start {
            return Err(EditError::OverlappingEdits);
        }
    }

    let mut out = text.to_string();
    for (start, end, new_text) in resolved {
        out.replace_range(start..end, new_text);
    }
    Ok(out)
}

/// Byte offset of the start of every line, plus the length of the text as a
/// final sentinel, so the last line's end is addressable.
fn line_offsets(text: &str) -> Vec<usize> {
    let mut offsets = vec![0];
    offsets.extend(
        text.char_indices()
            .filter(|(_, ch)| *ch == '\n')
            .map(|(i, _)| i + 1),
    );
    offsets
}

/// A protocol position (0-based line, UTF-16 character within it) as a byte
/// offset into `text`.
///
/// A character offset past the end of its line clamps to the line's end
/// rather than failing: servers routinely address "the end of this line" as
/// a huge character number, and `u32::MAX` is the spec's own idiom for it.
/// A *line* past the end of the document is a real error, and is reported.
fn byte_offset(text: &str, offsets: &[usize], line: u32, character: u32) -> Option<usize> {
    let start = *offsets.get(line as usize)?;
    let line_text = &text[start..];
    let line_text = match line_text.find('\n') {
        Some(end) => &line_text[..end],
        None => line_text,
    };

    let mut utf16 = 0u32;
    for (index, ch) in line_text.char_indices() {
        if utf16 >= character {
            return Some(start + index);
        }
        utf16 += ch.len_utf16() as u32;
    }
    // Includes the `character == 0` case on an empty line.
    Some(start + line_text.len())
}

/// Where each document a `WorkspaceEdit` touches has to be applied.
///
/// The split is a rule, not a view detail: a document the user has open is
/// spliced into the live `QPlainTextEdit` so one Ctrl+Z undoes the whole
/// refactoring, while a document that is not open is rewritten on disk and
/// re-indexed. The view is told which pile each document is in; it never
/// decides.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EditPlan {
    /// Documents open in a tab, to be spliced in the buffer.
    pub buffers: Vec<DocumentEdits>,
    /// Documents not open, to be rewritten on disk.
    pub files: Vec<DocumentEdits>,
    /// Files this edit creates, renames or deletes, in the order the server
    /// sent them. Performed in full, before any text edit is written
    /// (ADR-0026) — the bridge maps each one to `app_core::FileOp` and
    /// nothing here decides what performing one means.
    pub ops: Vec<ResourceOp>,
    /// Whether anything outside the file the gesture started in is touched.
    /// This is what decides "apply straight away" from "show the preview
    /// first", so it is answered here rather than counted in C++. A resource
    /// operation always sets this: creating, renaming or deleting a file is
    /// never a same-file change.
    pub touches_other_files: bool,
}

impl EditPlan {
    /// How many documents the plan changes in total.
    pub fn document_count(&self) -> usize {
        self.buffers.len() + self.files.len()
    }

    /// How many individual edits the plan makes.
    pub fn edit_count(&self) -> usize {
        self.buffers
            .iter()
            .chain(self.files.iter())
            .map(|doc| doc.edits.len())
            .sum()
    }

    /// Nothing to do — a rename that changed no text, or an edit whose
    /// documents all resolved to nothing.
    pub fn is_empty(&self) -> bool {
        self.buffers.is_empty() && self.files.is_empty() && self.ops.is_empty()
    }
}

/// Split `docs` into what the buffers apply and what the disk applies, after
/// checking that every document is still the one the server was looking at.
///
/// `open_paths` are the files currently open in a tab, `current_path` is the
/// file the gesture started in (empty when there is none), and `versions`
/// answers what version this client last sent for a URI — [`
/// crate::LspManager::document_version`] in production.
///
/// The version rule: an entry naming a version other than the one we last
/// sent rejects the **whole** edit, not just that document. `None` is
/// accepted, because the protocol uses it for "unversioned, don't care".
/// Rejecting wholesale is the same reasoning `apply_to_text` uses within a
/// file — half of an extract-method is a corrupted program, and a rename
/// that reaches four files out of five is worse than one that reaches none.
///
/// The edits of each document come back in descending order, ready to apply.
pub fn plan(
    docs: Vec<DocumentEdits>,
    open_paths: &[String],
    current_path: &str,
    versions: &dyn Fn(&str) -> Option<i32>,
) -> Result<EditPlan, EditError> {
    let mut out = EditPlan::default();
    for mut doc in docs {
        if let Some(expected) = doc.version {
            if versions(&doc.uri) != Some(expected) {
                return Err(EditError::StaleVersion {
                    uri: doc.uri.clone(),
                });
            }
        }
        // A document the server named but made no edits to changes nothing,
        // and would otherwise show up in the preview as an empty row.
        if doc.edits.is_empty() {
            continue;
        }
        if doc.path != current_path {
            out.touches_other_files = true;
        }
        doc.edits = descending(std::mem::take(&mut doc.edits));
        if open_paths.contains(&doc.path) {
            out.buffers.push(doc);
        } else {
            out.files.push(doc);
        }
    }
    Ok(out)
}

/// [`plan`], extended to carry the resource operations a `WorkspaceChanges`
/// may include.
///
/// Ordering is deliberately simpler than the protocol's own "apply the array
/// in order": every resource operation is performed first, as one
/// all-or-nothing step, and only then are the text edits applied — matching
/// `app_core::apply_file_ops`'s own "abort before any text edit" rule
/// (ADR-0026). A server that interleaves a rename between two edits to make
/// the second one address the new path is not served correctly by this; no
/// server in the conformance suite does that, and getting it wrong reads as
/// "the refactoring did nothing" rather than corrupting a file, which is the
/// bar ADR-0019 sets.
pub fn plan_changes(
    changes: WorkspaceChanges,
    open_paths: &[String],
    current_path: &str,
    versions: &dyn Fn(&str) -> Option<i32>,
) -> Result<EditPlan, EditError> {
    let ops: Vec<ResourceOp> = changes.operations().cloned().collect();
    let docs: Vec<DocumentEdits> = changes.documents().cloned().collect();
    let mut out = plan(docs, open_paths, current_path, versions)?;
    out.touches_other_files |= !ops.is_empty();
    out.ops = ops;
    Ok(out)
}

/// Decides whether an edit computed against a buffer may still be applied to
/// it.
///
/// The sibling of [`crate::HoverTracker`], and needed for the same reason
/// with worse consequences: a refactoring is answered on a worker thread,
/// and a hover that lands late merely shows stale text, while an edit that
/// lands late rewrites the wrong bytes. Every request records the editor's
/// document revision; an answer is accepted only if the buffer has not moved
/// since.
///
/// This is not redundant with the version check in [`plan`]. That one
/// compares what the *server* was told; this one compares what the *editor*
/// actually holds, and the two can differ because `didChange` is debounced —
/// the buffer can move without the server hearing about it yet.
#[derive(Debug, Default)]
pub struct EditGate {
    pending: Option<i64>,
}

impl EditGate {
    /// Record the buffer revision a refactoring request is being made
    /// against, invalidating any request already in flight.
    pub fn begin(&mut self, revision: i64) {
        self.pending = Some(revision);
    }

    /// The gesture was abandoned: nothing in flight may be applied.
    pub fn cancel(&mut self) {
        self.pending = None;
    }

    /// May an answer be applied to a buffer now at `revision`?
    ///
    /// True only for the revision the request was made against — an edit is
    /// consumed once, so this also refuses a second application of the same
    /// answer.
    pub fn accept(&mut self, revision: i64) -> bool {
        match self.pending {
            Some(pending) if pending == revision => {
                self.pending = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(
        start_line: u32,
        start_character: u32,
        end_line: u32,
        end_character: u32,
        new_text: &str,
    ) -> TextEdit {
        TextEdit {
            start_line,
            start_character,
            end_line,
            end_character,
            new_text: new_text.to_string(),
        }
    }

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Value {
        json!({"start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec}})
    }

    #[test]
    fn document_changes_are_parsed_with_their_versions() {
        let value = json!({"documentChanges": [{
            "textDocument": {"uri": "file:///a/main.rs", "version": 7},
            "edits": [{"range": range(1, 0, 1, 3), "newText": "let"}],
        }]});

        let docs = parse_workspace_edit(&value).unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].path, "/a/main.rs");
        assert_eq!(docs[0].version, Some(7));
        assert_eq!(docs[0].edits, vec![edit(1, 0, 1, 3, "let")]);
    }

    #[test]
    fn legacy_changes_are_parsed_and_ordered_by_uri() {
        let value = json!({"changes": {
            "file:///a/z.rs": [{"range": range(0, 0, 0, 1), "newText": "z"}],
            "file:///a/a.rs": [{"range": range(0, 0, 0, 1), "newText": "a"}],
        }});

        let docs = parse_workspace_edit(&value).unwrap();
        assert_eq!(
            docs.iter().map(|d| d.path.as_str()).collect::<Vec<_>>(),
            vec!["/a/a.rs", "/a/z.rs"],
        );
        assert!(docs.iter().all(|d| d.version.is_none()));
    }

    #[test]
    fn document_changes_win_over_changes_and_are_never_merged() {
        let value = json!({
            "changes": {"file:///a/legacy.rs": [{"range": range(0, 0, 0, 1), "newText": "x"}]},
            "documentChanges": [{
                "textDocument": {"uri": "file:///a/modern.rs", "version": null},
                "edits": [{"range": range(0, 0, 0, 1), "newText": "y"}],
            }],
        });

        let docs = parse_workspace_edit(&value).unwrap();
        assert_eq!(docs.len(), 1, "the legacy field must not be merged in");
        assert_eq!(docs[0].path, "/a/modern.rs");
        assert_eq!(docs[0].version, None, "an explicit null means unversioned");
    }

    #[test]
    fn an_annotated_edit_is_applied_as_a_plain_one() {
        let value = json!({"documentChanges": [{
            "textDocument": {"uri": "file:///a/main.rs", "version": 1},
            "edits": [{
                "range": range(0, 0, 0, 1), "newText": "x", "annotationId": "group-1",
            }],
        }]});

        let docs = parse_workspace_edit(&value).unwrap();
        assert_eq!(docs[0].edits, vec![edit(0, 0, 0, 1, "x")]);
    }

    #[test]
    fn a_resource_operation_rejects_the_whole_edit() {
        for kind in ["create", "rename", "delete"] {
            let value = json!({"documentChanges": [
                {
                    "textDocument": {"uri": "file:///a/main.rs", "version": 1},
                    "edits": [{"range": range(0, 0, 0, 1), "newText": "x"}],
                },
                {"kind": kind, "uri": "file:///a/new.rs"},
            ]});

            assert_eq!(
                parse_workspace_edit(&value),
                Err(EditError::ResourceOperation(kind.to_string())),
                "a {kind} operation anywhere must refuse the edit, not drop that entry",
            );
        }
    }

    #[test]
    fn an_empty_or_null_edit_is_not_an_error() {
        assert_eq!(parse_workspace_edit(&Value::Null), Ok(Vec::new()));
        assert_eq!(parse_workspace_edit(&json!({})), Ok(Vec::new()));
    }

    #[test]
    fn an_unreadable_payload_is_malformed() {
        assert_eq!(
            parse_workspace_edit(&json!("nonsense")),
            Err(EditError::Malformed),
        );
        assert_eq!(
            parse_workspace_edit(&json!({"documentChanges": [{"textDocument": {}}]})),
            Err(EditError::Malformed),
        );
    }

    #[test]
    fn a_single_line_replacement_applies() {
        let text = "let alpha = 1;\nlet beta = 2;\n";
        let out = apply_to_text(text, &[edit(0, 4, 0, 9, "gamma")]).unwrap();
        assert_eq!(out, "let gamma = 1;\nlet beta = 2;\n");
    }

    #[test]
    fn a_multi_line_range_is_replaced_whole() {
        // The shape every extract-method edit has: a block of lines out, a
        // call in — the case a single-line span type could not express.
        let text = "fn main() {\n    let a = 1;\n    let b = 2;\n}\n";
        let out = apply_to_text(text, &[edit(1, 4, 2, 14, "extracted();")]).unwrap();
        assert_eq!(out, "fn main() {\n    extracted();\n}\n");
    }

    #[test]
    fn an_empty_range_inserts() {
        let text = "fn main() {}\n";
        let out = apply_to_text(text, &[edit(0, 0, 0, 0, "#[test]\n")]).unwrap();
        assert_eq!(out, "#[test]\nfn main() {}\n");
    }

    #[test]
    fn several_edits_apply_back_to_front_in_one_pass() {
        let text = "one two three\n";
        let out = apply_to_text(text, &[edit(0, 0, 0, 3, "1"), edit(0, 8, 0, 13, "3")]).unwrap();
        assert_eq!(out, "1 two 3\n");
    }

    #[test]
    fn crlf_line_endings_survive() {
        let text = "let a = 1;\r\nlet b = 2;\r\n";
        let out = apply_to_text(text, &[edit(1, 4, 1, 5, "beta")]).unwrap();
        assert_eq!(out, "let a = 1;\r\nlet beta = 2;\r\n");
    }

    #[test]
    fn characters_are_counted_in_utf16_code_units() {
        // "𝄞" is one char but two UTF-16 code units, so a server counting
        // the protocol's way names character 2 for what Rust calls byte 4.
        let text = "let 𝄞 = 1;\n";
        let out = apply_to_text(text, &[edit(0, 4, 0, 6, "clef")]).unwrap();
        assert_eq!(out, "let clef = 1;\n");
    }

    #[test]
    fn a_character_past_the_end_of_a_line_clamps_to_it() {
        // The spec's own idiom for "the end of this line".
        let text = "one\ntwo\n";
        let out = apply_to_text(text, &[edit(0, 0, 0, u32::MAX, "1")]).unwrap();
        assert_eq!(out, "1\ntwo\n");
    }

    #[test]
    fn a_line_past_the_end_of_the_document_is_rejected() {
        let text = "one\n";
        assert_eq!(
            apply_to_text(text, &[edit(9, 0, 9, 1, "x")]),
            Err(EditError::RangeOutOfBounds),
        );
    }

    #[test]
    fn an_inverted_range_is_rejected() {
        let text = "one two\n";
        assert_eq!(
            apply_to_text(text, &[edit(0, 5, 0, 1, "x")]),
            Err(EditError::RangeOutOfBounds),
        );
    }

    #[test]
    fn overlapping_edits_are_rejected_rather_than_ordered() {
        let text = "one two three\n";
        assert_eq!(
            apply_to_text(text, &[edit(0, 0, 0, 7, "a"), edit(0, 4, 0, 13, "b")]),
            Err(EditError::OverlappingEdits),
        );
    }

    #[test]
    fn two_insertions_at_the_same_point_do_not_count_as_overlapping() {
        let text = "x\n";
        let out = apply_to_text(text, &[edit(0, 0, 0, 0, "a"), edit(0, 0, 0, 0, "b")]).unwrap();
        assert_eq!(out.len(), 4, "both insertions landed: {out:?}");
    }

    #[test]
    fn nothing_is_applied_when_any_edit_is_invalid() {
        let text = "one two\n";
        assert_eq!(
            apply_to_text(text, &[edit(0, 0, 0, 3, "1"), edit(9, 0, 9, 1, "x")]),
            Err(EditError::RangeOutOfBounds),
            "one bad range must refuse the file, not apply the good edit",
        );
    }

    #[test]
    fn descending_puts_the_last_edit_first() {
        let sorted = descending(vec![
            edit(0, 0, 0, 1, "a"),
            edit(4, 2, 4, 3, "c"),
            edit(2, 0, 2, 1, "b"),
        ]);
        assert_eq!(
            sorted.iter().map(|e| e.start_line).collect::<Vec<_>>(),
            vec![4, 2, 0],
        );
    }
    fn doc(path: &str, version: Option<i32>, edits: Vec<TextEdit>) -> DocumentEdits {
        DocumentEdits {
            uri: format!("file://{path}"),
            path: path.to_string(),
            version,
            edits,
        }
    }

    fn no_versions(_: &str) -> Option<i32> {
        None
    }

    #[test]
    fn open_documents_go_to_the_buffers_and_the_rest_to_disk() {
        let docs = vec![
            doc("/a/open.rs", None, vec![edit(0, 0, 0, 1, "x")]),
            doc("/a/closed.rs", None, vec![edit(0, 0, 0, 1, "y")]),
        ];

        let plan = plan(
            docs,
            &["/a/open.rs".to_string()],
            "/a/open.rs",
            &no_versions,
        )
        .unwrap();

        assert_eq!(
            plan.buffers
                .iter()
                .map(|d| d.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/a/open.rs"],
        );
        assert_eq!(
            plan.files
                .iter()
                .map(|d| d.path.as_str())
                .collect::<Vec<_>>(),
            vec!["/a/closed.rs"],
        );
        assert!(plan.touches_other_files);
        assert_eq!(plan.document_count(), 2);
        assert_eq!(plan.edit_count(), 2);
    }

    #[test]
    fn an_edit_confined_to_the_current_file_touches_nothing_else() {
        let plan = plan(
            vec![doc("/a/main.rs", None, vec![edit(0, 0, 0, 1, "x")])],
            &["/a/main.rs".to_string()],
            "/a/main.rs",
            &no_versions,
        )
        .unwrap();

        assert!(
            !plan.touches_other_files,
            "a same-file edit applies without a preview",
        );
        assert!(plan.files.is_empty());
    }

    #[test]
    fn with_nothing_open_every_document_is_written_to_disk() {
        let plan = plan(
            vec![doc("/a/main.rs", None, vec![edit(0, 0, 0, 1, "x")])],
            &[],
            "",
            &no_versions,
        )
        .unwrap();

        assert!(plan.buffers.is_empty());
        assert_eq!(plan.files.len(), 1);
        assert!(plan.touches_other_files);
    }

    #[test]
    fn a_matching_version_is_accepted_and_an_absent_one_is_not_checked() {
        let plan = plan(
            vec![
                doc("/a/versioned.rs", Some(4), vec![edit(0, 0, 0, 1, "x")]),
                doc("/a/unversioned.rs", None, vec![edit(0, 0, 0, 1, "y")]),
            ],
            &[],
            "",
            &|uri| (uri == "file:///a/versioned.rs").then_some(4),
        )
        .unwrap();

        assert_eq!(plan.document_count(), 2);
    }

    #[test]
    fn one_stale_version_rejects_the_whole_edit() {
        let result = plan(
            vec![
                doc("/a/fresh.rs", Some(2), vec![edit(0, 0, 0, 1, "x")]),
                doc("/a/stale.rs", Some(2), vec![edit(0, 0, 0, 1, "y")]),
            ],
            &[],
            "",
            &|uri| (uri == "file:///a/fresh.rs").then_some(2),
        );

        assert_eq!(
            result,
            Err(EditError::StaleVersion {
                uri: "file:///a/stale.rs".into()
            }),
            "a document we never sent that version for is stale, not skippable",
        );
    }

    #[test]
    fn a_document_with_no_edits_is_dropped_rather_than_listed() {
        let plan = plan(
            vec![
                doc("/a/main.rs", None, vec![edit(0, 0, 0, 1, "x")]),
                doc("/a/untouched.rs", None, vec![]),
            ],
            &[],
            "/a/main.rs",
            &no_versions,
        )
        .unwrap();

        assert_eq!(plan.document_count(), 1);
        assert!(
            !plan.touches_other_files,
            "a document nothing happens to must not force a preview",
        );
    }

    #[test]
    fn an_empty_edit_plans_to_nothing() {
        let plan = plan(Vec::new(), &[], "", &no_versions).unwrap();
        assert!(plan.is_empty());
        assert_eq!(plan.edit_count(), 0);
    }

    #[test]
    fn planned_edits_come_back_ready_to_apply_back_to_front() {
        let plan = plan(
            vec![doc(
                "/a/main.rs",
                None,
                vec![edit(0, 0, 0, 1, "a"), edit(5, 0, 5, 1, "b")],
            )],
            &[],
            "/a/main.rs",
            &no_versions,
        )
        .unwrap();

        assert_eq!(
            plan.files[0]
                .edits
                .iter()
                .map(|e| e.start_line)
                .collect::<Vec<_>>(),
            vec![5, 0],
        );
    }

    #[test]
    fn the_gate_accepts_only_the_revision_the_request_was_made_against() {
        let mut gate = EditGate::default();
        gate.begin(11);

        assert!(!gate.accept(12), "the buffer moved, so the edit is stale");
        gate.begin(11);
        assert!(gate.accept(11));
    }

    #[test]
    fn the_gate_answers_once_and_forgets_a_cancelled_request() {
        let mut gate = EditGate::default();
        gate.begin(3);
        assert!(gate.accept(3));
        assert!(!gate.accept(3), "an answer must not be applied twice");

        gate.begin(4);
        gate.cancel();
        assert!(!gate.accept(4));
    }

    #[test]
    fn the_gate_refuses_an_answer_nobody_asked_for() {
        let mut gate = EditGate::default();
        assert!(!gate.accept(0));
    }
}

#[cfg(test)]
mod resource_op_tests {
    use super::*;
    use serde_json::json;

    fn range(sl: u32, sc: u32, el: u32, ec: u32) -> Value {
        json!({"start": {"line": sl, "character": sc}, "end": {"line": el, "character": ec}})
    }

    #[test]
    fn create_rename_and_delete_are_read_with_their_options() {
        let value = json!({"documentChanges": [
            {"kind": "create", "uri": "file:///p/new.rs", "options": {"overwrite": true}},
            {"kind": "rename", "oldUri": "file:///p/a.rs", "newUri": "file:///p/b.rs"},
            {"kind": "delete", "uri": "file:///p/old.rs", "options": {"recursive": true}},
        ]});
        let changes = parse_workspace_changes(&value).unwrap();
        let ops: Vec<_> = changes.operations().cloned().collect();
        assert_eq!(ops.len(), 3);
        assert!(matches!(
            &ops[0],
            ResourceOp::Create {
                overwrite: true,
                ignore_if_exists: false,
                ..
            }
        ));
        assert!(matches!(
            &ops[1],
            ResourceOp::Rename { old_path, new_path, .. }
                if old_path.ends_with("a.rs") && new_path.ends_with("b.rs")
        ));
        assert!(matches!(
            &ops[2],
            ResourceOp::Delete {
                recursive: true,
                ..
            }
        ));
    }

    // The specification applies documentChanges in order, and real
    // refactorings depend on it: "rename the type, then rename its file to
    // match" is a text edit followed by a rename. Reordering breaks it.
    #[test]
    fn order_is_preserved_between_edits_and_operations() {
        let value = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///p/a.rs", "version": 1},
             "edits": [{"range": range(0, 0, 0, 3), "newText": "Bar"}]},
            {"kind": "rename", "oldUri": "file:///p/a.rs", "newUri": "file:///p/bar.rs"},
        ]});
        let changes = parse_workspace_changes(&value).unwrap();
        assert!(matches!(changes.steps[0], ChangeStep::Edits(_)));
        assert!(matches!(changes.steps[1], ChangeStep::Op(_)));
    }

    #[test]
    fn an_unknown_kind_is_malformed_rather_than_ignored() {
        let value = json!({"documentChanges": [{"kind": "teleport", "uri": "file:///p/a.rs"}]});
        assert_eq!(
            parse_workspace_changes(&value),
            Err(EditError::Malformed),
            "an operation we do not understand must not be silently skipped"
        );
    }

    #[test]
    fn a_rename_missing_its_target_is_malformed() {
        let value = json!({"documentChanges": [{"kind": "rename", "oldUri": "file:///p/a.rs"}]});
        assert_eq!(parse_workspace_changes(&value), Err(EditError::Malformed));
    }

    // The legacy `changes` map has no ordering and cannot express a resource
    // operation, so it reads exactly as it always has.
    #[test]
    fn the_legacy_changes_map_still_works_and_has_no_operations() {
        let value = json!({"changes": {
            "file:///p/a.rs": [{"range": range(0, 0, 0, 1), "newText": "x"}],
        }});
        let changes = parse_workspace_changes(&value).unwrap();
        assert!(!changes.has_operations());
        assert_eq!(changes.documents().count(), 1);
    }

    // The text-only reading keeps refusing, because its callers splice into
    // open buffers and have nowhere to put a file rename.
    #[test]
    fn the_text_only_parser_still_refuses_resource_operations() {
        let value = json!({"documentChanges": [
            {"kind": "rename", "oldUri": "file:///p/a.rs", "newUri": "file:///p/b.rs"},
        ]});
        assert!(matches!(
            parse_workspace_edit(&value),
            Err(EditError::ResourceOperation(_))
        ));
    }

    #[test]
    fn a_null_edit_has_no_steps() {
        assert_eq!(
            parse_workspace_changes(&Value::Null).unwrap(),
            WorkspaceChanges::default()
        );
    }

    #[test]
    fn plan_changes_carries_the_operations_and_marks_other_files_touched() {
        let value = json!({"documentChanges": [
            {"kind": "create", "uri": "file:///p/new.rs"},
        ]});
        let changes = parse_workspace_changes(&value).unwrap();
        let plan = plan_changes(changes, &[], "", &|_| None).unwrap();
        assert_eq!(plan.ops.len(), 1);
        assert!(plan.touches_other_files);
        assert!(!plan.is_empty());
    }

    #[test]
    fn plan_changes_still_splits_documents_between_buffers_and_files() {
        let value = json!({"documentChanges": [
            {"textDocument": {"uri": "file:///p/a.rs", "version": null},
             "edits": [{"range": range(0, 0, 0, 3), "newText": "Bar"}]},
            {"kind": "delete", "uri": "file:///p/old.rs"},
        ]});
        let changes = parse_workspace_changes(&value).unwrap();
        let open = vec!["/p/a.rs".to_string()];
        let plan = plan_changes(changes, &open, "/p/a.rs", &|_| None).unwrap();
        assert_eq!(plan.buffers.len(), 1);
        assert_eq!(plan.ops.len(), 1);
        // The edit is to the current file, but the delete still makes this
        // a multi-file change — a resource operation is never "the same
        // file", even when it is the only other thing in the plan.
        assert!(plan.touches_other_files);
    }

    #[test]
    fn a_plan_with_no_documents_or_operations_is_empty() {
        let plan = plan_changes(WorkspaceChanges::default(), &[], "", &|_| None).unwrap();
        assert!(plan.is_empty());
    }
}
