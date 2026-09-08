//! Action catalog and keymap rules: which commands the UI offers, what their
//! default keyboard shortcuts are, and how a user's overrides interact.
//!
//! No Qt dependency. Shortcuts are opaque strings in `QKeySequence`'s portable
//! text form (e.g. `"Ctrl+Shift+F"`), canonicalized by the view — this module
//! never parses a key sequence, it only compares and stores the strings the
//! view hands it. An empty string means "deliberately unbound".

use std::collections::HashMap;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

/// One user-triggerable command: a stable id (the persisted key), the menu
/// label the settings UI shows, the menu it lives under, and the shortcut it
/// has when the user hasn't rebound it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActionDef {
    pub id: &'static str,
    pub label: &'static str,
    pub category: &'static str,
    /// Empty means the action ships without a shortcut.
    pub default_shortcut: &'static str,
}

/// Every rebindable action in the app, in menu order. Ids are persisted in
/// `settings.toml` and must therefore stay stable across releases; labels and
/// defaults may change freely.
pub const ACTIONS: &[ActionDef] = &[
    ActionDef {
        id: "file.openFolder",
        label: "Open Folder...",
        category: "File",
        default_shortcut: "",
    },
    ActionDef {
        id: "file.save",
        label: "Save",
        category: "File",
        default_shortcut: "Ctrl+S",
    },
    ActionDef {
        id: "file.saveAs",
        label: "Save As...",
        category: "File",
        default_shortcut: "Ctrl+Shift+S",
    },
    ActionDef {
        id: "file.preferences",
        label: "Preferences...",
        category: "File",
        default_shortcut: "Ctrl+,",
    },
    ActionDef {
        id: "file.projectSettings",
        label: "Project Settings...",
        category: "File",
        // No default: `Ctrl+,` belongs to the settings dialog itself, and a
        // second shortcut for the same dialog opened on a different tab is
        // not worth one of the remaining free combinations (ADR-0022).
        default_shortcut: "",
    },
    ActionDef {
        id: "file.exit",
        label: "Exit",
        category: "File",
        default_shortcut: "Ctrl+Q",
    },
    ActionDef {
        id: "edit.undo",
        label: "Undo",
        category: "Edit",
        default_shortcut: "Ctrl+Z",
    },
    ActionDef {
        id: "edit.redo",
        label: "Redo",
        category: "Edit",
        default_shortcut: "Ctrl+Shift+Z",
    },
    ActionDef {
        id: "edit.cut",
        label: "Cut",
        category: "Edit",
        default_shortcut: "Ctrl+X",
    },
    ActionDef {
        id: "edit.copy",
        label: "Copy",
        category: "Edit",
        default_shortcut: "Ctrl+C",
    },
    ActionDef {
        id: "edit.paste",
        label: "Paste",
        category: "Edit",
        default_shortcut: "Ctrl+V",
    },
    ActionDef {
        id: "edit.find",
        label: "Find...",
        category: "Edit",
        default_shortcut: "Ctrl+F",
    },
    ActionDef {
        id: "edit.replace",
        label: "Replace...",
        category: "Edit",
        default_shortcut: "Ctrl+R",
    },
    ActionDef {
        id: "edit.findNext",
        label: "Find Next",
        category: "Edit",
        default_shortcut: "F3",
    },
    ActionDef {
        id: "edit.findPrevious",
        label: "Find Previous",
        category: "Edit",
        default_shortcut: "Shift+F3",
    },
    ActionDef {
        id: "edit.findInFiles",
        label: "Find in Files...",
        category: "Edit",
        default_shortcut: "Ctrl+Shift+F",
    },
    // F1: the editing operations. Grouped after Find because that is where
    // they sit in the Edit menu, and given JetBrains' defaults where there
    // is one — the shortcuts people already have in their fingers.
    ActionDef {
        id: "edit.selectNextOccurrence",
        label: "Select Next Occurrence",
        category: "Edit",
        default_shortcut: "Ctrl+D",
    },
    ActionDef {
        id: "edit.addCaretAbove",
        label: "Add Caret Above",
        category: "Edit",
        default_shortcut: "Ctrl+Alt+Up",
    },
    ActionDef {
        id: "edit.addCaretBelow",
        label: "Add Caret Below",
        category: "Edit",
        default_shortcut: "Ctrl+Alt+Down",
    },
    ActionDef {
        id: "edit.toggleLineComment",
        label: "Comment with Line Comment",
        category: "Edit",
        default_shortcut: "Ctrl+/",
    },
    ActionDef {
        id: "edit.toggleBlockComment",
        label: "Comment with Block Comment",
        category: "Edit",
        default_shortcut: "Ctrl+Shift+/",
    },
    ActionDef {
        id: "edit.duplicateLine",
        label: "Duplicate Line or Selection",
        category: "Edit",
        default_shortcut: "Ctrl+Alt+D",
    },
    ActionDef {
        id: "edit.moveLineUp",
        label: "Move Line Up",
        category: "Edit",
        default_shortcut: "Alt+Shift+Up",
    },
    ActionDef {
        id: "edit.moveLineDown",
        label: "Move Line Down",
        category: "Edit",
        default_shortcut: "Alt+Shift+Down",
    },
    ActionDef {
        id: "edit.deleteLine",
        label: "Delete Line",
        category: "Edit",
        default_shortcut: "Ctrl+Y",
    },
    ActionDef {
        id: "edit.joinLines",
        label: "Join Lines",
        category: "Edit",
        default_shortcut: "Ctrl+Shift+J",
    },
    ActionDef {
        id: "edit.expandSelection",
        label: "Extend Selection",
        category: "Edit",
        default_shortcut: "Ctrl+W",
    },
    ActionDef {
        id: "edit.shrinkSelection",
        label: "Shrink Selection",
        category: "Edit",
        default_shortcut: "Ctrl+Shift+W",
    },
    ActionDef {
        id: "edit.matchingBracket",
        label: "Go to Matching Bracket",
        category: "Edit",
        default_shortcut: "Ctrl+]",
    },
    ActionDef {
        id: "code.reformat",
        label: "Reformat Code",
        category: "Code",
        default_shortcut: "Ctrl+Alt+L",
    },
    ActionDef {
        id: "code.showIntentions",
        label: "Show Intention Actions",
        category: "Code",
        default_shortcut: "Alt+Return",
    },
    ActionDef {
        id: "code.parameterInfo",
        label: "Parameter Info",
        category: "Code",
        default_shortcut: "Ctrl+P",
    },
    ActionDef {
        id: "code.optimizeImports",
        label: "Optimize Imports",
        category: "Code",
        default_shortcut: "Ctrl+Alt+O",
    },
    ActionDef {
        id: "code.toggleInlayHints",
        label: "Show Inlay Hints",
        category: "Code",
        default_shortcut: "",
    },
    ActionDef {
        id: "code.collapseAll",
        label: "Collapse All",
        category: "Code",
        default_shortcut: "Ctrl+Shift+-",
    },
    ActionDef {
        id: "code.expandAll",
        label: "Expand All",
        category: "Code",
        default_shortcut: "Ctrl+Shift+=",
    },
    ActionDef {
        id: "view.classView",
        label: "Class View",
        category: "View",
        default_shortcut: "Ctrl+Alt+C",
    },
    ActionDef {
        id: "view.projectTree",
        label: "Project",
        category: "View",
        default_shortcut: "Ctrl+Alt+T",
    },
    ActionDef {
        id: "view.terminal",
        label: "Terminal",
        category: "View",
        default_shortcut: "Ctrl+`",
    },
    ActionDef {
        id: "view.problems",
        label: "Problems",
        category: "View",
        default_shortcut: "Ctrl+Alt+P",
    },
    ActionDef {
        id: "view.saveLayout",
        label: "Save Current Layout...",
        category: "View",
        // Unbound by default: keeping a layout is a deliberate, occasional
        // act, and the View category's obvious keys are spoken for.
        default_shortcut: "",
    },
    ActionDef {
        id: "view.deleteLayout",
        label: "Delete Layout...",
        category: "View",
        default_shortcut: "",
    },
    ActionDef {
        id: "view.searchEverywhere",
        label: "Search Everywhere...",
        category: "View",
        default_shortcut: "Ctrl+Shift+E",
    },
    ActionDef {
        id: "view.goToFile",
        label: "Go to File...",
        category: "View",
        default_shortcut: "Ctrl+Shift+N",
    },
    // AI assistant (ADR-0021). Ctrl+L matches what every other IDE with a
    // chat panel binds "send this selection to the assistant" to, and was
    // unbound here.
    ActionDef {
        id: "ai.addSelection",
        label: "Add Selection to AI Chat",
        category: "AI",
        default_shortcut: "Ctrl+L",
    },
    ActionDef {
        id: "ai.addSelectionNewChat",
        label: "Add Selection to New AI Chat",
        category: "AI",
        default_shortcut: "",
    },
    ActionDef {
        id: "ai.addFile",
        label: "Add File to AI Chat",
        category: "AI",
        default_shortcut: "",
    },
    ActionDef {
        id: "ai.newChat",
        label: "New AI Chat",
        category: "AI",
        default_shortcut: "Ctrl+Shift+L",
    },
    ActionDef {
        id: "ai.togglePanel",
        label: "AI Chat",
        category: "AI",
        default_shortcut: "",
    },
    // The View menu is where every other dock's show-action lives
    // (Class View, Problems, Terminal). AI Chat having its toggle only on
    // the AI menu is what made the panel hard to find after a restored
    // layout closed it.
    ActionDef {
        id: "view.aiChat",
        label: "AI Chat",
        category: "View",
        default_shortcut: "",
    },
    ActionDef {
        id: "view.preview",
        label: "Preview",
        category: "View",
        // Ctrl+Alt+M is already Extract Method's; Ctrl+Alt+V is free.
        default_shortcut: "Ctrl+Alt+V",
    },
    ActionDef {
        id: "view.togglePreviewMode",
        label: "Preview Mode",
        category: "View",
        // Flips the *current tab* between its source and its rendered form,
        // where `view.preview` above opens the dock beside it. Ctrl+Shift+M
        // is free; Ctrl+Shift+V, the shortcut the same gesture carries in
        // some other editors, is already Terminal's Paste.
        default_shortcut: "Ctrl+Shift+M",
    },
    ActionDef {
        id: "view.findAction",
        label: "Find Action...",
        category: "View",
        default_shortcut: "Ctrl+Shift+A",
    },
    ActionDef {
        id: "view.goToSymbol",
        label: "Go to Symbol...",
        category: "View",
        default_shortcut: "Ctrl+Shift+O",
    },
    ActionDef {
        id: "view.goToLine",
        label: "Go to Line...",
        category: "View",
        default_shortcut: "Ctrl+G",
    },
    // Refactoring (RF11). Defaults follow JetBrains, which is where these
    // gestures come from: Shift+F6 for rename, Ctrl+Alt+M for extract
    // method, and Ctrl+Alt+Shift+T for the "what can you do here" menu.
    // Extract Class ships unbound — JetBrains has no default for it either,
    // and inventing one risks colliding with a binding a user already has.
    ActionDef {
        id: "refactor.rename",
        label: "Rename...",
        category: "Refactor",
        default_shortcut: "Shift+F6",
    },
    ActionDef {
        id: "refactor.extractMethod",
        label: "Extract Method...",
        category: "Refactor",
        default_shortcut: "Ctrl+Alt+M",
    },
    ActionDef {
        id: "refactor.extractClass",
        label: "Extract Class...",
        category: "Refactor",
        default_shortcut: "",
    },
    ActionDef {
        id: "refactor.refactorThis",
        label: "Refactor This...",
        category: "Refactor",
        default_shortcut: "Ctrl+Alt+Shift+T",
    },
    // Code navigation (N8). Defaults follow JetBrains, the closest
    // reference for these gestures: Ctrl+B for the declaration a
    // Ctrl+Click also reaches, Alt+F7 for usages, Ctrl+Alt+B/Ctrl+Alt+U
    // for the two directions of the type hierarchy, and Ctrl+Alt+Left /
    // Ctrl+Alt+Right for the jump history.
    ActionDef {
        id: "navigate.goToDeclaration",
        label: "Go to Declaration",
        category: "Navigate",
        default_shortcut: "Ctrl+B",
    },
    ActionDef {
        id: "navigate.findUsages",
        label: "Find Usages",
        category: "Navigate",
        default_shortcut: "Alt+F7",
    },
    ActionDef {
        id: "navigate.goToImplementation",
        label: "Go to Implementation",
        category: "Navigate",
        default_shortcut: "Ctrl+Alt+B",
    },
    ActionDef {
        id: "navigate.goToInterface",
        label: "Go to Interface",
        category: "Navigate",
        default_shortcut: "Ctrl+Alt+U",
    },
    // C11-followup: call/type hierarchy dock. JetBrains' own defaults —
    // Ctrl+Alt+H for the call hierarchy, Ctrl+H for the type hierarchy.
    ActionDef {
        id: "navigate.showCallHierarchy",
        label: "Show Call Hierarchy",
        category: "Navigate",
        default_shortcut: "Ctrl+Alt+H",
    },
    ActionDef {
        id: "navigate.showTypeHierarchy",
        label: "Show Type Hierarchy",
        category: "Navigate",
        default_shortcut: "Ctrl+H",
    },
    ActionDef {
        id: "navigate.back",
        label: "Back",
        category: "Navigate",
        default_shortcut: "Ctrl+Alt+Left",
    },
    ActionDef {
        id: "navigate.forward",
        label: "Forward",
        category: "Navigate",
        default_shortcut: "Ctrl+Alt+Right",
    },
    // Terminal copy/paste deliberately avoid Ctrl+C/Ctrl+V: in a terminal
    // those belong to the shell (Ctrl+C is SIGINT), so the conventional
    // Ctrl+Shift pair is the default here.
    ActionDef {
        id: "terminal.copy",
        label: "Copy",
        category: "Terminal",
        default_shortcut: "Ctrl+Shift+C",
    },
    ActionDef {
        id: "terminal.paste",
        label: "Paste",
        category: "Terminal",
        default_shortcut: "Ctrl+Shift+V",
    },
    // F4-14b: multi-session terminal — a new tab, and (Windows-only; see
    // `pty_core::WindowsShellKind`) picking which shell it spawns.
    ActionDef {
        id: "terminal.newSession",
        label: "New Terminal Tab",
        category: "Terminal",
        default_shortcut: "Ctrl+Shift+T",
    },
    ActionDef {
        id: "terminal.selectShell",
        label: "Select Shell...",
        category: "Terminal",
        default_shortcut: "",
    },
    // F3-19: Git v1's action set.
    ActionDef {
        id: "vcs.commit",
        label: "Commit...",
        category: "Git",
        default_shortcut: "Ctrl+K",
    },
    ActionDef {
        id: "vcs.push",
        label: "Push",
        category: "Git",
        default_shortcut: "Ctrl+Shift+K",
    },
    ActionDef {
        id: "vcs.pull",
        label: "Pull",
        category: "Git",
        default_shortcut: "",
    },
    ActionDef {
        id: "vcs.fetch",
        label: "Fetch",
        category: "Git",
        default_shortcut: "",
    },
    ActionDef {
        id: "vcs.branches",
        label: "Branches...",
        category: "Git",
        default_shortcut: "Ctrl+Shift+`",
    },
    ActionDef {
        id: "vcs.showDiff",
        label: "Show Diff",
        category: "Git",
        default_shortcut: "Ctrl+Alt+G",
    },
    ActionDef {
        id: "vcs.rollbackHunk",
        label: "Rollback Hunk",
        category: "Git",
        default_shortcut: "",
    },
    ActionDef {
        // Was F7/Shift+F7, which is IntelliJ's Step Into and its inverse.
        // The debugger has the stronger claim on those (D3-8), and these two
        // move to the keys IntelliJ actually gives them.
        id: "vcs.nextChange",
        label: "Next Change",
        category: "Git",
        default_shortcut: "Ctrl+Alt+Shift+Down",
    },
    ActionDef {
        id: "vcs.previousChange",
        label: "Previous Change",
        category: "Git",
        default_shortcut: "Ctrl+Alt+Shift+Up",
    },
    ActionDef {
        id: "vcs.annotate",
        label: "Annotate with Blame",
        category: "Git",
        default_shortcut: "",
    },
    ActionDef {
        id: "view.changes",
        label: "Changes",
        category: "View",
        default_shortcut: "Alt+9",
    },
    ActionDef {
        id: "view.vcsHistory",
        label: "File History",
        category: "View",
        default_shortcut: "",
    },
    ActionDef {
        id: "debug.debug",
        label: "Debug",
        category: "Debug",
        default_shortcut: "Shift+F9",
    },
    ActionDef {
        id: "debug.resume",
        label: "Resume Program",
        category: "Debug",
        default_shortcut: "F9",
    },
    ActionDef {
        id: "debug.pause",
        label: "Pause Program",
        category: "Debug",
        default_shortcut: "",
    },
    ActionDef {
        id: "debug.stepOver",
        label: "Step Over",
        category: "Debug",
        default_shortcut: "F8",
    },
    ActionDef {
        id: "debug.stepInto",
        label: "Step Into",
        category: "Debug",
        default_shortcut: "F7",
    },
    ActionDef {
        id: "debug.stepOut",
        label: "Step Out",
        category: "Debug",
        default_shortcut: "Shift+F8",
    },
    ActionDef {
        id: "debug.stop",
        label: "Stop Debugging",
        category: "Debug",
        default_shortcut: "",
    },
    ActionDef {
        id: "debug.attach",
        label: "Attach to Process...",
        category: "Debug",
        default_shortcut: "",
    },
    ActionDef {
        id: "debug.reloadClasses",
        label: "Reload Changed Classes",
        category: "Debug",
        default_shortcut: "",
    },
    ActionDef {
        id: "debug.attachRemote",
        label: "Attach to Remote...",
        category: "Debug",
        default_shortcut: "",
    },
    ActionDef {
        id: "debug.toggleBreakpoint",
        label: "Toggle Breakpoint",
        category: "Debug",
        default_shortcut: "Ctrl+F8",
    },
    ActionDef {
        id: "debug.muteBreakpoints",
        label: "Mute Breakpoints",
        category: "Debug",
        default_shortcut: "",
    },
    ActionDef {
        id: "view.debug",
        label: "Debug",
        category: "View",
        default_shortcut: "",
    },
    ActionDef {
        id: "build.build",
        label: "Build Project",
        category: "Build",
        default_shortcut: "Ctrl+F9",
    },
    ActionDef {
        id: "build.rebuild",
        label: "Rebuild Project",
        category: "Build",
        default_shortcut: "Ctrl+Shift+F9",
    },
    ActionDef {
        id: "build.stop",
        label: "Stop Build",
        category: "Build",
        default_shortcut: "",
    },
    ActionDef {
        id: "view.build",
        label: "Build",
        category: "View",
        default_shortcut: "",
    },
    // The PHP tooling plan's B9: runs every enabled, installed analyzer
    // against the whole open project via `AnalysisService::inspectProject`.
    ActionDef {
        id: "analysis.inspectProject",
        label: "Inspect Project",
        category: "Analysis",
        default_shortcut: "",
    },
    ActionDef {
        id: "run.run",
        label: "Run",
        category: "Run",
        default_shortcut: "Shift+F10",
    },
    ActionDef {
        id: "run.runContext",
        label: "Run File",
        category: "Run",
        default_shortcut: "Ctrl+Shift+F10",
    },
    ActionDef {
        id: "run.stop",
        label: "Stop",
        category: "Run",
        default_shortcut: "Ctrl+F2",
    },
    ActionDef {
        id: "run.kill",
        label: "Kill",
        category: "Run",
        default_shortcut: "",
    },
    ActionDef {
        id: "run.rerun",
        label: "Rerun",
        category: "Run",
        default_shortcut: "Ctrl+F5",
    },
    ActionDef {
        id: "run.selectConfiguration",
        label: "Select Run Configuration...",
        category: "Run",
        default_shortcut: "Alt+Shift+F10",
    },
    ActionDef {
        id: "run.showRunningList",
        label: "Show Running List",
        category: "Run",
        default_shortcut: "",
    },
    ActionDef {
        id: "run.editConfigurations",
        label: "Edit Configurations...",
        category: "Run",
        default_shortcut: "",
    },
    ActionDef {
        id: "view.runConsole",
        label: "Run Console",
        category: "View",
        default_shortcut: "Alt+4",
    },
    // Last, because the Help menu is last in the bar. Unbound by default:
    // every platform's convention for About is the menu, not a key.
    ActionDef {
        id: "help.about",
        label: "About Kestrel",
        category: "Help",
        default_shortcut: "",
    },
];

