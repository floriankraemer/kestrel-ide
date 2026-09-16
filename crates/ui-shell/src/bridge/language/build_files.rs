//! Build-file coordinate completion in the editor (D5, jvm-build-tools
//! plan): pom.xml / build.gradle(.kts) / libs.versions.toml, injected
//! into `LanguageService::completion_at` *before* the language-server
//! path — D0 registers these files in `open_docs` with no server, so
//! without this branch they would silently get nothing (`push_job`'s
//! `manager.completion()` call just errors `NoServer` and the closure
//! returns).
//!
//! Two-phase delivery, the same shape `containers.rs`'s Hub lookup uses:
//! D3's local repository index answers synchronously (no network wait),
//! and D4's Maven Central client answers on a background thread, then
//! re-delivers through the *same* `CompletionTracker` token the LSP path
//! uses (`completion.begin()`/`deliver(token, ..)`) — a `deliver` that
//! returns `false` means a newer request or a caret move already
//! superseded this one, and the stale answer is dropped. Two separate
//! `deliver`s (rather than blocking the first popup on the network) is
//! the whole point: the local index answers in microseconds and the
//! popup should not sit empty waiting on a socket that might time out.
//!
//! `*self.completion_language.borrow_mut() = None` marks every item from
//! this path exactly the way `fallback_completion`/`container_completion`
//! already do: `accept_completion`'s `resolvable` check short-circuits on
//! a `None` language, so `completionItem/resolve` is never attempted —
//! there is no server to resolve against.
//!
//! Known gap, left open rather than half-implemented: `libs.versions.toml`
//! `version.ref = "…"` fields get no candidates from either source. That
//! field's value is a *key* into the same file's own `[versions]` table —
//! answering it needs a whole-document scan `editing::context` was never
//! asked to do (D2's scope is the caret's own context, not a
//! document-wide index), and misusing D3/D4's *version-number* candidates
//! for a *key-name* field would show entirely wrong suggestions Whatever
//! way this gets built, D2 is where the scan belongs, not this bridge.
//!
//! D3's local repository index is `BuildToolsService`'s (review fix #2):
//! built off the Qt thread, on its own background sync thread — once at
//! `projectOpened` and again after every sync — and shared here through
//! `registry::shared_repo_index`, a `thread_local!` `Rc<RefCell<..>>` the
//! same shape `shared_build_models` already uses (both QObjects live on
//! the one Qt thread, so a thread-local is enough; nothing here ever
//! walks `~/.m2`/`~/.gradle` itself). `None` until the first background
//! build finishes — every reader treats that as "answer from Central
//! only", never as a reason to block or fall back to walking the
//! filesystem synchronously.
//!
//! The thread-local only reads correctly *on* the Qt thread, so every
//! background thread this file itself spawns (Central's network calls)
//! takes an owned snapshot — [`local_index_snapshot`] — before spawning,
//! rather than reaching into the thread-local from the worker thread.

use std::path::Path;
use std::pin::Pin;

use cxx_qt::Threading;

use jvm_build_core::editing::central;
use jvm_build_core::editing::completion::{self, Completion};
use jvm_build_core::editing::context::{
    self, CoordinatePart, DeclaredVersion, EditContext, TomlLibraryField,
};
use jvm_build_core::editing::repo_index::RepoIndex;
use jvm_build_core::editing::versions::{self, VersionHint};

use crate::bridge::ffi;

/// D6's diagnostics-store key (ADR-0046): a version hint never clobbers,
/// or gets clobbered by, a sync failure (`build-tools:sync`) or a
/// language server's own rows for the same file.
const VERSION_HINTS_SOURCE: &str = "build-tools:versions";

/// Review fix #10: the most Central round trips one `refresh_version_hints`
/// background pass will make — bounds worst-case latency for a project
/// with many declared dependencies and a still-cold disk cache (D4's own
/// cache already bounds every *repeat* save regardless).
const MAX_CENTRAL_LOOKUPS_PER_SAVE: usize = 25;

