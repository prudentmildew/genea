//! Session state (ticket #59): what each project's window showed, so that a
//! restart brings it back, and the list of projects that were open.
//!
//! Each project's session is a JSON file in Genea's application-support
//! folder, `sessions/<key>.json`, the key derived from the project root
//! like the review store's. It holds the editor tabs and the split (with
//! each tab's carets and scroll position as lines and char columns, so a
//! file changed meanwhile only clamps them), the left column's view, the
//! terminal's tabs, the zoom, and the window's layout (its frame, the left
//! column's width and the terminal's size), which only the view knows and
//! reports with `Command::SetWindowLayout`.
//!
//! The open projects are `open-projects.txt` beside it, one root per line,
//! in the order they opened.
//!
//! Files are read when they are needed: a project's session when it opens
//! (a small read, like the folder check in `open_project`; the app needs
//! the window's frame before it shows the window), the open projects at
//! [`Workbench::restore_session`](crate::Workbench::restore_session).
//! Changes are written in the background a short pause after the last one
//! (on the host clock), so a burst of keystrokes writes once; quitting
//! writes everything at once ([`Workbench::quit`](crate::Workbench::quit)).

use std::{
    ffi::OsStr,
    fs, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use genea_host::Clock;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    jobs::Jobs,
    problems::TextPosition,
    view::{LeftColumnView, WindowFrame, WindowLayout},
};

/// The pause between the last change and the write.
pub(crate) const SAVE_DELAY: Duration = Duration::from_secs(1);

const SESSIONS_DIR: &str = "sessions";
const OPEN_PROJECTS: &str = "open-projects.txt";

/// What a project's window showed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ProjectSession {
    /// The editor area's sides, left to right; empty without tabs.
    pub(crate) panes: Vec<PaneSession>,
    pub(crate) focused_pane: usize,
    pub(crate) left_column: Option<LeftColumnView>,
    pub(crate) terminal: TerminalSession,
    /// Font-size steps from the default (`Command::ZoomIn`).
    pub(crate) zoom: i32,
    pub(crate) layout: Option<WindowLayout>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PaneSession {
    pub(crate) tabs: Vec<TabSession>,
    pub(crate) active: usize,
}

/// A tab: its file (relative to the root when inside it) and where it was.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabSession {
    pub(crate) path: PathBuf,
    /// Each caret's (anchor, caret), in the order they were added: the last
    /// is the primary.
    pub(crate) selections: Vec<(TextPosition, TextPosition)>,
    /// The first visible row.
    pub(crate) scroll_top: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TerminalSession {
    pub(crate) tabs: Vec<TerminalTabSession>,
    pub(crate) active: usize,
    pub(crate) visible: bool,
}

/// A terminal tab: a shell starts again fresh; a command's tab comes back
/// without running, for Re-run.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum TerminalTabSession {
    Shell,
    /// The package manager with `args` (an install).
    PackageManager { name: String, args: Vec<String>, title: String },
    /// A `package.json` script, run in `directory` (relative to the root).
    Script { script: String, directory: PathBuf, title: String },
}

