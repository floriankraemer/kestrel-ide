//! The E2E fixture: one seeded, isolated IDE process driven over X11.
//!
//! Deliberately depends on no workspace crate (ADR-0024). It launches the
//! *built* binary and talks to it the way a user does — the keyboard, the
//! mouse, and the files on disk — so nothing it asserts can be satisfied by
//! the code under test simply agreeing with itself.
//!
//! Three rules the whole design follows:
//!
//! 1. Input is xdotool, always. MCP is observation only.
//! 2. Nothing waits for a duration; everything waits for a transition.
//!    [`wait::wait_for`] is the single primitive and holds the only `sleep`.
//! 3. Every assertion is against state the app published — the marker
//!    stream, MCP, or the filesystem — never against a screenshot.

pub mod mcp;
pub mod wait;
pub mod xdotool;

use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use serde_json::Value;
use tempfile::TempDir;

pub use wait::{wait_for, wait_for_within};

/// The main window's title, as `buildMainWindow` sets it. Anchored: the
/// Search Everywhere and preview dialogs are separate toplevels.
const MAIN_WINDOW_TITLE: &str = "^Kestrel$";

/// A position in the marker stream, from [`Ide::mark`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mark(usize);

impl Mark {
    /// The beginning of the stream — for a wait on something the app
    /// published during startup, before the test could take a mark.
    pub fn start() -> Mark {
        Mark(0)
    }
}

/// A seeded, isolated IDE process.
pub struct Ide {
    name: String,
    binary: PathBuf,
    /// Holds `XDG_CONFIG_HOME`, `XDG_CACHE_HOME` and `HOME`. Kept across a
    /// restart, which is the whole point of the persistence flow.
    home: TempDir,
    /// The project root. Throwaway per test — and the *only* isolation the
    /// index has, since `index-core` writes `<root>/.ide-index` and honours
    /// no environment variable.
    project: TempDir,
    events_path: PathBuf,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    child: Option<Child>,
    window: String,
}

impl Ide {
    /// Copy `fixture` into a fresh project root, seed a fresh config dir and
    /// launch. `binary` must come from `env!("CARGO_BIN_EXE_app")` — guessing
    /// at `target/debug` is wrong under every profile but one.
    pub fn launch(name: &str, binary: impl AsRef<Path>, fixture: impl AsRef<Path>) -> Ide {
        let home = TempDir::new().expect("temp home");
        let project = TempDir::new().expect("temp project");
        copy_tree(fixture.as_ref(), project.path());

        let mut ide = Ide {
            name: name.to_string(),
            binary: binary.as_ref().to_path_buf(),
            events_path: home.path().join("events.jsonl"),
            stdout_path: home.path().join("app.stdout"),
            stderr_path: home.path().join("app.stderr"),
            home,
            project,
            child: None,
            window: String::new(),
        };
        std::fs::create_dir_all(ide.config_dir()).expect("config dir");
        // How the app finds the project: there is no CLI argument, and
        // driving the Open Folder file dialog would test GTK, not us.
        std::fs::write(
            ide.config_dir().join("last-project.txt"),
            ide.project_root().to_string_lossy().as_bytes(),
        )
        .expect("seeding last-project.txt");
        ide.spawn();
        ide
    }

    /// Come back with the same config dir and project, after a [`Ide::quit`].
    /// The persistence flow's whole subject: `app-config`'s round-trip is
    /// unit-tested, the view rebuilding itself from it is not.
    pub fn relaunch(&mut self) {
        assert!(self.child.is_none(), "quit before relaunching");
        self.spawn();
    }

    fn spawn(&mut self) {
        assert!(
            std::env::var_os("DISPLAY").is_some(),
            "no DISPLAY: run this under `make e2e`, which supplies Xvfb"
        );
        // Truncated rather than appended: a restart's markers belong to the
        // restarted app, and a test that greps the whole file would otherwise
        // match the previous process' startup.
        let _ = std::fs::remove_file(&self.events_path);

        let stdout = std::fs::File::create(&self.stdout_path).expect("app.stdout");
        let stderr = std::fs::File::create(&self.stderr_path).expect("app.stderr");
        let config_home = self.home.path().join("config");
        std::fs::create_dir_all(&config_home).expect("XDG_CONFIG_HOME");

        // Passed to the child, never through `std::env::set_var`: integration
        // tests share one process, so mutating our own environment is a
        // process-global race between test threads.
        let child = Command::new(&self.binary)
            .env("XDG_CONFIG_HOME", &config_home)
            .env("XDG_CACHE_HOME", self.home.path().join("cache"))
            .env("HOME", self.home.path())
            .env("IDE_E2E_EVENTS", &self.events_path)
            .env_remove("XDG_CONFIG_DIRS")
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .unwrap_or_else(|e| panic!("launching {}: {e}", self.binary.display()));
        self.child = Some(child);
        self.await_startup();
    }

