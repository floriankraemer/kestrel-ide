use core::pin::Pin;

use cxx_qt::Threading;
use cxx_qt_lib::{QString, QStringList};

use crate::bridge::convert::{
    search_options, symbol_kind_word, to_ffi_refusal, to_ffi_resolution_tier, to_ffi_symbol_match,
};
use crate::bridge::ffi::{self};
use crate::bridge::registry::index_slot;

/// Rust side of the `SearchModel` QObject (Task H). The index itself lives
/// behind an `RwLock` (not the `RefCell` the other adapters use) because,
/// unlike `AppSession`, it is genuinely accessed from background threads —
/// the Qt-thread invokables below only ever clone the `Arc` and hand it off.
/// A read lock is enough for every query, so several searches can run at
/// once and only re-indexing serialises them.
pub struct SearchModelRust {
    index: mcp_server::IndexHandle,
    /// RF12: the index leg of hover is a second round trip that
    /// `LanguageService`'s tracker cannot see, so it needs its own. The rule
    /// is `lsp_core::HoverTracker`'s; only its state lives here.
    hover: std::cell::RefCell<lsp_core::HoverTracker>,
    /// RF9: the name-based rename waiting for the preview's verdict, and the
    /// name it would write. Kept so the view can read the sites back rather
    /// than being handed them in a signal, the same shape completion uses.
    rename: std::cell::RefCell<Option<(index_core::IndexRenamePlan, String)>>,
    context: std::sync::Arc<std::sync::Mutex<SearchContext>>,
    /// Separate query guards so the popup and the results panel never
    /// cancel each other.
    everywhere: std::sync::Arc<QueryGuard>,
    find_in_files: std::sync::Arc<QueryGuard>,
    /// F3-15: the last `previewReplacements` answer, read back one file at a
    /// time by `replacePreviewDiff`/`Hunks`/`Spans` as the dialog's
    /// selection changes — the same "compute once, fetch lazily" shape
    /// `LanguageService::pending` already uses for the refactor preview.
    replace_preview: std::cell::RefCell<Vec<index_core::FileDiffPreview>>,
}

impl Default for SearchModelRust {
    fn default() -> Self {
        SearchModelRust {
            // Not a fresh slot: the MCP server queries the same index this
            // QObject builds, and cxx-qt constructs QObjects via `Default`
            // with no way to inject a shared handle. Same reasoning as the
            // `APP_SESSION` thread-local above — one project, one index.
            index: index_slot(),
            hover: Default::default(),
            rename: Default::default(),
            context: Default::default(),
            everywhere: Default::default(),
            find_in_files: Default::default(),
            replace_preview: Default::default(),
        }
    }
}

/// The non-index inputs Search Everywhere's cheap tiers need. Cached here
/// rather than re-read from `settings.toml` per keystroke.
#[derive(Default)]
struct SearchContext {
    recent_files: Vec<std::path::PathBuf>,
    keymap: app_config::keymap::Keymap,
}

/// Search-as-you-type bookkeeping for one query stream: which generation the
/// view is currently waiting for, and the flag that tells the running worker
/// to stop scanning because a newer keystroke superseded it.
#[derive(Default)]
struct QueryGuard {
    generation: std::sync::atomic::AtomicU64,
    cancel: std::sync::Mutex<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl QueryGuard {
    /// Cancel whatever is running and take ownership of `generation`,
    /// returning the fresh cancellation flag for the new worker. The old
    /// worker keeps its own (now-raised) flag, so it stops without the new
    /// one ever seeing a cancellation meant for its predecessor.
    fn begin(&self, generation: u64) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        use std::sync::atomic::Ordering;
        self.generation.store(generation, Ordering::SeqCst);
        let mut slot = self.cancel.lock().unwrap();
        slot.store(true, Ordering::Relaxed);
        let fresh = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        *slot = std::sync::Arc::clone(&fresh);
        fresh
    }

    /// Whether `generation` is still the one the view wants.
    fn is_current(&self, generation: u64) -> bool {
        self.generation.load(std::sync::atomic::Ordering::SeqCst) == generation
    }
}

/// Hits per `searchBatch` emission. Find in Files can produce tens of
/// thousands of matches and one signal per match is one cross-thread hop
/// per match; batching turns that into a handful of hops.
const SEARCH_BATCH_SIZE: usize = 256;

/// Ceiling on Find-in-Files matches. A three-character query on a large repo
/// can match more lines than any human will scroll.
// ponytail: a fixed ceiling with no "showing first N of M" affordance in the
// results panel — add a count and a "load more" if users hit it in anger.
const MAX_FIND_IN_FILES_MATCHES: usize = 10_000;