/// An owned copy of whatever `BuildToolsService` has published so far —
/// safe to move into a background thread, unlike the `thread_local!` this
/// clones out of. Qt-thread-only to call (the thread-local itself is).
fn local_index_snapshot() -> Option<RepoIndex> {
    crate::bridge::registry::shared_repo_index()
        .borrow()
        .clone()
}

/// A UTF-16 `(line, character)` caret position (the shape `completion_at`
/// receives) converted to the byte offset `editing::context::context`
/// needs.
fn caret_byte_offset(text: &str, line: u32, character: u32) -> usize {
    let starts = editor_core::offsets::line_starts(text);
    let line_range = editor_core::offsets::line_range(text, &starts, line as usize);
    let line_text = &text[line_range.clone()];
    line_range.start + editor_core::offsets::byte_offset(line_text, character as usize)
}

fn byte_range_to_text_range(text: &str, range: std::ops::Range<usize>) -> lsp_core::TextRange {
    let starts = editor_core::offsets::utf16_line_starts(text);
    let start = editor_core::offsets::utf16_offset(text, range.start);
    let end = editor_core::offsets::utf16_offset(text, range.end);
    let (start_line, start_character) = editor_core::offsets::utf16_position_at(&starts, start);
    let (end_line, end_character) = editor_core::offsets::utf16_position_at(&starts, end);
    lsp_core::TextRange {
        start_line,
        start_character,
        end_line,
        end_character,
    }
}

fn to_completion_item(item: Completion, text: &str) -> lsp_core::CompletionItem {
    lsp_core::CompletionItem {
        label: item.label,
        kind: None,
        detail: String::new(),
        documentation: String::new(),
        sort_text: None,
        filter_text: None,
        insert: item.insert_text,
        is_snippet: false,
        deprecated: false,
        range: Some(byte_range_to_text_range(text, item.range)),
        raw: serde_json::Value::Null,
    }
}

fn to_completion_list(items: Vec<Completion>, text: &str) -> lsp_core::CompletionList {
    lsp_core::CompletionList {
        items: items
            .into_iter()
            .map(|item| to_completion_item(item, text))
            .collect(),
        is_incomplete: false,
    }
}