impl ProjectSession {
    fn to_json(&self, root: &Path) -> Value {
        let position = |p: &TextPosition| json!([p.line, p.column]);
        json!({
            "root": root.to_string_lossy(),
            "panes": self.panes.iter().map(|pane| json!({
                "active": pane.active,
                "tabs": pane.tabs.iter().map(|tab| json!({
                    "path": tab.path.to_string_lossy(),
                    "selections": tab.selections.iter().map(|(a, c)| json!([position(a), position(c)])).collect::<Vec<_>>(),
                    "scroll_top": tab.scroll_top,
                })).collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "focused_pane": self.focused_pane,
            "left_column": self.left_column.map(left_column_name),
            "terminal": {
                "active": self.terminal.active,
                "visible": self.terminal.visible,
                "tabs": self.terminal.tabs.iter().map(|tab| match tab {
                    TerminalTabSession::Shell => json!({ "kind": "shell" }),
                    TerminalTabSession::PackageManager { name, args, title } => json!({
                        "kind": "package-manager", "name": name, "args": args, "title": title,
                    }),
                    TerminalTabSession::Script { script, directory, title } => json!({
                        "kind": "script", "script": script, "directory": directory.to_string_lossy(), "title": title,
                    }),
                }).collect::<Vec<_>>(),
            },
            "zoom": self.zoom,
            "layout": self.layout.map(|l| json!({
                "frame": [l.frame.x, l.frame.y, l.frame.width, l.frame.height],
                "left_column_width": l.left_column_width,
                "terminal_width": l.terminal_width,
                "terminal_height": l.terminal_height,
            })),
        })
    }

    /// Reads a session file's JSON. Anything missing or malformed falls
    /// back to what a project opens with.
    fn from_json(value: &Value) -> ProjectSession {
        let usize_at = |v: &Value, key: &str| v.get(key).and_then(Value::as_u64).map_or(0, |n| n as usize);
        let string = |v: &Value, key: &str| v.get(key).and_then(Value::as_str).unwrap_or_default().to_owned();
        let position = |v: &Value| {
            let n = |i: usize| v.get(i).and_then(Value::as_u64).map_or(0, |n| n as usize);
            TextPosition { line: n(0), column: n(1) }
        };
        let array = |v: Option<&Value>| v.and_then(Value::as_array).cloned().unwrap_or_default();
        let panes = array(value.get("panes"))
            .iter()
            .map(|pane| PaneSession {
                active: usize_at(pane, "active"),
                tabs: array(pane.get("tabs"))
                    .iter()
                    .filter_map(|tab| {
                        let path = tab.get("path")?.as_str()?;
                        let mut selections: Vec<_> = array(tab.get("selections"))
                            .iter()
                            .filter_map(|s| Some((position(s.get(0)?), position(s.get(1)?))))
                            .collect();
                        // A tab always has a caret: at the start, if none was saved.
                        if selections.is_empty() {
                            selections.push((TextPosition::default(), TextPosition::default()));
                        }
                        let scroll_top = tab.get("scroll_top").and_then(Value::as_f64).unwrap_or(0.0).max(0.0);
                        Some(TabSession { path: path.into(), selections, scroll_top })
                    })
                    .collect(),
            })
            .filter(|pane: &PaneSession| !pane.tabs.is_empty())
            .take(2)
            .collect();
        let left_column = match value.get("left_column") {
            Some(Value::String(name)) => left_column_view(name),
            Some(Value::Null) => None,
            _ => Some(LeftColumnView::Files),
        };
        let terminal = match value.get("terminal") {
            Some(terminal) => TerminalSession {
                active: usize_at(terminal, "active"),
                visible: terminal.get("visible").and_then(Value::as_bool).unwrap_or(true),
                tabs: array(terminal.get("tabs"))
                    .iter()
                    .filter_map(|tab| match tab.get("kind")?.as_str()? {
                        "shell" => Some(TerminalTabSession::Shell),
                        "package-manager" => Some(TerminalTabSession::PackageManager {
                            name: string(tab, "name"),
                            args: array(tab.get("args")).iter().filter_map(|a| Some(a.as_str()?.to_owned())).collect(),
                            title: string(tab, "title"),
                        }),
                        "script" => Some(TerminalTabSession::Script {
                            script: string(tab, "script"),
                            directory: string(tab, "directory").into(),
                            title: string(tab, "title"),
                        }),
                        _ => None,
                    })
                    .collect(),
            },
            None => TerminalSession::default(),
        };
        let layout = value.get("layout").and_then(|layout| {
            let frame = layout.get("frame")?.as_array()?;
            let n = |i: usize| frame.get(i).and_then(Value::as_f64);
            let length = |key: &str| layout.get(key).and_then(Value::as_f64);
            Some(WindowLayout {
                frame: WindowFrame { x: n(0)?, y: n(1)?, width: n(2)?, height: n(3)? },
                left_column_width: length("left_column_width")?,
                terminal_width: length("terminal_width")?,
                terminal_height: length("terminal_height")?,
            })
        });
        let zoom = value.get("zoom").and_then(Value::as_i64).map_or(0, |z| z.clamp(-100, 100) as i32);
        let focused_pane = usize_at(value, "focused_pane");
        ProjectSession { panes, focused_pane, left_column, terminal, zoom, layout }
    }
}

impl Default for TerminalSession {
    /// A new project's terminal: one shell, showing.
    fn default() -> Self {
        TerminalSession { tabs: vec![TerminalTabSession::Shell], active: 0, visible: true }
    }
}

fn left_column_name(view: LeftColumnView) -> &'static str {
    match view {
        LeftColumnView::Files => "files",
        LeftColumnView::Problems => "problems",
        LeftColumnView::Search => "search",
        LeftColumnView::Changes => "changes",
        LeftColumnView::Scripts => "scripts",
        LeftColumnView::Usages => "usages",
    }
}

fn left_column_view(name: &str) -> Option<LeftColumnView> {
    Some(match name {
        "files" => LeftColumnView::Files,
        "problems" => LeftColumnView::Problems,
        "search" => LeftColumnView::Search,
        "changes" => LeftColumnView::Changes,
        "scripts" => LeftColumnView::Scripts,
        "usages" => LeftColumnView::Usages,
        _ => return None,
    })
}

/// A project's session file.
pub(crate) struct SessionFile {
    root: PathBuf,
    file: DebouncedFile,
    /// The session last handed to `file`, to write only changes.
    last: Option<ProjectSession>,
}

impl SessionFile {
    pub(crate) fn new(support_dir: &Path, root: &Path) -> Self {
        let key: String = Sha256::digest(root.as_os_str().as_bytes()).iter().take(8).map(|b| format!("{b:02x}")).collect();
        let file = support_dir.join(SESSIONS_DIR).join(format!("{key}.json"));
        SessionFile { root: root.to_owned(), file: DebouncedFile::new(file), last: None }
    }

    /// Reads the saved session, if there is one for this root.
    pub(crate) fn read(&self) -> Option<ProjectSession> {
        let bytes = fs::read(&self.file.path).ok()?;
        let value: Value = serde_json::from_slice(&bytes).ok()?;
        // Two roots whose keys collide: not this project's.
        if value.get("root").and_then(Value::as_str) != Some(&*self.root.to_string_lossy()) {
            return None;
        }
        Some(ProjectSession::from_json(&value))
    }

    /// Writes `session` a short pause from now, if it changed.
    pub(crate) fn changed(&mut self, session: ProjectSession, jobs: &Jobs, clock: &dyn Clock) {
        if self.last.as_ref() == Some(&session) {
            return;
        }
        self.file.changed(self.bytes(&session), jobs, clock);
        self.last = Some(session);
    }

    /// Writes `session` now, on this thread (quitting).
    pub(crate) fn write_now(&mut self, session: ProjectSession) {
        self.file.write_now(self.bytes(&session));
        self.last = Some(session);
    }

    /// Writes `session` in the background now (the project closes).
    pub(crate) fn write_soon(&mut self, session: ProjectSession, jobs: &Jobs) {
        self.file.write_soon(self.bytes(&session), jobs);
        self.last = Some(session);
    }

    fn bytes(&self, session: &ProjectSession) -> Vec<u8> {
        let mut bytes = serde_json::to_vec_pretty(&session.to_json(&self.root)).expect("JSON values serialise");
        bytes.push(b'\n');
        bytes
    }
}

/// The open projects' list.
pub(crate) struct OpenProjects {
    file: DebouncedFile,
}

impl OpenProjects {
    pub(crate) fn new(support_dir: &Path) -> Self {
        OpenProjects { file: DebouncedFile::new(support_dir.join(OPEN_PROJECTS)) }
    }

    /// The roots that were open, in the order they opened.
    pub(crate) fn read(&self) -> Vec<PathBuf> {
        let bytes = fs::read(&self.file.path).unwrap_or_default();
        bytes
            .split(|&b| b == b'\n')
            .map(|line| PathBuf::from(OsStr::from_bytes(line)))
            .filter(|root| root.is_absolute())
            .collect()
    }

    pub(crate) fn changed<'a>(&mut self, roots: impl IntoIterator<Item = &'a Path>, jobs: &Jobs, clock: &dyn Clock) {
        self.file.changed(lines(roots), jobs, clock);
    }

    pub(crate) fn write_now<'a>(&mut self, roots: impl IntoIterator<Item = &'a Path>) {
        self.file.write_now(lines(roots));
    }
}