    /// The three-part startup wait. A window can exist before it is mapped,
    /// and `xdotool type` into an unmapped window is silently dropped — so
    /// neither "the process is running" nor `search --name` alone is enough.
    fn await_startup(&mut self) {
        let events = self.events_path.clone();
        wait_for("the main window to be shown", || {
            read_events(&events)
                .iter()
                .find(|e| e["ev"] == "main_window_shown")
                .map(|_| ())
        });

        let window = wait_for("exactly one visible IDE window", || {
            let windows = xdotool::visible_windows(MAIN_WINDOW_TITLE);
            match windows.len() {
                1 => Some(windows[0].clone()),
                0 => None,
                _ => panic!("{} visible IDE windows: {windows:?}", windows.len()),
            }
        });

        xdotool::run(&["windowfocus", "--sync", &window]);
        wait_for("the IDE window to take the input focus", || {
            xdotool::focused_window().filter(|focused| *focused == window)
        });
        self.window = window;
    }

    // --- environment -----------------------------------------------------

    /// `resolve_config_dir()`'s answer for this instance.
    pub fn config_dir(&self) -> PathBuf {
        // Asserted rather than merely set: with both `XDG_CONFIG_HOME` and
        // `HOME` unset, `resolve_config_dir` falls back to
        // `std::env::temp_dir().join("ide")`, which is shared — a suite that
        // only *sets* the variable can silently lose its isolation.
        let config_home = self.home.path().join("config");
        assert!(
            config_home.starts_with(self.home.path()),
            "XDG_CONFIG_HOME escaped the fixture's temp dir"
        );
        config_home.join("ide")
    }

    pub fn project_root(&self) -> &Path {
        self.project.path()
    }

    /// Read a fixture file from disk directly — never through the app, which
    /// would let the app's own idea of the buffer answer for the file.
    pub fn read_project_file(&self, relative: impl AsRef<Path>) -> String {
        let path = self.project_root().join(relative.as_ref());
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {}: {e}", path.display()))
    }

    /// Every regular file under the project root, by relative path, with its
    /// bytes. The "cancel that applies anyway" assertion compares two of
    /// these.
    pub fn project_snapshot(&self) -> Vec<(PathBuf, Vec<u8>)> {
        let mut files = Vec::new();
        collect_files(self.project_root(), self.project_root(), &mut files);
        files.sort();
        files
    }

    // --- the marker stream -----------------------------------------------

    pub fn events(&self) -> Vec<Value> {
        read_events(&self.events_path)
    }

    /// A position in the marker stream. Taken *before* a gesture, so what
    /// follows can be asserted about without the app's startup marks — or a
    /// previous step's — answering the question by accident.
    pub fn mark(&self) -> Mark {
        Mark(self.events().len())
    }

    /// Markers published since `mark`.
    pub fn events_since(&self, mark: Mark) -> Vec<Value> {
        let mut all = self.events();
        if mark.0 >= all.len() {
            return Vec::new();
        }
        all.split_off(mark.0)
    }

    /// Markers of one kind published since `mark`.
    pub fn events_since_of(&self, mark: Mark, kind: &str) -> Vec<Value> {
        self.events_since(mark)
            .into_iter()
            .filter(|e| e["ev"] == kind)
            .collect()
    }

    /// Wait for a marker published since `mark` that matches `predicate`.
    pub fn wait_for_event(
        &self,
        mark: Mark,
        what: &str,
        predicate: impl Fn(&Value) -> bool,
    ) -> Value {
        let events = self.events_path.clone();
        wait_for(what, || {
            read_events(&events)
                .into_iter()
                .skip(mark.0)
                .find(|e| predicate(e))
        })
    }