/// Build one Search Everywhere row.
fn hit(kind: ffi::FfiHitKind, text: &str, detail: &str, positions: Vec<u32>) -> ffi::FfiSearchHit {
    ffi::FfiSearchHit {
        kind,
        path: QString::from(""),
        line: 0,
        start: 0,
        end: 0,
        text: QString::from(text),
        detail: QString::from(detail),
        action_id: QString::from(""),
        positions,
    }
}

/// Human label for a symbol hit's secondary column.
fn symbol_detail(m: &index_core::SymbolMatch) -> String {
    let kind = symbol_kind_word(m.kind);
    match &m.container {
        Some(container) => format!("{kind} in {container}"),
        None => kind.to_string(),
    }
}

/// Turn one text match into a `FfiHitKind::Text` row, shared by Search
/// Everywhere's text tier and Find in Files so both render identically.
fn text_hit(m: index_core::SearchMatch, root: &std::path::Path) -> ffi::FfiSearchHit {
    let path = m.path.to_string_lossy().into_owned();
    let trimmed = m.line_text.trim_start();
    // The trim shifts the match span, which the view highlights.
    let shift = m.line_text.len() - trimmed.len();
    let display = trimmed.trim_end();
    let start = m.start.saturating_sub(shift);
    let end = m.end.saturating_sub(shift);
    // The view highlights character offsets, not byte offsets — they only
    // agree on ASCII lines.
    let positions: Vec<u32> = display
        .char_indices()
        .enumerate()
        .filter(|(_, (byte, _))| *byte >= start && *byte < end)
        .map(|(index, _)| index as u32)
        .collect();
    let relative = m
        .path
        .strip_prefix(root)
        .unwrap_or(&m.path)
        .to_string_lossy()
        .into_owned();
    ffi::FfiSearchHit {
        kind: ffi::FfiHitKind::Text,
        path: QString::from(path.as_str()),
        line: m.line as u32,
        start: start as u32,
        end: end as u32,
        text: QString::from(display),
        detail: QString::from(format!("{relative}:{}", m.line).as_str()),
        action_id: QString::from(""),
        positions,
    }
}

/// Shortest gap between two `indexProgress` emissions. A status bar cannot
/// show more than a few updates a second anyway, and each one is a
/// cross-thread hop.
const PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(100);

