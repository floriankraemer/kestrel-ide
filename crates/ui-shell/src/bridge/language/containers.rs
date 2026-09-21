//! Container assistance in the editor (C6, ADR-0055): image-name
//! completion on a Dockerfile `FROM` / compose `image:` and the "Pull
//! image" intention, injected into `LanguageService`'s completion and
//! intentions paths *before* their language-server gate — a Dockerfile
//! has no server here, and a compose file's YAML server knows nothing
//! about images.
//!
//! Translation only: which line is a `FROM` line, what the prefix is and
//! how candidates rank are `container_core::{image_ref, completion}`;
//! this file wires a worker thread for Docker Hub and re-fires
//! `completionReady` when it answers. A third `impl ffi::LanguageService`
//! block, split out for the file-size ceiling like `lsp_surface.rs`.

use std::collections::HashMap;
use std::path::Path;
use std::pin::Pin;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::{Duration, Instant};

use cxx_qt::{CxxQtThread, Threading};
use cxx_qt_lib::QString;

use container_core::completion::{
    image_completions, registry_match, registry_repo_completions, HubRepo, ImageCompletion,
    ImageCompletionKind,
};
use container_core::image_ref::{self, ImageRef};
use container_registry::hub;
use container_registry::registry::RegistryClient;

use crate::bridge::containers::configured_registries;

use crate::bridge::ffi;

/// How long a Hub answer for one prefix is reused.
const HUB_CACHE_TTL: Duration = Duration::from_secs(60);
/// Keystrokes closer together than this collapse into one Hub request.
const HUB_DEBOUNCE: Duration = Duration::from_millis(200);

/// The intention kind [`ffi::LanguageService::apply_intention`] routes
/// to `containerActionRequested` instead of the language server.
pub(crate) const PULL_INTENTION_KIND: &str = "container.pull";

/// Everything Hub-related `LanguageServiceRust` holds: the cache, the
/// worker's sender, and which request the popup is currently waiting on.
#[derive(Default)]
pub(crate) struct HubState {
    cache: HashMap<String, (Instant, Fetched)>,
    worker: Option<Sender<HubQuery>>,
    /// `(prefix, generation)` of the request whose answer may still refill
    /// the popup; bumped on every new completion so a late answer for an
    /// earlier prefix is dropped.
    pending: Option<(String, u64)>,
    generation: u64,
}

struct HubQuery {
    prefix: String,
    generation: u64,
}

/// What one background lookup answered: Docker Hub's search/tags (the
/// original C6 shape), or a configured registry's repository catalog
/// (C7) once `prefix` names one via `<address>/`.
#[derive(Clone)]
enum Fetched {
    Hub(Vec<HubRepo>),
    Registry { repositories: Vec<String> },
}

/// C7: every configured registry as the `(id, address)` pairs
/// `registry_match`/`registry_repo_completions` take.
fn registry_pairs() -> Vec<(String, String)> {
    configured_registries()
        .into_iter()
        .filter(|setting| setting.kind != "generic") // push-only, never browsable
        .map(|setting| (setting.id, setting.address))
        .collect()
}

/// One background lookup for `prefix`: a configured registry's
/// repositories when `prefix` names one via `<address>/` (C7); otherwise
/// Docker Hub — a tag list when `prefix` names a repository and a `:`, a
/// repository search otherwise (C6).
fn fetch(prefix: &str) -> Fetched {
    let registries = registry_pairs();
    if let Some((id, _, _)) = registry_match(prefix, &registries) {
        let repositories = configured_registries()
            .into_iter()
            .find(|setting| setting.id == id)
            .and_then(|setting| {
                let secret = container_registry::secrets::store()
                    .load(&setting.id)
                    .ok()
                    .flatten()
                    .unwrap_or_default();
                RegistryClient::new(
                    &setting.address,
                    container_core::registry_ref::RegistryKind::from_id(&setting.kind),
                    &setting.username,
                    &secret,
                    &setting.gitlab_project,
                )
                .ok()
            })
            .and_then(|client| client.catalog(100, None).ok())
            .map(|page| page.items)
            .unwrap_or_default();
        return Fetched::Registry { repositories };
    }

    let hub = match prefix.split_once(':') {
        Some((repository, _)) => ImageRef::parse(repository)
            .and_then(|reference| reference.hub_repository())
            .and_then(|repository| hub::tags(&repository).ok())
            .map(|tags| {
                tags.into_iter()
                    .map(|name| HubRepo {
                        name,
                        is_official: false,
                        star_count: 0,
                        description: String::new(),
                    })
                    .collect()
            })
            .unwrap_or_default(),
        None if prefix.is_empty() => Vec::new(),
        None => hub::search(prefix).unwrap_or_default(),
    };
    Fetched::Hub(hub)
}