    pub fn wait_for_ev(&self, mark: Mark, kind: &str) -> Value {
        self.wait_for_event(mark, &format!("a `{kind}` marker"), |e| e["ev"] == kind)
    }

    /// Assert that the diff tab `tab_id` was built over panes of exactly
    /// `left` and `right` characters.
    ///
    /// A diff tab existing says nothing about what is in it: a compare whose
    /// two lookups both resolve to nothing still opens a tab, and reads as a
    /// pass to any assertion that only counts tabs (#210). The panes' own
    /// text is not reachable from outside the process, so `diff_tab_content`
    /// carries their lengths.
    pub fn assert_diff_panes(&self, mark: Mark, tab_id: u64, left: u64, right: u64) {
        let content = self.wait_for_event(mark, "the diff tab's pane sizes", |e| {
            e["ev"] == "diff_tab_content" && e["tab_id"].as_u64() == Some(tab_id)
        });
        assert_eq!(
            (content["left"].as_u64(), content["right"].as_u64()),
            (Some(left), Some(right)),
            "diff tab {tab_id} was built over the wrong texts"
        );
    }

    // --- observation ------------------------------------------------------

    pub fn mcp(&self) -> mcp::Mcp {
        let config_dir = self.config_dir();
        wait_for("the MCP server to publish its discovery file", || {
            mcp::Mcp::discover(&config_dir)
        })
    }

    /// Drain the Qt event loop. Editor-touching MCP commands are marshalled
    /// onto the GUI thread, so a reply proves everything queued before the
    /// request has already run — which is what lets a test assert that
    /// something did *not* happen without inventing a settle delay.
    pub fn sync(&self, mcp: &mcp::Mcp) {
        mcp.call("list_open_buffers", serde_json::json!({}));
    }

    // --- input -------------------------------------------------------------

    /// One or more X keystrokes, e.g. `"ctrl+s"` or `"Escape"`.
    pub fn key(&self, keys: &str) {
        // --clearmodifiers so a modifier left latched by a previous chord
        // cannot silently change the next one.
        xdotool::run(&["key", "--clearmodifiers", keys]);
    }

    /// Type literal text. `--delay 0` because the product's own debounce
    /// windows are what several flows measure — an inter-key delay of
    /// xdotool's choosing would be measuring xdotool.
    pub fn type_text(&self, text: &str) {
        xdotool::run(&["type", "--clearmodifiers", "--delay", "0", text]);
    }

    pub fn mouse_move(&self, x: i32, y: i32) {
        xdotool::run(&["mousemove", "--sync", &x.to_string(), &y.to_string()]);
    }

    pub fn click(&self, button: u8) {
        xdotool::run(&["click", "--clearmodifiers", &button.to_string()]);
    }

    pub fn click_at(&self, x: i32, y: i32, button: u8) {
        self.mouse_move(x, y);
        self.click(button);
    }

    /// A genuine double-click: two button events from one `xdotool`
    /// invocation (`--repeat 2`), close enough together to land inside
    /// Qt's double-click interval — two separate `click_at` calls each pay
    /// a fresh subprocess spawn, which on a loaded CI machine can exceed it
    /// and register as two single clicks instead. `QTreeWidget::itemDoubleClicked`
    /// and friends need an actual double-click event, not two independent
    /// single clicks far enough apart for Qt to treat them as such.
    pub fn double_click_at(&self, x: i32, y: i32, button: u8) {
        self.mouse_move(x, y);
        xdotool::run(&[
            "click",
            "--clearmodifiers",
            "--repeat",
            "2",
            "--delay",
            "40",
            &button.to_string(),
        ]);
    }

    /// Press at `from`, travel to `to`, release — a real drag.
    ///
    /// The travel is stepped rather than a single jump: a drag is recognised
    /// from pointer *motion*, and one teleporting move gives the widget
    /// under the cursor nothing to recognise. The steps also carry the
    /// pointer over the intermediate widgets, which is what makes the
    /// drag-enter/drag-move handshake happen at all.
    pub fn drag(&self, from: (i32, i32), to: (i32, i32)) {
        const STEPS: i32 = 12;
        self.mouse_move(from.0, from.1);
        xdotool::run(&["mousedown", "--clearmodifiers", "1"]);
        for step in 1..=STEPS {
            self.mouse_move(
                from.0 + (to.0 - from.0) * step / STEPS,
                from.1 + (to.1 - from.1) * step / STEPS,
            );
        }
        xdotool::run(&["mouseup", "--clearmodifiers", "1"]);
    }