/// Every artifact id under every known group — used when a group has not
/// been typed yet (a bare `implementation("`) and there is nothing to
/// scope the artifact search to.
fn all_artifacts(index: &RepoIndex) -> Vec<String> {
    index
        .groups()
        .flat_map(|group| index.artifacts(group).map(str::to_string))
        .collect()
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

/// The local index's candidates for whichever part `ctx` names — empty
/// when `index` is `None` (nothing built yet; review fix #2's "answer
/// from Central only" rule). Provider-agnostic beyond this:
/// `jvm_build_core::editing::completion::items` does the actual ranking
/// against whatever list this (or [`central_candidates`]) hands it.
fn local_candidates(ctx: &EditContext, index: Option<&RepoIndex>) -> Vec<String> {
    let Some(index) = index else {
        return Vec::new();
    };
    match ctx {
        EditContext::PomCoordinate {
            part, coordinate, ..
        }
        | EditContext::GradleCoordinate {
            part, coordinate, ..
        } => match part {
            CoordinatePart::GroupId => index.groups().map(str::to_string).collect(),
            CoordinatePart::ArtifactId => match non_empty(&coordinate.group_id) {
                Some(group) => index.artifacts(group).map(str::to_string).collect(),
                None => all_artifacts(index),
            },
            CoordinatePart::Version => {
                match (
                    non_empty(&coordinate.group_id),
                    non_empty(&coordinate.artifact_id),
                ) {
                    (Some(group), Some(artifact)) => index
                        .versions(group, artifact)
                        .into_iter()
                        .map(str::to_string)
                        .collect(),
                    _ => Vec::new(),
                }
            }
        },
        EditContext::GradlePluginVersion { plugin_id, .. } => match non_empty(plugin_id) {
            // Gradle Plugin Portal's own convention: a plugin `id` is
            // published to the local cache as `<id>:<id>.gradle.plugin`.
            Some(id) => index
                .versions(id, &format!("{id}.gradle.plugin"))
                .into_iter()
                .map(str::to_string)
                .collect(),
            None => Vec::new(),
        },
        // See this module's doc comment: neither has an unambiguous local
        // answer without a whole-document scan `editing::context` does
        // not do.
        EditContext::TomlVersion { .. } => Vec::new(),
        EditContext::TomlLibraryField { field, entry, .. } => match field {
            TomlLibraryField::Module => Vec::new(),
            TomlLibraryField::Group => index.groups().map(str::to_string).collect(),
            TomlLibraryField::Name => match non_empty(&entry.group) {
                Some(group) => index.artifacts(group).map(str::to_string).collect(),
                None => all_artifacts(index),
            },
            TomlLibraryField::VersionRef => Vec::new(),
        },
    }
}

/// The Solr `q` value for a group/artifact-id search, or `None` when
/// there is nothing worth asking Central about yet (an empty prefix would
/// return an arbitrary 20-row sample of the entire index).
fn central_search_query(ctx: &EditContext) -> Option<String> {
    match ctx {
        EditContext::PomCoordinate {
            part, coordinate, ..
        }
        | EditContext::GradleCoordinate {
            part, coordinate, ..
        } => match part {
            CoordinatePart::GroupId => {
                non_empty(&coordinate.group_id).map(|typed| format!("g:{typed}*"))
            }
            CoordinatePart::ArtifactId => {
                let typed = non_empty(&coordinate.artifact_id)?;
                Some(match non_empty(&coordinate.group_id) {
                    Some(group) => format!("g:{group} AND a:{typed}*"),
                    None => format!("a:{typed}*"),
                })
            }
            CoordinatePart::Version => None,
        },
        _ => None,
    }
}

/// The `(group, artifact)` a Central `maven-metadata.xml` version lookup
/// needs, when `ctx` has both.
fn central_version_lookup(ctx: &EditContext) -> Option<(String, String)> {
    match ctx {
        EditContext::PomCoordinate {
            part: CoordinatePart::Version,
            coordinate,
            ..
        }
        | EditContext::GradleCoordinate {
            part: CoordinatePart::Version,
            coordinate,
            ..
        } => Some((
            non_empty(&coordinate.group_id)?.to_string(),
            non_empty(&coordinate.artifact_id)?.to_string(),
        )),
        EditContext::GradlePluginVersion { plugin_id, .. } => {
            let id = non_empty(plugin_id)?;
            Some((id.to_string(), format!("{id}.gradle.plugin")))
        }
        _ => None,
    }
}

/// D4's answer for `ctx`, blocking — always called off the Qt thread.
fn central_candidates(ctx: &EditContext, client: &central::CentralClient) -> Vec<String> {
    if let Some((group, artifact)) = central_version_lookup(ctx) {
        return client.versions(&group, &artifact);
    }
    if let Some(query) = central_search_query(ctx) {
        let artifacts = client.search(&query);
        return match ctx {
            EditContext::PomCoordinate {
                part: CoordinatePart::GroupId,
                ..
            }
            | EditContext::GradleCoordinate {
                part: CoordinatePart::GroupId,
                ..
            } => artifacts.into_iter().map(|a| a.group_id).collect(),
            _ => artifacts.into_iter().map(|a| a.artifact_id).collect(),
        };
    }
    Vec::new()
}

/// `pom.xml` reads `[build_tools.maven].offline`; every other build file
/// this module recognises (`build.gradle(.kts)`, `libs.versions.toml`) is
/// Gradle's, and reads `[build_tools.gradle].offline`.
fn is_offline_for(path: &str) -> bool {
    let settings = crate::bridge::convert::load_resolved_settings();
    let name = Path::new(path)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if name == "pom.xml" {
        settings.build_tools.maven.offline.unwrap_or(false)
    } else {
        settings.build_tools.gradle.offline.unwrap_or(false)
    }
}

impl ffi::LanguageService {
    /// Returns `true` when `path` is a build file and the caret sits
    /// inside a coordinate `editing::context::context` recognises —
    /// `completion_at` treats that as "handled", the same convention
    /// `container_completion` already uses, whether or not this call
    /// actually had anything new to say.
    pub(crate) fn build_file_completion(
        mut self: Pin<&mut Self>,
        path: &str,
        line: u32,
        character: u32,
        explicit_request: bool,
    ) -> bool {
        let Some(content) = self.session.borrow().content_for_path(Path::new(path)) else {
            return false;
        };
        let caret = caret_byte_offset(&content, line, character);
        let Some(ctx) = context::context(Path::new(path), &content, caret) else {
            return false;
        };

        let range = match &ctx {
            EditContext::PomCoordinate { range, .. }
            | EditContext::GradleCoordinate { range, .. }
            | EditContext::GradlePluginVersion { range, .. }
            | EditContext::TomlVersion { range, .. }
            | EditContext::TomlLibraryField { range, .. } => range.clone(),
        };
        let typed = content.get(range).unwrap_or_default();
        // `CompletionTracker` (`still_typing`/`needs_request`) is built
        // for a bare-word prefix — see `completion::tracker_prefix`'s own
        // doc comment for why passing the whole dotted/hyphenated literal
        // to it leaves the popup empty from the second character on.
        let tracker_prefix = completion::tracker_prefix(typed);
        if !self
            .completion
            .borrow()
            .needs_request(tracker_prefix, explicit_request)
        {
            return true;
        }

        *self.completion_language.borrow_mut() = None;
        let token = self.completion.borrow_mut().begin(tracker_prefix);

        let index = local_index_snapshot();
        let local = local_candidates(&ctx, index.as_ref());
        let items = completion::items(&ctx, &content, &local);
        *self.completions.borrow_mut() = to_completion_list(items, &content);
        self.as_mut().completion_ready();

        let offline = is_offline_for(path);
        if !offline {
            let config_dir = app_core::resolve_config_dir();
            let ctx_for_thread = ctx.clone();
            let content_for_thread = content.clone();
            let index_for_thread = index;
            let qt_thread = self.as_mut().qt_thread();
            std::thread::spawn(move || {
                let client = central::CentralClient::new(&config_dir, false);
                let remote = central_candidates(&ctx_for_thread, &client);
                if remote.is_empty() {
                    return;
                }
                let merged: Vec<String> =
                    local_candidates(&ctx_for_thread, index_for_thread.as_ref())
                        .into_iter()
                        .chain(remote)
                        .collect();
                let items = completion::items(&ctx_for_thread, &content_for_thread, &merged);
                let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                    if !service.completion.borrow_mut().deliver(token, false) {
                        return;
                    }
                    *service.completions.borrow_mut() =
                        to_completion_list(items, &content_for_thread);
                    service.as_mut().completion_ready();
                });
            });
        }
        true
    }

    /// D6: recompute every "newer version available" hint for `path` and
    /// republish them under [`VERSION_HINTS_SOURCE`] — replacing whatever
    /// this source previously held for the file, an empty list included
    /// (an upgrade landing, or the last declared dependency being
    /// deleted, must clear the row exactly as publishing a fresh one
    /// does). Called from `document_opened`'s build-file branch (D0) and
    /// `document_saved`; `document_closed` calls [`clear_version_hints`]
    /// instead.
    ///
    /// A no-op, cheaply, for a file `editing::context::declared_versions`
    /// does not recognise — every caller here runs unconditionally on
    /// every open/save rather than pre-filtering by extension, so this is
    /// the one place that check lives.
    ///
    /// Not yet wired to "after a sync" (the plan's third refresh trigger):
    /// that event lives on `BuildToolsService`, a different `#[qobject]`
    /// than `LanguageService`, and cross-service wiring is the same
    /// deferred follow-up D5's own doc comment already names for sharing
    /// `RepoIndex` — a sync's freshly-resolved versions do widen what a
    /// hint can see, but open/save already recompute on the *declared*
    /// side; the local disk index a hint compares against just is not
    /// forced to reload until the process restarts (D3/D5's own
    /// known limitation, inherited here).
    pub(crate) fn refresh_version_hints(mut self: Pin<&mut Self>, path: &str) {
        let Some(content) = self.session.borrow().content_for_path(Path::new(path)) else {
            return;
        };
        let declared = context::declared_versions(Path::new(path), &content);
        let uri = lsp_core::uri_from_path(path);
        if declared.is_empty() {
            self.store
                .borrow_mut()
                .replace(VERSION_HINTS_SOURCE, &uri, Vec::new());
            self.as_mut().diagnostics_changed();
            return;
        }

        let index = local_index_snapshot();
        let local_hints = local_only_hints(&declared, index.as_ref());
        self.store.borrow_mut().replace(
            VERSION_HINTS_SOURCE,
            &uri,
            to_diagnostics(local_hints, &content),
        );
        self.as_mut().diagnostics_changed();

        if is_offline_for(path) {
            return;
        }
        // Review fix #10: guards against a stale background delivery
        // (an earlier save's) landing after a newer one already has.
        let token = self.version_hints_tracker.borrow_mut().begin();
        let config_dir = app_core::resolve_config_dir();
        let path_owned = path.to_string();
        let content_for_thread = content.clone();
        let qt_thread = self.as_mut().qt_thread();
        std::thread::spawn(move || {
            let client = central::CentralClient::new(&config_dir, false);
            // Review fix #10: bounds how many blocking Central round trips
            // one save can trigger — a project with hundreds of
            // dependencies must not turn every save into hundreds of
            // sequential network calls (each already cache-backed on its
            // own, but a *cold* cache is exactly the case that would make
            // the very first open of a large project feel frozen). Past
            // the cap, a dependency's hint falls back to the local index
            // alone for this pass; a later save (or the cache warming up
            // from other dependencies already queried) picks it up.
            let mut central_lookups_remaining = MAX_CENTRAL_LOOKUPS_PER_SAVE;
            let hints = versions::hints(&declared, |group, artifact| {
                let mut candidates = index
                    .as_ref()
                    .map(|index| {
                        index
                            .versions(group, artifact)
                            .into_iter()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                if central_lookups_remaining > 0 {
                    central_lookups_remaining -= 1;
                    candidates.extend(client.versions(group, artifact));
                }
                candidates
            });
            let uri = lsp_core::uri_from_path(&path_owned);
            let _ = qt_thread.queue(move |mut service: Pin<&mut Self>| {
                if !service.version_hints_tracker.borrow().accept(token) {
                    return;
                }
                // The file may have been closed while Central was
                // answering; a closed file's diagnostics were already
                // cleared and must stay cleared.
                if !service.open_docs.borrow().contains_key(&path_owned) {
                    return;
                }
                service.store.borrow_mut().replace(
                    VERSION_HINTS_SOURCE,
                    &uri,
                    to_diagnostics(hints, &content_for_thread),
                );
                service.as_mut().diagnostics_changed();
            });
        });
    }

    /// D6: forget every version hint this source published for `path` —
    /// `document_closed`'s counterpart to [`refresh_version_hints`],
    /// mirroring how it already forgets the file's language-server rows.
    pub(crate) fn clear_version_hints(mut self: Pin<&mut Self>, path: &str) {
        let uri = lsp_core::uri_from_path(path);
        self.store.borrow_mut().remove(VERSION_HINTS_SOURCE, &uri);
        self.as_mut().diagnostics_changed();
    }

    /// D7's synthesised "Update to `latest`" quick fix for the caret in
    /// `path`, if any — `None` both for a file this module does not
    /// recognise and for a build file with no version-hint issue under
    /// the caret. Pure (no `self` mutation): shared by
    /// [`build_file_intentions`](Self::build_file_intentions)'s
    /// short-circuit path (no server exists for this file) and
    /// `request_intentions`'s own merge path (review fix #3 — a server
    /// *does* exist), so the same computation backs both rather than two
    /// copies of it drifting apart.
    ///
    /// Recomputed from the *local* index only, deliberately, the same
    /// tradeoff `build_file_completion`'s first delivery makes: instant,
    /// no network wait for an interactive Alt+Enter. This can disagree
    /// with a squiggle D6's background Central pass already upgraded —
    /// the fix would then offer an older "latest" than the diagnostic's
    /// own message names. Narrow and rare (only in the few seconds between
    /// a file opening and that pass finishing) and left as a known gap
    /// alongside D5's completion's own Central-vs-local sync note, rather
    /// than caching D6's last-published hints on this struct for one
    /// caller.
    pub(crate) fn build_file_quick_fix(
        &self,
        path: &str,
        line: u32,
        character: u32,
    ) -> Option<lsp_core::Intention> {
        if !context::is_build_file(Path::new(path)) {
            return None;
        }
        // Review fix #7: a file this module recognises by name but that
        // D0 never actually registered (the project isn't open yet, or
        // the path fell through some other gate) must not offer a fix —
        // `run_action`'s buffer-vs-disk split (`open_document_paths`)
        // reads `open_docs` directly, so offering one here that D0 never
        // saw would splice nowhere and silently fall back to disk.
        if !self.open_docs.borrow().contains_key(path) {
            return None;
        }
        let content = self
            .session
            .borrow()
            .content_for_path(Path::new(path))
            .unwrap_or_default();
        let caret = caret_byte_offset(&content, line, character);
        let declared = context::declared_versions(Path::new(path), &content);
        let index = local_index_snapshot();
        let hint = declared
            .iter()
            .find(|d| d.range.start <= caret && caret <= d.range.end)
            .and_then(|d| {
                let candidates = index
                    .as_ref()
                    .map(|index| {
                        index
                            .versions(&d.group_id, &d.artifact_id)
                            .into_iter()
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                versions::hint_for(d, &candidates)
            })?;
        Some(lsp_core::Intention {
            item: quick_fix_item(&hint, path, &content),
            group: lsp_core::IntentionGroup::QuickFix,
            preferred: true,
        })
    }

    /// D7's short-circuit branch: `request_intentions` tries this
    /// *before* the language-server path, mirroring C6's
    /// `container_intentions`. Returns `false` — never touching
    /// `self.intentions` — both when there is no hint at all (the file
    /// is not a build file, or nothing is wrong at the caret) and when
    /// one exists but a real server is also configured for this file
    /// (`pom.xml` with lemminx, `build.gradle.kts` with
    /// kotlin-language-server): review fix #3 — short-circuiting there
    /// would silently hide every one of that server's own intentions.
    /// `request_intentions` merges the quick fix back in for that case
    /// itself, via [`build_file_quick_fix`](Self::build_file_quick_fix).
    pub(crate) fn build_file_intentions(
        mut self: Pin<&mut Self>,
        path: &str,
        line: u32,
        character: u32,
    ) -> bool {
        let Some(intention) = self.build_file_quick_fix(path, line, character) else {
            return false;
        };
        if self.config_for_path(path).is_some() {
            return false;
        }
        self.intentions_tracker.borrow_mut().begin();
        *self.intentions.borrow_mut() = vec![intention];
        self.intentions_language.borrow_mut().clear();
        self.as_mut().intentions_ready();
        true
    }
}

/// The synthesised "Update to `latest`" quick fix's `CodeActionItem`
/// (D7): a `WorkspaceEdit` JSON replacing exactly `hint.range` with the
/// new version, in the shape `lsp_core::parse_workspace_edit`'s `changes`
/// branch already reads — `run_action` applies it through the ordinary
/// code-action path, unchanged, since a `CodeActionItem` carrying an
/// `edit` needs no resolve and no server.
fn quick_fix_item(hint: &VersionHint, path: &str, text: &str) -> lsp_core::CodeActionItem {
    let uri = lsp_core::uri_from_path(path);
    let range = byte_range_to_text_range(text, hint.range.clone());
    let edit = serde_json::json!({
        "changes": {
            uri: [{
                "range": {
                    "start": { "line": range.start_line, "character": range.start_character },
                    "end": { "line": range.end_line, "character": range.end_character },
                },
                "newText": hint.latest,
            }]
        }
    });
    lsp_core::CodeActionItem {
        title: format!("Update to {}", hint.latest),
        kind: Some("quickfix".to_string()),
        edit: Some(edit),
        command: None,
        disabled: None,
        raw: serde_json::json!({}),
    }
}

fn local_only_hints(declared: &[DeclaredVersion], index: Option<&RepoIndex>) -> Vec<VersionHint> {
    versions::hints(declared, |group, artifact| {
        index
            .map(|index| {
                index
                    .versions(group, artifact)
                    .into_iter()
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default()
    })
}

fn to_diagnostics(hints: Vec<VersionHint>, text: &str) -> Vec<diagnostics_core::Diagnostic> {
    hints
        .into_iter()
        .map(|hint| {
            let range = byte_range_to_diagnostics_range(text, hint.range.clone());
            diagnostics_core::Diagnostic {
                range,
                severity: diagnostics_core::Severity::Hint,
                message: hint.message(),
                source: "build-tools".to_string(),
                raw: None,
            }
        })
        .collect()
}

fn byte_range_to_diagnostics_range(
    text: &str,
    range: std::ops::Range<usize>,
) -> diagnostics_core::Range {
    let starts = editor_core::offsets::utf16_line_starts(text);
    let start = editor_core::offsets::utf16_offset(text, range.start);
    let end = editor_core::offsets::utf16_offset(text, range.end);
    let (start_line, start_character) = editor_core::offsets::utf16_position_at(&starts, start);
    let (end_line, end_character) = editor_core::offsets::utf16_position_at(&starts, end);
    diagnostics_core::Range {
        start: diagnostics_core::Position {
            line: start_line,
            character: start_character,
        },
        end: Some(diagnostics_core::Position {
            line: end_line,
            character: end_character,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// D7: the synthesised quick fix's edit JSON is exactly what
    /// `lsp_core::parse_workspace_edit`'s `changes` branch already reads —
    /// the same shape `run_action` uses for a real server's edit, proving
    /// this item needs no special-casing anywhere past this point.
    #[test]
    fn quick_fix_item_edit_round_trips_through_parse_workspace_edit() {
        let text = r#"<dependency><version>32.1.3-jre</version></dependency>"#;
        let range = text.find("32.1.3-jre").unwrap()..(text.find("32.1.3-jre").unwrap() + 10);
        let hint = VersionHint {
            range,
            current: "32.1.3-jre".to_string(),
            latest: "33.0.0-jre".to_string(),
        };

        let item = quick_fix_item(&hint, "/proj/pom.xml", text);
        assert_eq!(item.title, "Update to 33.0.0-jre");
        assert_eq!(item.kind.as_deref(), Some("quickfix"));
        assert!(!item.needs_resolve(), "an edit-bearing item never resolves");

        let edit = item.edit.expect("edit present");
        let docs = lsp_core::parse_workspace_edit(&edit).expect("valid WorkspaceEdit JSON");
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].path, "/proj/pom.xml");
        assert_eq!(docs[0].edits.len(), 1);
        assert_eq!(docs[0].edits[0].new_text, "33.0.0-jre");
        // The replaced span covers exactly "32.1.3-jre", not the
        // surrounding `<version>...</version>` tags.
        assert_eq!(docs[0].edits[0].start_character, 21);
        assert_eq!(docs[0].edits[0].end_character, 31);
    }
}