/// The worker loop: take the newest query, wait [`HUB_DEBOUNCE`] for a
/// newer one to replace it, then fetch and hand the answer back.
fn hub_worker(rx: mpsc::Receiver<HubQuery>, qt_thread: CxxQtThread<ffi::LanguageService>) {
    while let Ok(mut query) = rx.recv() {
        loop {
            match rx.recv_timeout(HUB_DEBOUNCE) {
                Ok(newer) => query = newer,
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
        let repos = fetch(&query.prefix);
        let queued = qt_thread.queue(move |service: Pin<&mut ffi::LanguageService>| {
            service.hub_answered(query.prefix, query.generation, repos);
        });
        if queued.is_err() {
            return;
        }
    }
}

fn to_item(
    completion: ImageCompletion,
    line: u32,
    start_character: u32,
    end_character: u32,
) -> lsp_core::CompletionItem {
    // LSP `CompletionItemKind`: Module (9) for a repository, Constant (21)
    // for a local image, EnumMember (20) for a tag — the icons the popup
    // already has, so a Hub hit and a local image read differently.
    let kind = match completion.kind {
        ImageCompletionKind::Local => 21,
        ImageCompletionKind::Official
        | ImageCompletionKind::Community
        | ImageCompletionKind::Registry => 9,
        ImageCompletionKind::Tag => 20,
    };
    lsp_core::CompletionItem {
        label: completion.label,
        insert: completion.insert,
        kind: Some(kind),
        detail: completion.detail,
        documentation: String::new(),
        sort_text: None,
        filter_text: None,
        is_snippet: false,
        deprecated: false,
        range: Some(lsp_core::TextRange {
            start_line: line,
            start_character,
            end_line: line,
            end_character,
        }),
        raw: serde_json::Value::Null,
    }
}

impl ffi::LanguageService {
    /// Whether `path` is a file whose `FROM`/`image:` lines this module
    /// assists in, and which of the two it is.
    fn container_file_kind(path: &str) -> Option<bool> {
        let path = Path::new(path);
        if syntax_core::language_for_path(path).id() == "dockerfile" {
            return Some(true);
        }
        container_core::compose_file::is_compose_file(path).then_some(false)
    }

    /// The image-name completion branch of `completionAt` (C6). `true`
    /// when the caret is on an image reference in a Dockerfile/compose
    /// file and the popup has been filled — the caller stops there; `false`
    /// hands the request on to the language server / fallback path.
    pub(crate) fn container_completion(
        mut self: Pin<&mut Self>,
        path: &str,
        line: u32,
        character: u32,
        text_before_cursor: &str,
    ) -> bool {
        let Some(is_dockerfile) = Self::container_file_kind(path) else {
            return false;
        };
        let Some(context) = image_ref::completion_context(text_before_cursor, is_dockerfile) else {
            return false;
        };
        *self.completion_language.borrow_mut() = None;
        self.completion
            .borrow_mut()
            .begin(lsp_core::completion_prefix(text_before_cursor));

        let generation = {
            let mut hub_state = self.hub.borrow_mut();
            hub_state.generation += 1;
            let generation = hub_state.generation;
            hub_state.pending = Some((context.prefix.clone(), generation));
            generation
        };
        let start_character =
            character.saturating_sub(context.prefix.encode_utf16().count() as u32);
        *self.container_completion_span.borrow_mut() = (line, start_character, character);

        let cached = self.cached_fetch(&context.prefix);
        self.as_mut()
            .fill_image_completions(&context.prefix, cached.as_ref());
        if cached.is_none() {
            self.as_mut().ask_hub(HubQuery {
                prefix: context.prefix,
                generation,
            });
        }
        true
    }

    fn cached_fetch(&self, prefix: &str) -> Option<Fetched> {
        self.hub
            .borrow()
            .cache
            .get(prefix)
            .filter(|(fetched_at, _)| fetched_at.elapsed() < HUB_CACHE_TTL)
            .map(|(_, fetched)| fetched.clone())
    }

    fn ask_hub(mut self: Pin<&mut Self>, query: HubQuery) {
        let qt_thread = self.as_mut().qt_thread();
        let mut hub_state = self.hub.borrow_mut();
        let sender = hub_state.worker.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel();
            std::thread::spawn(move || hub_worker(rx, qt_thread));
            tx
        });
        if sender.send(query).is_err() {
            hub_state.worker = None;
        }
    }

    /// A background lookup landed on the Qt thread: cache it, and refill
    /// the popup only if it is still waiting on exactly this prefix.
    fn hub_answered(self: Pin<&mut Self>, prefix: String, generation: u64, fetched: Fetched) {
        let still_wanted = {
            let mut hub_state = self.hub.borrow_mut();
            hub_state
                .cache
                .insert(prefix.clone(), (Instant::now(), fetched.clone()));
            hub_state.pending == Some((prefix.clone(), generation))
        };
        if still_wanted {
            self.fill_image_completions(&prefix, Some(&fetched));
        }
    }

    fn fill_image_completions(mut self: Pin<&mut Self>, prefix: &str, fetched: Option<&Fetched>) {
        let local = self.local_image_names();
        let (line, start_character, end_character) = *self.container_completion_span.borrow();
        let mut candidates = match fetched {
            Some(Fetched::Hub(hub)) => image_completions(prefix, &local, Some(hub)),
            None => image_completions(prefix, &local, None),
            Some(Fetched::Registry { .. }) => image_completions(prefix, &local, None),
        };
        if let Some(Fetched::Registry { repositories }) = fetched {
            if let Some((_, address, rest)) = registry_match(prefix, &registry_pairs()) {
                candidates.extend(registry_repo_completions(address, &rest, repositories));
            }
        }
        let items = candidates
            .into_iter()
            .map(|completion| to_item(completion, line, start_character, end_character))
            .collect();
        *self.completions.borrow_mut() = lsp_core::CompletionList {
            items,
            is_incomplete: fetched.is_none(),
        };
        self.as_mut().completion_ready();
    }

    /// The image names the view last handed over (`setLocalImages`, fed
    /// from `ContainerService::treeChanged`) — the local half of the
    /// ranking, without this QObject reaching into another one.
    fn local_image_names(&self) -> Vec<String> {
        self.local_images.borrow().clone()
    }

    /// C6: the connected engines' image names, `\n`-joined as every other
    /// list on this seam is, pushed by the view whenever the Containers
    /// tree changes.
    pub fn set_local_images(self: Pin<&mut Self>, names: &QString) {
        *self.local_images.borrow_mut() = names
            .to_string()
            .lines()
            .filter(|name| !name.is_empty())
            .map(str::to_string)
            .collect();
    }

    /// The "Pull image" branch of `requestIntentions` (C6). `true` when
    /// the caret is on an image reference and the intentions list has
    /// been filled with the one action; `false` hands on to the server.
    pub(crate) fn container_intentions(
        mut self: Pin<&mut Self>,
        path: &str,
        line: u32,
        character: u32,
    ) -> bool {
        if Self::container_file_kind(path).is_none() {
            return false;
        }
        let content = self
            .session
            .borrow()
            .content_for_path(Path::new(path))
            .unwrap_or_default();
        let Some(line_text) = content.lines().nth(line as usize) else {
            return false;
        };
        let byte = editor_core::offsets::byte_offset(line_text, character as usize);
        let column = line_text[..byte].chars().count();
        let Some((reference, _)) = image_ref::image_ref_at(line_text, column) else {
            return false;
        };
        let reference = reference.to_string();
        self.intentions_tracker.borrow_mut().begin();
        *self.intentions.borrow_mut() = vec![lsp_core::Intention {
            item: lsp_core::CodeActionItem {
                title: format!("Pull image {reference}"),
                kind: Some(PULL_INTENTION_KIND.to_string()),
                edit: None,
                command: None,
                disabled: None,
                raw: serde_json::json!({ "image": reference }),
            },
            group: lsp_core::IntentionGroup::Other,
            preferred: false,
        }];
        self.intentions_language.borrow_mut().clear();
        self.as_mut().intentions_ready();
        true
    }

    /// The `container.*` branch of `applyIntention`: no server involved —
    /// the view wires `containerActionRequested` to `ContainerService`.
    /// `true` when `action` was one of ours.
    pub(crate) fn apply_container_intention(
        mut self: Pin<&mut Self>,
        action: &lsp_core::CodeActionItem,
    ) -> bool {
        let Some(kind) = action
            .kind
            .as_deref()
            .filter(|k| k.starts_with("container."))
        else {
            return false;
        };
        let payload = action
            .raw
            .get("image")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        self.as_mut()
            .container_action_requested(QString::from(kind), QString::from(payload));
        true
    }
}
