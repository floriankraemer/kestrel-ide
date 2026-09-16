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
//! Also deliberately not sharing `BuildToolsService`'s own synced index:
//! `editing::repo_index`'s doc comment says that index belongs to "the
//! service that owns the synced `BuildModel`" (B1's `BuildToolsService`),
//! rebuilt once per sync. Wiring that here would mean threading
//! `BuildToolsService` state into a different `#[qobject]`, a bigger
//! change than a first cut of D5 warrants — this module builds and caches
//! its own index instead, off the OS home directory's default cache
//! locations (not the project's `[build_tools.maven].local_repository`/
//! `GRADLE_USER_HOME` override), once per process and never refreshed.
//! Centralising the two is a reasonable follow-up once this path is
//! exercised for real.

use std::path::Path;
use std::pin::Pin;
use std::sync::OnceLock;

use cxx_qt::Threading;

use jvm_build_core::editing::completion::{self, Completion};
use jvm_build_core::editing::context::{self, CoordinatePart, EditContext, TomlLibraryField};
use jvm_build_core::editing::{central, repo_index};

use crate::bridge::ffi;

static REPO_INDEX: OnceLock<repo_index::RepoIndex> = OnceLock::new();

fn local_repo_index() -> &'static repo_index::RepoIndex {
    REPO_INDEX.get_or_init(|| {
        let home = dirs::home_dir().unwrap_or_else(std::env::temp_dir);
        let gradle_user_home = std::env::var("GRADLE_USER_HOME").ok();
        let maven_repo = repo_index::default_maven_repository(None, &home);
        let gradle_modules = repo_index::default_gradle_modules(gradle_user_home.as_deref(), &home);
        repo_index::build(&maven_repo, &gradle_modules)
    })
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
fn all_artifacts() -> Vec<String> {
    let index = local_repo_index();
    index
        .groups()
        .flat_map(|group| index.artifacts(group).map(str::to_string))
        .collect()
}

fn non_empty(value: &Option<String>) -> Option<&str> {
    value.as_deref().filter(|v| !v.is_empty())
}

/// The local index's candidates for whichever part `ctx` names. Provider-
/// agnostic beyond this: `jvm_build_core::editing::completion::items` does
/// the actual ranking against whatever list this (or [`central_candidates`])
/// hands it.
fn local_candidates(ctx: &EditContext) -> Vec<String> {
    let index = local_repo_index();
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
                None => all_artifacts(),
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
                None => all_artifacts(),
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
        if !self
            .completion
            .borrow()
            .needs_request(typed, explicit_request)
        {
            return true;
        }

        *self.completion_language.borrow_mut() = None;
        let token = self.completion.borrow_mut().begin(typed);

        let local = local_candidates(&ctx);
        let items = completion::items(&ctx, &content, &local);
        *self.completions.borrow_mut() = to_completion_list(items, &content);
        self.as_mut().completion_ready();

        let offline = is_offline_for(path);
        if !offline {
            let config_dir = app_core::resolve_config_dir();
            let ctx_for_thread = ctx.clone();
            let content_for_thread = content.clone();
            let qt_thread = self.as_mut().qt_thread();
            std::thread::spawn(move || {
                let client = central::CentralClient::new(&config_dir, false);
                let remote = central_candidates(&ctx_for_thread, &client);
                if remote.is_empty() {
                    return;
                }
                let merged: Vec<String> = local_candidates(&ctx_for_thread)
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
}