/// The action with this id, or `None` for an id that no longer exists (an old
/// settings file can name one).
pub fn action(id: &str) -> Option<&'static ActionDef> {
    ACTIONS.iter().find(|a| a.id == id)
}

/// One row of the keymap settings table: what the action is, the shortcut it
/// actually responds to right now, and whether that's still the shipped
/// default (so the view can render rebound rows differently without
/// recomputing the rule itself).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub action: &'static ActionDef,
    pub shortcut: String,
    pub is_default: bool,
}

/// A user's keyboard shortcut overrides, layered over [`ACTIONS`]' defaults.
///
/// Only overrides are held (and persisted): an action absent from the map uses
/// its default, so changing a default in a release reaches users who never
/// rebound that action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Keymap {
    overrides: HashMap<String, String>,
}

impl Keymap {
    /// Wrap a persisted override map (`Settings::keymap`).
    pub fn from_overrides(overrides: HashMap<String, String>) -> Self {
        Self { overrides }
    }

    /// The override map to persist back into `Settings::keymap`.
    pub fn into_overrides(self) -> HashMap<String, String> {
        self.overrides
    }

    /// The shortcut `id` currently responds to: the user's override if it has
    /// one, otherwise the shipped default. Empty means unbound — either
    /// shipped that way or deliberately cleared.
    pub fn shortcut_for(&self, id: &str) -> &str {
        if let Some(over) = self.overrides.get(id) {
            return over;
        }
        action(id).map(|a| a.default_shortcut).unwrap_or("")
    }