    /// Wait until some window is active and return its id. A dialog is its
    /// own toplevel, so this is how a flow knows a modal actually took focus
    /// before typing into it.
    pub fn wait_for_focus_change(&self, from: &str) -> String {
        let from = from.to_string();
        wait_for("the input focus to move to another window", || {
            xdotool::focused_window().filter(|id| *id != from)
        })
    }

    pub fn window(&self) -> &str {
        &self.window
    }

    /// Give the input focus back to the main window.
    ///
    /// Needed after every dialog closes, and only because there is no window
    /// manager under bare Xvfb. Qt's shortcuts default to
    /// `Qt::WindowShortcut`, which fires only for the *active* window, so
    /// without this a Ctrl+S after a dialog is silently dropped while plain
    /// typing still reaches the editor — a divergence from every real desktop
    /// and a fine way to spend an afternoon.
    pub fn focus_main(&self) {
        let window = self.window.clone();
        xdotool::run(&["windowfocus", "--sync", &window]);
        wait_for("the main window to take the input focus back", || {
            xdotool::focused_window().filter(|focused| *focused == window)
        });
    }

    // --- shutdown ----------------------------------------------------------

    /// Ctrl+Q, then wait for the process to actually exit. Returns its code.
    pub fn quit(&mut self) -> i32 {
        self.key("ctrl+q");
        self.wait_for_exit()
    }

    pub fn wait_for_exit(&mut self) -> i32 {
        let mut child = self.child.take().expect("the app is running");
        let status = wait_for_within("the app to exit", Duration::from_secs(30), || {
            child.try_wait().expect("waiting on the app")
        });
        // The window must be gone too, or the next spawn's "exactly one
        // visible window" wait races the corpse of this one.
        wait_for("the IDE window to disappear", || {
            xdotool::visible_windows(MAIN_WINDOW_TITLE)
                .is_empty()
                .then_some(())
        });
        status
            .code()
            .unwrap_or_else(|| panic!("the app was killed: {status}"))
    }
}

impl Drop for Ide {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
        // Marker streams are kept on success too: they are the input to the
        // seam-split golden comparison.
        let dir = artifact_dir(&self.name);
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::copy(&self.events_path, dir.join("events.jsonl"));
        if std::thread::panicking() {
            let _ = std::fs::copy(&self.stdout_path, dir.join("app.stdout"));
            let _ = std::fs::copy(&self.stderr_path, dir.join("app.stderr"));
            // A screenshot is a diagnostic, never an assertion (ADR-0024):
            // pixel comparison is a permanent low-grade red nobody reads.
            let _ = Command::new("import")
                .args([OsStr::new("-window"), OsStr::new("root")])
                .arg(dir.join("screen.png"))
                .status();
            eprintln!("e2e artifacts for `{}`: {}", self.name, dir.display());
        }
    }
}

fn artifact_dir(name: &str) -> PathBuf {
    // `CARGO_TARGET_DIR` is not set in this workspace, so target/ sits beside
    // the manifest dir's parent — resolved relative to the crate rather than
    // the cwd, which cargo does not promise.
    let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("workspace root")
        .to_path_buf();
    std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| workspace.join("target"))
        .join("e2e-artifacts")
        .join(name)
}

fn read_events(path: &Path) -> Vec<Value> {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    raw.lines()
        // A partial trailing line is a mark caught mid-write. Skipping it is
        // correct: the next poll will see it whole.
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap_or_else(|e| panic!("{}: {e}", from.display())) {
        let entry = entry.expect("readable fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            std::fs::create_dir_all(&target).expect("fixture subdirectory");
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("fixture file");
        }
    }
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        // The index writes `<root>/.ide-index` (it honours no environment
        // variable), so it is inside the project on purpose and is not part
        // of what a "did any file change" assertion is about.
        if path.file_name().and_then(OsStr::to_str) == Some(".ide-index") {
            continue;
        }
        if path.is_dir() {
            collect_files(root, &path, out);
        } else if let Ok(bytes) = std::fs::read(&path) {
            out.push((
                path.strip_prefix(root).expect("under root").to_path_buf(),
                bytes,
            ));
        }
    }
}