impl ffi::SearchModel {
    pub fn open_index(self: Pin<&mut Self>, root_path: &QString) {
        let root = std::path::PathBuf::from(root_path.to_string());
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        self.as_ref().load_context();
        // Marked as building *before* the worker starts, so a query fired in
        // the seconds-to-minutes window between Open Folder and `indexReady`
        // is answered with "still building" rather than "no project open".
        {
            let mut current = slot.write().unwrap();
            // A second open for the project already being indexed would race
            // its own worker for tantivy's one-writer-per-directory lock and
            // fail with `LockBusy`; the build in flight is the one to wait for.
            if matches!(&*current, index_core::IndexSlot::Building(building) if building == &root) {
                return;
            }
            *current = index_core::IndexSlot::Building(root.clone());
        }
        std::thread::spawn(move || {
            // Read on the worker thread rather than the Qt thread (F0-freeze
            // fix, ADR-0037): the settings layers are files, and reading
            // them for `root` explicitly (`load_resolved_settings_for`,
            // rather than `load_resolved_settings`'s "whatever project is
            // currently open") is what makes that safe off the Qt thread —
            // the resolved answer still belongs to the project being
            // opened, not to whatever the shared session has open when the
            // walk reaches a directory. Which layer the patterns came from
            // is `settings_model::scope`'s answer (ADR-0022); all
            // `index-core` gets is the list.
            let options = index_core::IndexOptions {
                excludes: crate::bridge::convert::load_resolved_settings_for(&root).index_excludes,
            };
            // One cross-thread hop per file would cost more than the file
            // took to index, so the closure reports at most every
            // `PROGRESS_INTERVAL` — plus the first report of a pass (which
            // carries the total) and the last (which says it is done).
            let last = std::sync::Mutex::new(None::<std::time::Instant>);
            let progress_thread = qt_thread.clone();
            let progress = move |p: index_core::IndexProgress| {
                {
                    let mut last = last.lock().unwrap();
                    let due = match *last {
                        None => true,
                        Some(at) => at.elapsed() >= PROGRESS_INTERVAL,
                    };
                    if !due && p.done != p.total {
                        return;
                    }
                    *last = Some(std::time::Instant::now());
                }
                let (done, total) = (p.done as u32, p.total as u32);
                let _ = progress_thread.queue(move |mut model: Pin<&mut Self>| {
                    model.as_mut().index_progress(done, total);
                });
            };
            match index_core::TextIndex::open_or_build_with_progress(&root, &options, &progress) {
                Ok(index) => {
                    *slot.write().unwrap() = index_core::IndexSlot::Ready(Box::new(index));
                    let _ = qt_thread.queue(|mut model: Pin<&mut Self>| {
                        model.as_mut().index_ready();
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    *slot.write().unwrap() = index_core::IndexSlot::Failed(message.clone());
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().index_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    pub fn sync_indexed_files(self: Pin<&mut Self>, paths: &QStringList) {
        let paths: Vec<std::path::PathBuf> = paths
            .iter()
            .map(|p| std::path::PathBuf::from(p.to_string()))
            .collect();
        if paths.is_empty() {
            return;
        }
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            if let Some(index) = slot.write().unwrap().ready_mut() {
                // A path that vanished or became unreadable is dropped by
                // `sync_paths` itself, so there is nothing to report here.
                let _ = index.sync_paths(&paths);
            }
        });
    }

    pub fn reindex_file(self: Pin<&mut Self>, path: &QString) {
        let path = std::path::PathBuf::from(path.to_string());
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            if let Some(index) = slot.write().unwrap().ready_mut() {
                // A file that became unreadable is dropped from the index by
                // `reindex_file` itself, so there is nothing to report here.
                let _ = index.reindex_file(&path);
            }
        });
    }

    pub fn remove_indexed_file(self: Pin<&mut Self>, path: &QString) {
        let path = std::path::PathBuf::from(path.to_string());
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            if let Some(index) = slot.write().unwrap().ready_mut() {
                let _ = index.remove_file(&path);
            }
        });
    }

    pub fn note_recent_file(self: Pin<&mut Self>, path: &QString) {
        let path = std::path::PathBuf::from(path.to_string());
        {
            let mut context = self.context.lock().unwrap();
            context.recent_files.retain(|p| p != &path);
            context.recent_files.insert(0, path.clone());
        }
        let config_dir = app_core::resolve_config_dir();
        let Ok(mut settings) = app_config::load(&config_dir) else {
            return;
        };
        settings.push_recent_file(path);
        let _ = app_config::save(&config_dir, &settings);
    }

    pub fn refresh_keymap(self: Pin<&mut Self>) {
        self.as_ref().load_context();
    }

    /// Re-read the settings-derived half of the search context (recent
    /// files, keymap) so the cheap tiers answer from memory.
    fn load_context(&self) {
        let Ok(settings) = app_config::load(&app_core::resolve_config_dir()) else {
            return;
        };
        let mut context = self.context.lock().unwrap();
        context.keymap = settings.keymap();
        context.recent_files = settings.recent_files.clone();
    }

    pub fn search_everywhere(
        self: Pin<&mut Self>,
        query: &QString,
        tiers: ffi::FfiTierFilter,
        generation: u64,
        limit: u32,
    ) {
        let query = query.to_string();
        let limit = (limit as usize).max(1);
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        let context = std::sync::Arc::clone(&self.context);
        let guard = std::sync::Arc::clone(&self.everywhere);
        let cancel = guard.begin(generation);

        std::thread::spawn(move || {
            use std::sync::atomic::Ordering;

            let wanted =
                |tier: ffi::FfiTierFilter| tiers == ffi::FfiTierFilter::All || tiers == tier;
            let superseded = || cancel.load(Ordering::Relaxed) || !guard.is_current(generation);
            let emit = |hits: Vec<ffi::FfiSearchHit>| {
                if hits.is_empty() {
                    return;
                }
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model.as_mut().results_batch(generation, hits);
                });
            };
            let finish = || {
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model.as_mut().query_finished(generation);
                });
            };

            // Recent files answer from memory, so the empty-query landing
            // view paints before any index work starts. Once there is a query
            // the file tier covers them, ranked.
            if query.is_empty() && wanted(ffi::FfiTierFilter::Files) {
                let context = context.lock().unwrap();
                emit(
                    context
                        .recent_files
                        .iter()
                        .take(limit)
                        .map(|path| {
                            let display = path.to_string_lossy().into_owned();
                            let name = path
                                .file_name()
                                .map(|n| n.to_string_lossy().into_owned())
                                .unwrap_or_else(|| display.clone());
                            let mut row =
                                hit(ffi::FfiHitKind::RecentFile, &name, &display, Vec::new());
                            row.path = QString::from(display.as_str());
                            row
                        })
                        .collect(),
                );
            }