fn lines<'a>(roots: impl IntoIterator<Item = &'a Path>) -> Vec<u8> {
    let mut bytes = Vec::new();
    for root in roots {
        let line = root.as_os_str().as_bytes();
        // A newline in a folder name can't be stored in this format.
        if !line.contains(&b'\n') {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }
    }
    bytes
}

/// A file written in the background a short pause after its last change,
/// newest content wins.
struct DebouncedFile {
    path: PathBuf,
    pending: Arc<Mutex<Pending>>,
}

#[derive(Default)]
struct Pending {
    bytes: Option<Vec<u8>>,
    timer_set: bool,
}

impl DebouncedFile {
    fn new(path: PathBuf) -> Self {
        DebouncedFile { path, pending: Arc::default() }
    }

    fn changed(&mut self, bytes: Vec<u8>, jobs: &Jobs, clock: &dyn Clock) {
        let mut pending = self.pending.lock().unwrap();
        pending.bytes = Some(bytes);
        if pending.timer_set {
            return;
        }
        pending.timer_set = true;
        let (shared, path, jobs) = (self.pending.clone(), self.path.clone(), jobs.clone());
        clock.after(
            SAVE_DELAY,
            Box::new(move || {
                shared.lock().unwrap().timer_set = false;
                spawn_write(shared, path, &jobs);
            }),
        );
    }

    fn write_soon(&mut self, bytes: Vec<u8>, jobs: &Jobs) {
        self.pending.lock().unwrap().bytes = Some(bytes);
        spawn_write(self.pending.clone(), self.path.clone(), jobs);
    }

    fn write_now(&mut self, bytes: Vec<u8>) {
        let mut pending = self.pending.lock().unwrap();
        // A write waiting for its timer or job would be older.
        pending.bytes = None;
        report(&self.path, write(&self.path, &bytes));
    }
}

fn spawn_write(pending: Arc<Mutex<Pending>>, path: PathBuf, jobs: &Jobs) {
    jobs.spawn("save session", move || {
        // Taken inside the job, under the lock, so writes land in order and
        // the last one is the newest.
        let mut pending = pending.lock().unwrap();
        if let Some(bytes) = pending.bytes.take() {
            report(&path, write(&path, &bytes));
        }
        Box::new(|_| {})
    });
}

fn report(path: &Path, written: io::Result<()>) {
    if let Err(error) = written {
        eprintln!("genea: couldn't save the session to {}: {error}", path.display());
    }
}

/// Writes atomically: a crash mid-write keeps the old file.
fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, path)
}