    /// Whether `id` still has its shipped default shortcut. An override that
    /// happens to equal the default counts as default — it's the same binding.
    pub fn is_default(&self, id: &str) -> bool {
        match action(id) {
            Some(a) => self.shortcut_for(id) == a.default_shortcut,
            None => true,
        }
    }

    /// The actions that would lose their binding if `shortcut` were assigned
    /// to `id`. Unbinding (an empty `shortcut`) never conflicts, and an action
    /// never conflicts with itself.
    pub fn conflicts(&self, id: &str, shortcut: &str) -> Vec<&'static ActionDef> {
        if shortcut.is_empty() {
            return Vec::new();
        }
        ACTIONS
            .iter()
            .filter(|a| a.id != id && self.shortcut_for(a.id) == shortcut)
            .collect()
    }

    /// Bind `shortcut` to `id`, unbinding every action that held it before
    /// (the "warn and steal" rule — the view is expected to have confirmed
    /// with the user via [`Keymap::conflicts`] first).
    ///
    /// A stolen action gets an explicit empty override rather than being
    /// removed from the map, so it stays unbound instead of falling back to
    /// its default.
    pub fn assign(&mut self, id: &str, shortcut: &str) {
        if action(id).is_none() {
            return;
        }
        for conflicting in self
            .conflicts(id, shortcut)
            .iter()
            .map(|a| a.id)
            .collect::<Vec<_>>()
        {
            self.overrides
                .insert(conflicting.to_string(), String::new());
        }
        self.overrides.insert(id.to_string(), shortcut.to_string());
    }

    /// Drop every override, returning the whole keymap to the shipped
    /// defaults.
    pub fn reset_to_defaults(&mut self) {
        self.overrides.clear();
    }

    /// Every action with its effective shortcut, in [`ACTIONS`] order — the
    /// keymap settings table, ready to render.
    pub fn bindings(&self) -> Vec<Binding> {
        ACTIONS
            .iter()
            .map(|a| Binding {
                action: a,
                shortcut: self.shortcut_for(a.id).to_string(),
                is_default: self.is_default(a.id),
            })
            .collect()
    }
}