            if superseded() {
                finish();
                return;
            }

            let index_guard = slot.read().unwrap();
            let Some(index) = index_guard.ready() else {
                let reason = index_guard.unavailable_reason().unwrap_or_default();
                drop(index_guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model
                        .as_mut()
                        .query_failed(generation, QString::from(reason.as_str()));
                });
                return;
            };

            if wanted(ffi::FfiTierFilter::Files) {
                emit(
                    index
                        .find_files(&query, limit)
                        .into_iter()
                        .map(|m| {
                            let mut row = hit(ffi::FfiHitKind::File, &m.relative, "", m.positions);
                            row.path = QString::from(m.path.to_string_lossy().as_ref());
                            row
                        })
                        .collect(),
                );
            }

            if superseded() {
                drop(index_guard);
                finish();
                return;
            }

            if !query.is_empty() && wanted(ffi::FfiTierFilter::Symbols) {
                // ponytail: symbol rows carry no highlight positions —
                // `find_definitions_ranked` scores without reporting match
                // indices. Thread them through if the visual inconsistency
                // with the file tier starts to show.
                if let Ok(symbols) = index.find_definitions_ranked(&query, limit) {
                    emit(
                        symbols
                            .into_iter()
                            .map(|m| {
                                let detail = symbol_detail(&m);
                                let mut row =
                                    hit(ffi::FfiHitKind::Symbol, &m.name, &detail, Vec::new());
                                row.path = QString::from(m.path.to_string_lossy().as_ref());
                                row.line = m.line as u32;
                                row
                            })
                            .collect(),
                    );
                }
            }

            if superseded() {
                drop(index_guard);
                finish();
                return;
            }

            // Actions also answer from memory, but rank below the project's
            // own files and symbols: a query is far more often about the code
            // than about a command. An empty query in the All tab is the
            // recent-files landing view, so the whole command list stays out
            // of it — browsing commands is what the Actions tab is for.
            if wanted(ffi::FfiTierFilter::Actions)
                && (!query.is_empty() || tiers == ffi::FfiTierFilter::Actions)
            {
                let context = context.lock().unwrap();
                emit(
                    app_config::keymap::search_actions(&query, &context.keymap, limit)
                        .into_iter()
                        .map(|m| {
                            let label = format!("{}: {}", m.action.category, m.action.label);
                            let mut row =
                                hit(ffi::FfiHitKind::Action, &label, &m.shortcut, m.positions);
                            row.action_id = QString::from(m.action.id);
                            row
                        })
                        .collect(),
                );
            }

            if superseded() {
                drop(index_guard);
                finish();
                return;
            }

            // A one-character query would scan the whole project for
            // something every file contains; the other tiers already answer
            // it usefully.
            if query.chars().count() >= 2 && wanted(ffi::FfiTierFilter::Text) {
                if let Ok(matches) = index.search_with(&query, false, false, limit, &cancel) {
                    let root = index.root().to_path_buf();
                    emit(matches.into_iter().map(|m| text_hit(m, &root)).collect());
                }
            }

            drop(index_guard);
            finish();
        });
    }

    pub fn search(
        self: Pin<&mut Self>,
        pattern: &QString,
        is_regex: bool,
        case_sensitive: bool,
        generation: u64,
    ) {
        let pattern = pattern.to_string();
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        let guard = std::sync::Arc::clone(&self.find_in_files);
        let cancel = guard.begin(generation);

        std::thread::spawn(move || {
            let index_guard = slot.read().unwrap();
            let Some(index) = index_guard.ready() else {
                let reason = index_guard.unavailable_reason().unwrap_or_default();
                drop(index_guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model
                        .as_mut()
                        .search_failed(generation, QString::from(reason.as_str()));
                });
                return;
            };
            let result = index.search_with(
                &pattern,
                is_regex,
                case_sensitive,
                MAX_FIND_IN_FILES_MATCHES,
                &cancel,
            );
            let root = index.root().to_path_buf();
            drop(index_guard);

            match result {
                Ok(matches) => {
                    for chunk in matches.chunks(SEARCH_BATCH_SIZE) {
                        if !guard.is_current(generation) {
                            break;
                        }
                        let hits: Vec<ffi::FfiSearchHit> =
                            chunk.iter().cloned().map(|m| text_hit(m, &root)).collect();
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model.as_mut().search_batch(generation, hits);
                        });
                    }
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().search_finished(generation);
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .search_failed(generation, QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    pub fn hover_signature(
        mut self: Pin<&mut Self>,
        path: &QString,
        content: &QString,
        byte_offset: usize,
    ) {
        let path = std::path::PathBuf::from(path.to_string());
        let content = content.to_string();
        let token = self.hover.borrow_mut().begin();
        let qt_thread = self.as_mut().qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let guard = slot.read().unwrap();
            // Without a built index the buffer alone still resolves a
            // same-file declaration, which is most of what a hover asks
            // about — the same reasoning `resolve_declaration` uses.
            let resolution = match guard.ready() {
                Some(index) => index.resolve_declaration(&path, &content, byte_offset).ok(),
                None => Some(index_core::resolve_declaration_in_buffer(
                    &path,
                    &content,
                    byte_offset,
                )),
            };
            drop(guard);
            let Some(target) = resolution.and_then(|r| r.candidates.into_iter().next()) else {
                return;
            };
            let Some(signature) = index_core::declaration_signature(&target.path, target.line)
            else {
                return;
            };
            // Rendered through the tooltip path the server answers use, as
            // plain text: a declaration is source, not Markdown, and must
            // not have its punctuation reinterpreted.
            let html = lsp_core::to_tooltip_html(&lsp_core::HoverText {
                value: signature,
                markdown: false,
            });
            let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                if model.hover.borrow().accept(token) {
                    model
                        .as_mut()
                        .hover_signature_ready(QString::from(html.as_str()));
                }
            });
        });
    }

    pub fn cancel_hover_signature(self: Pin<&mut Self>) {
        self.hover.borrow_mut().cancel();
    }

    pub fn plan_index_rename(
        self: Pin<&mut Self>,
        path: &QString,
        content: &QString,
        byte_offset: usize,
        new_name: &QString,
        has_unsaved_changes: bool,
    ) {
        let path = std::path::PathBuf::from(path.to_string());
        let content = content.to_string();
        let new_name = new_name.to_string();
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let guard = slot.read().unwrap();
            let Some(index) = guard.ready() else {
                let reason = guard.unavailable_reason().unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model.as_mut().index_rename_failed(
                        ffi::FfiRenameRefusal::Unavailable,
                        QString::from(reason.as_str()),
                    );
                });
                return;
            };
            let planned = index
                .resolve_declaration(&path, &content, byte_offset)
                .and_then(|resolution| {
                    let usages = index.find_usages(&resolution.name)?;
                    let definitions = index.find_definitions_exact(&resolution.name)?;
                    Ok(index_core::plan_index_rename(
                        &resolution,
                        &usages,
                        &definitions,
                        &new_name,
                        has_unsaved_changes,
                    ))
                });
            drop(guard);
            match planned {
                Ok(Ok(plan)) => {
                    let name = QString::from(plan.name.as_str());
                    let ambiguous = plan.ambiguous;
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        *model.rename.borrow_mut() = Some((plan, new_name));
                        model.as_mut().index_rename_ready(name, ambiguous);
                    });
                }
                // A refusal and a failed lookup reach the user the same way:
                // as a sentence saying why nothing will happen.
                Ok(Err(refusal)) => {
                    let message = refusal.to_string();
                    let reason = to_ffi_refusal(&refusal);
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .index_rename_failed(reason, QString::from(message.as_str()));
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().index_rename_failed(
                            ffi::FfiRenameRefusal::Unavailable,
                            QString::from(message.as_str()),
                        );
                    });
                }
            }
        });
    }

    pub fn index_rename_sites(&self) -> Vec<ffi::FfiRenameSite> {
        let borrowed = self.rename.borrow();
        let Some((plan, _)) = borrowed.as_ref() else {
            return Vec::new();
        };
        plan.sites
            .iter()
            .map(|site| ffi::FfiRenameSite {
                path: QString::from(site.path.to_string_lossy().as_ref()),
                line: site.line as u32,
                col: site.col as u32,
                resolved: site.confidence == index_core::SiteConfidence::Resolved,
                is_definition: site.is_definition,
                checked: site.checked,
            })
            .collect()
    }

    pub fn exclude_from_index_rename(self: Pin<&mut Self>, path: &QString) {
        let path = std::path::PathBuf::from(path.to_string());
        if let Some((plan, _)) = self.rename.borrow_mut().as_mut() {
            for site in plan.sites.iter_mut().filter(|site| site.path == path) {
                site.checked = false;
            }
        }
    }

    pub fn take_index_rename_buffer_edits(
        self: Pin<&mut Self>,
        path: &QString,
    ) -> Vec<ffi::FfiTextEdit> {
        let path = std::path::PathBuf::from(path.to_string());
        let mut borrowed = self.rename.borrow_mut();
        let Some((plan, new_name)) = borrowed.as_mut() else {
            return Vec::new();
        };
        index_core::take_buffer_edits(plan, new_name, &path)
            .into_iter()
            .map(|edit| ffi::FfiTextEdit {
                path: QString::from(edit.path.to_string_lossy().as_ref()),
                in_buffer: true,
                start_line: edit.line,
                start_character: edit.start_character,
                end_line: edit.line,
                end_character: edit.end_character,
                new_text: QString::from(edit.text.as_str()),
            })
            .collect()
    }

    pub fn apply_index_rename(mut self: Pin<&mut Self>) {
        let Some((plan, new_name)) = self.rename.borrow_mut().take() else {
            self.as_mut().refactor_files_finished(0, 0);
            return;
        };
        let edits = index_core::rename_replacements(&plan, &new_name);
        if edits.is_empty() {
            self.as_mut().refactor_files_finished(0, 0);
            return;
        }

        let qt_thread = self.as_mut().qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let mut guard = slot.write().unwrap();
            let reason = guard.unavailable_reason();
            let Some(index) = guard.ready_mut() else {
                let reason = reason.unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model
                        .as_mut()
                        .refactor_files_failed(QString::from(reason.as_str()));
                });
                return;
            };
            let result = index.replace_in_files(&edits);
            drop(guard);
            match result {
                Ok(report) => {
                    let files = report.files as u32;
                    let skipped = report.skipped_files as u32;
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().refactor_files_finished(files, skipped);
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .refactor_files_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    pub fn apply_file_edits(mut self: Pin<&mut Self>, edits: Vec<ffi::FfiTextEdit>) {
        // Group by file, keeping each document's order — `lsp_core` already
        // sorted the edits last-first, which is what makes applying them in
        // one pass correct.
        let mut by_path: Vec<(String, Vec<lsp_core::TextEdit>)> = Vec::new();
        for edit in edits.iter().filter(|edit| !edit.in_buffer) {
            let path = edit.path.to_string();
            let converted = lsp_core::TextEdit {
                start_line: edit.start_line,
                start_character: edit.start_character,
                end_line: edit.end_line,
                end_character: edit.end_character,
                new_text: edit.new_text.to_string(),
            };
            match by_path.iter_mut().find(|(known, _)| *known == path) {
                Some((_, edits)) => edits.push(converted),
                None => by_path.push((path, vec![converted])),
            }
        }
        if by_path.is_empty() {
            self.as_mut().refactor_files_finished(0, 0);
            return;
        }

        let qt_thread = self.as_mut().qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let mut guard = slot.write().unwrap();
            let reason = guard.unavailable_reason();
            let Some(index) = guard.ready_mut() else {
                let reason = reason.unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model
                        .as_mut()
                        .refactor_files_failed(QString::from(reason.as_str()));
                });
                return;
            };

            // A file that cannot be read, or whose text the edits no longer
            // fit, is skipped whole — never half-written. The same rule
            // `replace_in_files` applies to a span it can no longer place.
            let mut skipped = 0u32;
            let mut rewritten = Vec::new();
            for (path, edits) in by_path {
                let path = std::path::PathBuf::from(path);
                let Ok(text) = std::fs::read_to_string(&path) else {
                    skipped += 1;
                    continue;
                };
                match lsp_core::apply_to_text(&text, &edits) {
                    Ok(new_text) => rewritten.push((path, new_text)),
                    Err(_) => skipped += 1,
                }
            }
            let result = index.write_files(&rewritten);
            drop(guard);
            match result {
                Ok(report) => {
                    let skipped = skipped + report.skipped_files as u32;
                    let files = report.files as u32;
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().refactor_files_finished(files, skipped);
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .refactor_files_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    pub fn replace_in_files(
        self: Pin<&mut Self>,
        edits: Vec<ffi::FfiFileReplacement>,
        pattern: &QString,
        replacement: &QString,
        is_regex: bool,
        case_sensitive: bool,
    ) {
        let pattern = pattern.to_string();
        let replacement = replacement.to_string();
        let opts = search_options(is_regex, case_sensitive);
        let edits: Vec<(std::path::PathBuf, usize, usize, usize)> = edits
            .into_iter()
            .map(|e| {
                (
                    std::path::PathBuf::from(e.path.to_string()),
                    e.line as usize,
                    e.start as usize,
                    e.end as usize,
                )
            })
            .collect();
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let mut guard = slot.write().unwrap();
            let reason = guard.unavailable_reason();
            let Some(index) = guard.ready_mut() else {
                let reason = reason.unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model
                        .as_mut()
                        .replace_failed(QString::from(reason.as_str()));
                });
                return;
            };

            let resolved =
                match index_core::resolve_replacements(&edits, &pattern, &replacement, opts) {
                    Ok(resolved) => resolved,
                    Err(message) => {
                        drop(guard);
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model
                                .as_mut()
                                .replace_failed(QString::from(message.as_str()));
                        });
                        return;
                    }
                };
            let result = index.replace_in_files(&resolved);
            drop(guard);
            match result {
                Ok(report) => {
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().replace_finished(
                            report.files as u32,
                            report.matches as u32,
                            report.skipped_files as u32,
                        );
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .replace_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    /// [`Self::replace_in_files`], without writing: same span resolution,
    /// `index_core::TextIndex::preview_replacements` instead of
    /// `replace_in_files`. The result is cached in `replace_preview` for
    /// `replacePreviewDiff`/`Hunks`/`Spans` to read back per file once the
    /// dialog is up.
    pub fn preview_replacements(
        self: Pin<&mut Self>,
        edits: Vec<ffi::FfiFileReplacement>,
        pattern: &QString,
        replacement: &QString,
        is_regex: bool,
        case_sensitive: bool,
    ) {
        let pattern = pattern.to_string();
        let replacement = replacement.to_string();
        let opts = search_options(is_regex, case_sensitive);
        let edits: Vec<(std::path::PathBuf, usize, usize, usize)> = edits
            .into_iter()
            .map(|e| {
                (
                    std::path::PathBuf::from(e.path.to_string()),
                    e.line as usize,
                    e.start as usize,
                    e.end as usize,
                )
            })
            .collect();
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let guard = slot.read().unwrap();
            let Some(index) = guard.ready() else {
                let reason = guard.unavailable_reason().unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model
                        .as_mut()
                        .replace_preview_failed(QString::from(reason.as_str()));
                });
                return;
            };

            let resolved =
                match index_core::resolve_replacements(&edits, &pattern, &replacement, opts) {
                    Ok(resolved) => resolved,
                    Err(message) => {
                        drop(guard);
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model
                                .as_mut()
                                .replace_preview_failed(QString::from(message.as_str()));
                        });
                        return;
                    }
                };
            let previews = index.preview_replacements(&resolved);
            drop(guard);
            let paths: Vec<String> = previews
                .iter()
                .map(|p| p.path.display().to_string())
                .collect();
            let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                *model.replace_preview.borrow_mut() = previews;
                let list: QStringList = paths.iter().map(|p| QString::from(p.as_str())).collect();
                model.as_mut().replace_preview_ready(list);
            });
        });
    }

    pub fn replace_preview_diff(&self, path: &QString) -> ffi::FfiFileDiff {
        let path = path.to_string();
        match self
            .replace_preview
            .borrow()
            .iter()
            .find(|p| p.path.display().to_string() == path)
        {
            Some(preview) => ffi::FfiFileDiff {
                path: QString::from(path.as_str()),
                old_text: QString::from(preview.old_text.as_str()),
                new_text: QString::from(preview.new_text.as_str()),
            },
            None => ffi::FfiFileDiff::default(),
        }
    }

    pub fn replace_preview_hunks(&self, path: &QString) -> Vec<ffi::FfiHunk> {
        let path = path.to_string();
        match self
            .replace_preview
            .borrow()
            .iter()
            .find(|p| p.path.display().to_string() == path)
        {
            Some(preview) => crate::bridge::convert::to_ffi_hunks(&preview.hunks),
            None => Vec::new(),
        }
    }

    pub fn replace_preview_spans(&self, path: &QString) -> Vec<ffi::FfiInlineSpan> {
        let path = path.to_string();
        match self
            .replace_preview
            .borrow()
            .iter()
            .find(|p| p.path.display().to_string() == path)
        {
            Some(preview) => crate::bridge::convert::to_ffi_inline_spans(
                &preview.old_text,
                &preview.new_text,
                &preview.hunks,
                editor_core::diff::HighlightMode::Words,
            ),
            None => Vec::new(),
        }
    }

    /// Task I: project-wide Class View tier — see `project_symbols`'s
    /// bridge doc comment for why this reuses `search`'s index handle and
    /// background-thread/per-match-signal shape instead of a new one.
    pub fn project_symbols(self: Pin<&mut Self>) {
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let guard = slot.read().unwrap();
            let Some(index) = guard.ready() else {
                let reason = guard.unavailable_reason().unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model
                        .as_mut()
                        .project_symbols_failed(QString::from(reason.as_str()));
                });
                return;
            };
            // Empty substring query matches every name (`str::contains("")`
            // is always true), so this lists every indexed definition — no
            // `index-core` change needed, see the plan doc's Task I.
            let result = index.find_definitions("");
            drop(guard);
            match result {
                Ok(matches) => {
                    for m in matches {
                        // A definition `identifier_occurrences()` found but
                        // `outline()` didn't also capture (no `kind`) has
                        // nothing structural to show in a class tree.
                        if m.kind.is_none() {
                            continue;
                        }
                        let row = to_ffi_symbol_match(m);
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model.as_mut().project_symbol_found(row);
                        });
                    }
                    let _ = qt_thread.queue(|mut model: Pin<&mut Self>| {
                        model.as_mut().project_symbols_finished();
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .project_symbols_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    /// Task J — find-usages: every occurrence of the exact name `name`,
    /// definitions and references alike. Same background-thread/streamed-
    /// signal shape as `search`/`project_symbols` above.
    pub fn find_usages(self: Pin<&mut Self>, name: &QString) {
        let name = name.to_string();
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let guard = slot.read().unwrap();
            let Some(index) = guard.ready() else {
                let reason = guard.unavailable_reason().unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model.as_mut().usages_failed(QString::from(reason.as_str()));
                });
                return;
            };
            let result = index.find_usages(&name);
            drop(guard);
            match result {
                Ok(matches) => {
                    for m in matches {
                        let row = to_ffi_symbol_match(m);
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model.as_mut().usages_found(row);
                        });
                    }
                    let _ = qt_thread.queue(|mut model: Pin<&mut Self>| {
                        model.as_mut().usages_finished();
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .usages_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    pub fn resolve_declaration(
        self: Pin<&mut Self>,
        path: &QString,
        content: &QString,
        byte_offset: usize,
    ) {
        let path = std::path::PathBuf::from(path.to_string());
        let content = content.to_string();
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let guard = slot.read().unwrap();
            // Without a ready index (no project open, or one still building)
            // the buffer alone still answers a same-file declaration, which is
            // the majority of what a Ctrl+Click asks about — far better than
            // refusing the gesture outright.
            let result = match guard.ready() {
                Some(index) => index.resolve_declaration(&path, &content, byte_offset),
                None => Ok(index_core::resolve_declaration_in_buffer(
                    &path,
                    &content,
                    byte_offset,
                )),
            };
            drop(guard);
            match result {
                Ok(resolution) => {
                    for candidate in resolution.candidates {
                        let row = to_ffi_symbol_match(candidate);
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model.as_mut().declaration_found(row);
                        });
                    }
                    let tier = to_ffi_resolution_tier(resolution.tier);
                    let name = QString::from(resolution.name.as_str());
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model.as_mut().declaration_finished(tier, name);
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .declaration_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }

    pub fn find_implementations(self: Pin<&mut Self>, name: &QString) {
        self.stream_usages(name.to_string(), |index, name| {
            index.find_implementations(name)
        });
    }

    pub fn find_supertypes(self: Pin<&mut Self>, name: &QString) {
        self.stream_usages(name.to_string(), |index, name| index.find_supertypes(name));
    }

    /// Shared body of `find_implementations`/`find_supertypes` (N3): run
    /// a name-keyed index query on a background thread and stream its
    /// rows out on the `usagesFound` trio, which is what `find_usages`
    /// itself does — see `findImplementations`' doc comment for why they
    /// share one signal set rather than each getting their own.
    fn stream_usages(
        self: Pin<&mut Self>,
        name: String,
        query: fn(
            &index_core::TextIndex,
            &str,
        ) -> Result<Vec<index_core::SymbolMatch>, index_core::IndexError>,
    ) {
        let qt_thread = self.qt_thread();
        let slot = std::sync::Arc::clone(&self.index);
        std::thread::spawn(move || {
            let guard = slot.read().unwrap();
            let Some(index) = guard.ready() else {
                let reason = guard.unavailable_reason().unwrap_or_default();
                drop(guard);
                let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                    model.as_mut().usages_failed(QString::from(reason.as_str()));
                });
                return;
            };
            let result = query(index, &name);
            drop(guard);
            match result {
                Ok(matches) => {
                    for m in matches {
                        let row = to_ffi_symbol_match(m);
                        let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                            model.as_mut().usages_found(row);
                        });
                    }
                    let _ = qt_thread.queue(|mut model: Pin<&mut Self>| {
                        model.as_mut().usages_finished();
                    });
                }
                Err(err) => {
                    let message = err.to_string();
                    let _ = qt_thread.queue(move |mut model: Pin<&mut Self>| {
                        model
                            .as_mut()
                            .usages_failed(QString::from(message.as_str()));
                    });
                }
            }
        });
    }
}