/// One action matched by [`search_actions`], with the shortcut it currently
/// responds to and the character positions in `"Category: Label"` that the
/// query matched — enough for the results list to render and run it without
/// asking anything else.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionMatch {
    pub action: &'static ActionDef,
    pub shortcut: String,
    pub score: u32,
    pub positions: Vec<u32>,
}

/// The haystack an action is matched against: `"Category: Label"`, so both
/// `"edit find"` and `"find in files"` reach Find in Files.
fn action_haystack(action: &ActionDef) -> String {
    format!("{}: {}", action.category, action.label)
}

/// Search Everywhere's action tier: fuzzy-rank [`ACTIONS`] against `query`,
/// best first, at most `limit` hits. An empty query lists the first `limit`
/// actions in menu order.
///
/// Matching lives here rather than in the view because which actions exist
/// and what they are called is this crate's knowledge — the view only
/// renders the rows and triggers the ids.
pub fn search_actions(query: &str, keymap: &Keymap, limit: usize) -> Vec<ActionMatch> {
    let bind = |action: &'static ActionDef, score: u32, positions: Vec<u32>| ActionMatch {
        action,
        shortcut: keymap.shortcut_for(action.id).to_string(),
        score,
        positions,
    };

    if query.is_empty() {
        return ACTIONS
            .iter()
            .take(limit)
            .map(|a| bind(a, 0, Vec::new()))
            .collect();
    }

    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();

    let mut scored: Vec<ActionMatch> = ACTIONS
        .iter()
        .filter_map(|action| {
            let haystack = action_haystack(action);
            let mut positions = Vec::new();
            let score = pattern.indices(
                Utf32Str::new(&haystack, &mut buf),
                &mut matcher,
                &mut positions,
            )?;
            positions.sort_unstable();
            positions.dedup();
            Some(bind(action, score, positions))
        })
        .collect();
    // Ties break towards the shorter label — the more specific command.
    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.action.label.len().cmp(&b.action.label.len()))
    });
    scored.truncate(limit);
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keymap() -> Keymap {
        Keymap::default()
    }

    #[test]
    fn action_ids_are_unique() {
        // Ids are the persisted key and the conflict-rule's identity — a
        // duplicate would make rebinding non-deterministic.
        let mut ids: Vec<&str> = ACTIONS.iter().map(|a| a.id).collect();
        ids.sort_unstable();
        let count = ids.len();
        ids.dedup();
        assert_eq!(ids.len(), count);
    }

    #[test]
    fn shipped_defaults_do_not_conflict_with_each_other() {
        // Any collision here would mean the app ships with two actions on one
        // shortcut, which Qt resolves ambiguously.
        let map = keymap();
        for a in ACTIONS {
            assert!(
                map.conflicts(a.id, a.default_shortcut).is_empty(),
                "{} collides on {}",
                a.id,
                a.default_shortcut
            );
        }
    }

    #[test]
    fn shortcut_falls_back_to_the_default_when_unset() {
        assert_eq!(keymap().shortcut_for("view.goToLine"), "Ctrl+G");
        assert!(keymap().is_default("view.goToLine"));
    }

    #[test]
    fn override_wins_over_the_default() {
        let mut map = keymap();
        map.assign("view.goToLine", "Ctrl+Alt+G");
        assert_eq!(map.shortcut_for("view.goToLine"), "Ctrl+Alt+G");
        assert!(!map.is_default("view.goToLine"));
    }

    #[test]
    fn unknown_action_id_reads_as_unbound_and_is_not_assignable() {
        // An old settings file can name an action that no longer exists.
        let mut map = Keymap::from_overrides(HashMap::from([(
            "gone.action".to_string(),
            "Ctrl+K".to_string(),
        )]));
        map.assign("also.gone", "Ctrl+J");
        assert_eq!(map.shortcut_for("also.gone"), "");
        assert!(map.conflicts("view.goToLine", "Ctrl+J").is_empty());
    }

    #[test]
    fn assigning_a_taken_shortcut_reports_the_current_owner() {
        let conflicts = keymap().conflicts("view.goToLine", "Ctrl+Shift+F");
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].id, "edit.findInFiles");
    }

    #[test]
    fn assign_steals_the_shortcut_and_leaves_the_loser_unbound() {
        let mut map = keymap();
        map.assign("view.goToLine", "Ctrl+Shift+F");
        assert_eq!(map.shortcut_for("view.goToLine"), "Ctrl+Shift+F");
        // Explicitly unbound, not reverted to its own default.
        assert_eq!(map.shortcut_for("edit.findInFiles"), "");
        assert!(!map.is_default("edit.findInFiles"));
    }

    #[test]
    fn an_action_does_not_conflict_with_itself() {
        assert!(keymap().conflicts("file.save", "Ctrl+S").is_empty());
    }

    #[test]
    fn unbinding_conflicts_with_nothing() {
        let mut map = keymap();
        map.assign("file.save", "");
        // Several actions ship unbound; clearing another one is not a clash.
        assert!(map.conflicts("file.saveAs", "").is_empty());
        assert_eq!(map.shortcut_for("file.save"), "");
    }

    #[test]
    fn reset_to_defaults_restores_every_action() {
        let mut map = keymap();
        map.assign("view.goToLine", "Ctrl+Shift+F");
        map.reset_to_defaults();
        for a in ACTIONS {
            assert_eq!(map.shortcut_for(a.id), a.default_shortcut);
            assert!(map.is_default(a.id));
        }
        assert_eq!(map.into_overrides().len(), 0);
    }

    #[test]
    fn bindings_cover_every_action_in_catalog_order() {
        let bindings = keymap().bindings();
        assert_eq!(bindings.len(), ACTIONS.len());
        assert_eq!(bindings[0].action.id, ACTIONS[0].id);
        assert!(bindings.iter().all(|b| b.is_default));
    }

    #[test]
    fn search_actions_ranks_the_closest_command_first() {
        let hits = search_actions("find in files", &keymap(), 10);
        assert_eq!(hits[0].action.id, "edit.findInFiles");
        assert_eq!(hits[0].shortcut, "Ctrl+Shift+F");
        assert!(!hits[0].positions.is_empty());
    }

    #[test]
    fn search_actions_matches_the_category_too() {
        let hits = search_actions("view terminal", &keymap(), 10);
        assert_eq!(hits[0].action.id, "view.terminal");
    }

    #[test]
    fn search_actions_reports_the_users_override_not_the_default() {
        let mut map = keymap();
        map.assign("edit.findInFiles", "Ctrl+Alt+F");
        let hits = search_actions("find in files", &map, 5);
        assert_eq!(hits[0].shortcut, "Ctrl+Alt+F");
    }

    #[test]
    fn search_actions_honours_the_limit_and_lists_actions_when_empty() {
        assert_eq!(search_actions("", &keymap(), 3).len(), 3);
        assert_eq!(search_actions("", &keymap(), 3)[0].action.id, ACTIONS[0].id);
        assert!(search_actions("zzzznotacommand", &keymap(), 10).is_empty());
    }
}
