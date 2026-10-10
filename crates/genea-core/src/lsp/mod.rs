//! The LSP client (spec #19, Language intelligence; ADR 0002; ticket #42):
//! hand-rolled JSON-RPC over stdio, with `gen-lsp-types` (LSP 3.18) for the
//! messages.
//!
//! - [`connection`]: one server process and its traffic, on background
//!   threads. Requests hold busy tokens, so `settle` waits for answers.
//! - [`LanguageServer`] (here): one server's lifecycle and state on the
//!   main thread: starting and initializing, the restart policy (up to
//!   [`MAX_RESTARTS`] within [`RESTART_WINDOW`] on the host clock, then
//!   failed until "Restart language server"), document sync and pulled
//!   diagnostics. It is generic: tsgo, `oxlint --lsp` (#49) and later
//!   `oxfmt --lsp` (#50) are instances with their own [`ServerSpec`].
//!   Generations are unique across servers, so a project routes events and
//!   timers by them ([`LanguageServer::owns`]).
//! - [`typescript`]: finding tsgo in `node_modules`, and "Add TypeScript 7".
//! - [`oxc`]: finding Oxlint and Oxfmt, and "Add Oxlint and Oxfmt".
//! - [`watch`]: the globs a server registers for
//!   `workspace/didChangeWatchedFiles`, fed from the project watcher.
//! - [`text`]: URIs, language ids and positions.
//!
//! The project glue (`project/language.rs`, and `project/oxlint.rs` for
//! Oxlint) owns the servers, feeds them the
//! open editors and the watcher's changes, and turns their [`Output`] into
//! Problems. **Typing never waits on a server**: edits apply on the main
//! thread as always; syncing a document only queues its text (a cheap rope
//! clone) for the writer thread.
//!
//! Adding a request (completion, hover, …; #43–#47): add a [`Pending`]
//! variant, send it with [`LanguageServer::request`] from a command (only
//! while [`LanguageServer::is_ready`]), and handle its answer in
//! [`LanguageServer::event`]. Positions go out in the server's
//! [`text::Encoding`]; the document the server has is the editor's text as of
//! the last [`LanguageServer::sync`], which runs after every command and
//! every background result.

pub(crate) mod actions;
mod connection;
pub(crate) mod oxc;
pub(crate) mod symbols;
pub(crate) mod text;
pub(crate) mod typescript;
pub(crate) mod watch;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use gen_lsp_types::{
    ClientCapabilities, ClientInfo, Diagnostic, DocumentSymbolClientCapabilities, WorkspaceSymbolClientCapabilities, DiagnosticClientCapabilities, DiagnosticWorkspaceClientCapabilities,
    DidChangeWatchedFilesClientCapabilities, DocumentDiagnosticReport, FileEvent, GeneralClientCapabilities,
    InitializeParams, InitializeResult, PositionEncodingKind, PublishDiagnosticsClientCapabilities,
    PublishDiagnosticsParams, RegistrationParams, TextDocumentClientCapabilities,
    TextDocumentSyncClientCapabilities, UnregistrationParams, WorkspaceClientCapabilities, WorkspaceFolder,
    WorkspaceFolders, WorkspaceFoldersInitializeParams,
};
use genea_host::{ProcessSpec, SharedHost};
use serde_json::{Value, json};

use crate::{
    editor::Editor,
    environment::ProcessEnv,
    jobs::Jobs,
    view::{LanguageServerState, LanguageServerStatus},
    workbench::{Core, ProjectId},
};
use connection::{Connection, Deliver, ResponseError};
pub(crate) use connection::Event;
use text::Encoding;
use watch::Watchers;

/// How many times a crashed server is restarted within [`RESTART_WINDOW`]
/// before it is left stopped (spec #19: 3 in 5 minutes).
pub const MAX_RESTARTS: usize = 3;
/// See [`MAX_RESTARTS`].
pub const RESTART_WINDOW: Duration = Duration::from_secs(5 * 60);
/// How long after a crash the server is started again, on the host clock.
pub const RESTART_DELAY: Duration = Duration::from_secs(1);
/// How long a server may take to answer `initialize` before the status bar
/// says it isn't responding, on the host clock. It keeps running, and is
/// used as soon as it answers.
pub const START_TIMEOUT: Duration = Duration::from_secs(30);

/// LSP's "the server cancelled the request" and "the content changed"
/// errors: the client asks again.
const SERVER_CANCELLED: i64 = -32802;
const CONTENT_MODIFIED: i64 = -32801;

/// What a language server is and how it starts.
#[derive(Clone, Debug)]
pub(crate) struct ServerSpec {
    /// What the user calls it: `TypeScript`.
    pub(crate) name: &'static str,
    /// The program and its arguments; the project environment is added at
    /// each start.
    pub(crate) process: ProcessSpec,
    /// `initializationOptions`.
    pub(crate) options: Value,
    /// Shown while it runs, e.g. `TypeScript 7.0.2`, until the server names
    /// its own version.
    pub(crate) label: String,
}

/// What the project must do after a server event or a sync.
#[derive(Debug)]
pub(crate) enum Output {
    /// Replace an open file's diagnostics from this server (positions in
    /// the server's encoding, against the file's current text).
    Diagnostics { path: PathBuf, diagnostics: Vec<Diagnostic> },
    /// Drop a file's diagnostics: it closed.
    Clear(PathBuf),
    /// Drop every file's diagnostics: the server is gone.
    ClearAll,
    /// The answer to the symbols request `id` (ticket #47); empty if it
    /// failed.
    Symbols { id: i64, symbols: Vec<symbols::Symbol> },
    /// The answer to [`code_actions`](LanguageServer::code_actions) with
    /// this ticket: the actions Genea can apply (ticket #45).
    CodeActions { ticket: u64, fixes: Vec<actions::CodeActionFix> },
}

/// A request waiting for its answer.
#[derive(Debug)]
pub(crate) enum Pending {
    Initialize,
    /// `textDocument/diagnostic` for an open file.
    Diagnostics(PathBuf),
    /// `textDocument/documentSymbol` for `document` (an absolute path), or
    /// `workspace/symbol` (ticket #47).
    Symbols { document: Option<PathBuf> },
    /// `textDocument/codeAction`, with the asker's ticket (ticket #45).
    CodeActions(u64),
}

/// A timer of a server's, on the host clock.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Timer {
    /// `initialize` has had [`START_TIMEOUT`].
    StartTimeout,
    /// [`RESTART_DELAY`] after a crash.
    Restart,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum State {
    /// Started; `initialize` not answered yet.
    Starting,
    Ready,
    /// `initialize` hasn't been answered within [`START_TIMEOUT`].
    NotResponding,
    /// Crashed; starts again after [`RESTART_DELAY`].
    Restarting,
    /// Crashed too often (or couldn't start): stays stopped until
    /// "Restart language server". Why, for the notice.
    Failed(String),
    /// Stopped on purpose.
    Stopped,
}

/// An open file the server knows about.
struct Document {
    uri: String,
    /// The version the server has: counts up with every change.
    version: i32,
    /// The editor's version it was last synced at (editors can go back to
    /// an earlier version with undo; the server's never does).
    editor_version: u64,
    /// Diagnostics should be pulled (again).
    wanted: bool,
    /// The diagnostics request in flight, if any. One at a time per file;
    /// the next goes out once it is answered.
    pulling: Option<i64>,
    /// What the server last reported for it, for asking for quick fixes.
    diagnostics: Vec<Diagnostic>,
}

/// One language server of a project, on the main thread.
pub(crate) struct LanguageServer {
    project: ProjectId,
    root: PathBuf,
    spec: ServerSpec,
    host: SharedHost,
    jobs: Jobs,
    /// New with every start and stop, so events, answers and timers of an
    /// earlier process are dropped. Unique across servers ([`next_generation`]),
    /// so the project can tell which of its servers an event is for.
    generation: u64,
    connection: Option<Connection>,
    state: State,
    /// What the server said it is (`serverInfo`), once initialized.
    label: String,
    encoding: Encoding,
    /// When each automatic restart happened, for the restart policy.
    restarts: Vec<Instant>,
    documents: BTreeMap<PathBuf, Document>,
    requests: HashMap<i64, Pending>,
    watchers: Watchers,
}

impl LanguageServer {
    pub(crate) fn new(project: ProjectId, root: PathBuf, spec: ServerSpec, host: SharedHost, jobs: Jobs) -> Self {
        let label = spec.label.clone();
        LanguageServer {
            project,
            root,
            spec,
            host,
            jobs,
            generation: 0,
            connection: None,
            state: State::Stopped,
            label,
            encoding: Encoding::default(),
            restarts: Vec::new(),
            documents: BTreeMap::new(),
            requests: HashMap::new(),
            watchers: Watchers::default(),
        }
    }

    /// Starts the server process (in the background) and initializes it.
    /// A running one is stopped first.
    pub(crate) fn start(&mut self, env: &ProcessEnv) -> Vec<Output> {
        let outputs = self.disconnect();
        self.generation = next_generation();
        let generation = self.generation;
        let id = self.project;
        let deliver: Deliver = Arc::new(move |core: &mut Core, event| {
            if let Some(project) = core.project_mut(id) {
                project.language_event(generation, event);
            }
        });
        let folder = WorkspaceFolder {
            uri: text::uri(&self.root).into(),
            name: self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        };
        let folders = serde_json::to_value([&folder]).expect("a workspace folder is JSON");
        let spec = env.apply(self.spec.process.clone().cwd(&self.root));
        let mut connection = Connection::start(self.host.clone(), spec, folders, &self.jobs, deliver);
        let initialize = connection.request("initialize", self.initialize_params(folder));
        self.requests.insert(initialize, Pending::Initialize);
        self.connection = Some(connection);
        self.state = State::Starting;
        self.after(START_TIMEOUT, Timer::StartTimeout);
        outputs
    }

    /// "Restart language server": a fresh start, with the restart policy
    /// reset.
    pub(crate) fn restart(&mut self, env: &ProcessEnv) -> Vec<Output> {
        self.restarts.clear();
        self.start(env)
    }

    /// Stops the server (closing the project does this).
    pub(crate) fn stop(&mut self) -> Vec<Output> {
        let outputs = self.disconnect();
        self.generation = next_generation();
        self.state = State::Stopped;
        outputs
    }

    /// Drops the connection (stopping its process) and everything that
    /// belonged to it.
    fn disconnect(&mut self) -> Vec<Output> {
        if let Some(mut connection) = self.connection.take() {
            connection.stop();
        }
        self.requests.clear();
        self.watchers = Watchers::default();
        let had_documents = !std::mem::take(&mut self.documents).is_empty();
        if had_documents { vec![Output::ClearAll] } else { Vec::new() }
    }

    /// Whether events and timers of `generation` are this server's: its
    /// current process's.
    pub(crate) fn owns(&self, generation: u64) -> bool {
        self.generation == generation
    }

    #[allow(dead_code)] // The seam for commands that ask the server (#43–#47).
    pub(crate) fn is_ready(&self) -> bool {
        self.state == State::Ready
    }

    /// The server's columns: positions it sends and expects are in these.
    pub(crate) fn encoding(&self) -> Encoding {
        self.encoding
    }

    /// The status-bar item.
    pub(crate) fn status(&self) -> LanguageServerStatus {
        let name = self.spec.name;
        let (state, label) = match &self.state {
            State::Starting => (LanguageServerState::Starting, format!("{name} starting…")),
            State::Ready => (LanguageServerState::Ready, self.label.clone()),
            State::NotResponding => (LanguageServerState::NotResponding, format!("{name} isn't responding")),
            State::Restarting => (LanguageServerState::Restarting, format!("{name} restarting…")),
            State::Failed(_) => (LanguageServerState::Failed, format!("{name} stopped")),
            State::Stopped => (LanguageServerState::Off, format!("{name} off")),
        };
        LanguageServerStatus { name: name.to_owned(), state, label }
    }

    /// Why the server stopped for good, while it has.
    pub(crate) fn failure(&self) -> Option<&str> {
        match &self.state {
            State::Failed(reason) => Some(reason),
            _ => None,
        }
    }

    /// Brings the server's open documents in line with the open editors:
    /// opens new ones, sends changed text, closes the rest, and pulls
    /// diagnostics where wanted. Only first-class-language files that are
    /// fully read and not large are synced. Called after every command and
    /// background result; cheap when nothing changed.
    pub(crate) fn sync<'a>(&mut self, editors: impl Iterator<Item = &'a Editor>) -> Vec<Output> {
        let mut outputs = Vec::new();
        let Some(connection) = self.connection.as_mut().filter(|_| self.state == State::Ready) else {
            return outputs;
        };
        let mut open = BTreeSet::new();
        let mut changed = false;
        for editor in editors {
            let path = editor.path();
            let Some(language) = text::language_id(path) else { continue };
            if editor.is_large() || !editor.is_loaded() {
                continue;
            }
            open.insert(path.to_owned());
            match self.documents.get_mut(path) {
                None => {
                    let uri = text::uri(&self.root.join(path));
                    connection.open(uri.clone(), language, 1, editor.text().clone());
                    let document = Document {
                        uri,
                        version: 1,
                        editor_version: editor.version(),
                        wanted: true,
                        pulling: None,
                        diagnostics: Vec::new(),
                    };
                    self.documents.insert(path.to_owned(), document);
                }
                Some(document) if document.editor_version != editor.version() => {
                    document.version += 1;
                    document.editor_version = editor.version();
                    connection.change(document.uri.clone(), document.version, editor.text().clone());
                    changed = true;
                }
                Some(_) => {}
            }
        }
        let closed: Vec<PathBuf> = self.documents.keys().filter(|path| !open.contains(*path)).cloned().collect();
        for path in closed {
            if let Some(document) = self.documents.remove(&path) {
                connection.notify("textDocument/didClose", json!({ "textDocument": { "uri": document.uri } }));
                outputs.push(Output::Clear(path));
            }
        }
        // A change in one file can change any open file's diagnostics.
        if changed {
            for document in self.documents.values_mut() {
                document.wanted = true;
            }
        }
        for (path, document) in &mut self.documents {
            if document.wanted && document.pulling.is_none() {
                document.wanted = false;
                let id = connection.request("textDocument/diagnostic", json!({ "textDocument": { "uri": document.uri } }));
                document.pulling = Some(id);
                self.requests.insert(id, Pending::Diagnostics(path.clone()));
            }
        }
        outputs
    }

    /// Sends watched-file changes the server registered for. Matching and
    /// reading the disk happen in the background.
    pub(crate) fn files_changed(&self, paths: &BTreeSet<PathBuf>, created: &BTreeSet<PathBuf>) {
        if self.state != State::Ready || self.watchers.is_empty() || paths.is_empty() {
            return;
        }
        let (watchers, paths, created) = (self.watchers.clone(), paths.clone(), created.clone());
        let (id, generation) = (self.project, self.generation);
        self.jobs.spawn("match watched files", move || {
            let events = watchers.events(&paths, &created);
            Box::new(move |core| {
                if let Some(project) = core.project_mut(id) {
                    project.language_files_events(generation, events);
                }
            })
        });
    }

    /// Sends file events (from [`files_changed`](Self::files_changed)'s
    /// job) if the server is still the one they were matched for.
    pub(crate) fn send_file_events(&self, generation: u64, events: Vec<FileEvent>) {
        if generation != self.generation || events.is_empty() {
            return;
        }
        if let Some(connection) = &self.connection {
            let params = json!({ "changes": events });
            connection.notify("workspace/didChangeWatchedFiles", params);
        }
    }

    /// Handles what the server sent.
    pub(crate) fn event(&mut self, generation: u64, event: Event) -> Vec<Output> {
        if generation != self.generation {
            return Vec::new();
        }
        let outputs = match event {
            Event::Response { id, result } => match self.requests.remove(&id) {
                Some(Pending::Initialize) => self.initialized(result),
                Some(Pending::Diagnostics(path)) => self.diagnostics_answered(path, id, result),
                Some(Pending::Symbols { document }) => {
                    let symbols = result.map(|answer| symbols::parse(&answer, document.as_deref())).unwrap_or_default();
                    vec![Output::Symbols { id, symbols }]
                }
                Some(Pending::CodeActions(ticket)) => self.code_actions_answered(ticket, result),
                None => Vec::new(),
            },
            Event::Message { method, params } => self.message(&method, params),
            Event::Exited { reason } => self.crashed(reason),
        };
        self.remember_diagnostics(&outputs);
        outputs
    }

    /// A timer of this server's came due.
    pub(crate) fn timer(&mut self, generation: u64, timer: Timer, env: &ProcessEnv) -> Vec<Output> {
        if generation != self.generation {
            return Vec::new();
        }
        match (timer, &self.state) {
            (Timer::StartTimeout, _) => {
                self.not_responding();
                Vec::new()
            }
            (Timer::Restart, State::Restarting) => {
                self.restarts.push(self.host.clock().now());
                self.start(env)
            }
            _ => Vec::new(),
        }
    }

    /// A server still starting has taken too long ([`START_TIMEOUT`]): the
    /// status bar says it isn't responding, and `settle` stops waiting for
    /// its `initialize` answer. It keeps running, and is used as soon as it
    /// answers.
    pub(crate) fn not_responding(&mut self) {
        if !matches!(self.state, State::Starting) {
            return;
        }
        self.state = State::NotResponding;
        let initialize = self.requests.iter().find(|(_, p)| matches!(p, Pending::Initialize)).map(|(id, _)| *id);
        if let (Some(connection), Some(id)) = (&self.connection, initialize) {
            connection.forget(id);
        }
    }

    fn initialize_params(&self, folder: WorkspaceFolder) -> Value {
        let capabilities = ClientCapabilities {
            general: Some(GeneralClientCapabilities {
                position_encodings: Some(vec![PositionEncodingKind::UTF8, PositionEncodingKind::UTF16]),
                ..Default::default()
            }),
            workspace: Some(WorkspaceClientCapabilities {
                did_change_watched_files: Some(DidChangeWatchedFilesClientCapabilities {
                    dynamic_registration: Some(true),
                    relative_pattern_support: Some(true),
                }),
                workspace_folders: Some(true),
                configuration: Some(true),
                diagnostics: Some(DiagnosticWorkspaceClientCapabilities { refresh_support: Some(true) }),
                symbol: Some(WorkspaceSymbolClientCapabilities::default()),
                ..Default::default()
            }),
            text_document: Some(TextDocumentClientCapabilities {
                synchronization: Some(TextDocumentSyncClientCapabilities {
                    dynamic_registration: Some(false),
                    did_save: Some(false),
                    ..Default::default()
                }),
                diagnostic: Some(DiagnosticClientCapabilities {
                    dynamic_registration: Some(false),
                    related_document_support: Some(false),
                    ..Default::default()
                }),
                publish_diagnostics: Some(PublishDiagnosticsClientCapabilities {
                    version_support: Some(true),
                    ..Default::default()
                }),
                document_symbol: Some(DocumentSymbolClientCapabilities {
                    hierarchical_document_symbol_support: Some(true),
                    ..Default::default()
                }),
                code_action: Some(actions::client_capabilities()),
                ..Default::default()
            }),
            ..Default::default()
        };
        #[allow(deprecated)] // rootUri: for servers that don't read workspaceFolders.
        let params = InitializeParams {
            process_id: Some(std::process::id() as i32),
            client_info: Some(ClientInfo { name: "Genea".into(), version: Some(env!("CARGO_PKG_VERSION").into()) }),
            root_uri: Some(folder.uri.clone()),
            capabilities,
            initialization_options: Some(self.spec.options.clone()),
            workspace_folders_initialize_params: WorkspaceFoldersInitializeParams {
                workspace_folders: Some(WorkspaceFolders::WorkspaceFolderList(vec![folder])),
            },
            ..Default::default()
        };
        serde_json::to_value(params).expect("initialize params are JSON")
    }

    fn initialized(&mut self, result: Result<Value, ResponseError>) -> Vec<Output> {
        let result = result
            .map_err(|error| format!("it refused to start: {}", error.message))
            .and_then(|result| serde_json::from_value::<InitializeResult>(result).map_err(|e| e.to_string()));
        match result {
            Ok(result) => {
                self.encoding = Encoding::from_lsp(result.capabilities.position_encoding.as_ref().map(|k| k.as_str()));
                if let Some(version) = result.server_info.and_then(|info| info.version) {
                    self.label = format!("{} {version}", self.spec.name);
                }
                self.state = State::Ready;
                if let Some(connection) = &self.connection {
                    connection.notify("initialized", json!({}));
                }
                Vec::new()
            }
            Err(reason) => {
                // Treated like a crash: stop it and let the restart policy decide.
                if let Some(mut connection) = self.connection.take() {
                    connection.stop();
                }
                self.crashed(reason)
            }
        }
    }

    fn diagnostics_answered(&mut self, path: PathBuf, id: i64, result: Result<Value, ResponseError>) -> Vec<Output> {
        let Some(document) = self.documents.get_mut(&path).filter(|d| d.pulling == Some(id)) else { return Vec::new() };
        document.pulling = None;
        match result {
            Ok(report) => match serde_json::from_value::<DocumentDiagnosticReport>(report) {
                Ok(DocumentDiagnosticReport::RelatedFullDocumentDiagnosticReport(report)) => {
                    let diagnostics = report.full_document_diagnostic_report.items;
                    vec![Output::Diagnostics { path, diagnostics }]
                }
                // Unchanged (Genea never sends a previous result id), or garbled.
                _ => Vec::new(),
            },
            Err(error) => {
                let retrigger = error.data["retriggerRequest"].as_bool().unwrap_or(true);
                if matches!(error.code, SERVER_CANCELLED | CONTENT_MODIFIED) && retrigger {
                    document.wanted = true;
                }
                Vec::new()
            }
        }
    }

    fn message(&mut self, method: &str, params: Value) -> Vec<Output> {
        match method {
            "client/registerCapability" => {
                if let Ok(params) = serde_json::from_value::<RegistrationParams>(params) {
                    for registration in &params.registrations {
                        self.watchers.register(registration);
                    }
                }
            }
            "client/unregisterCapability" => {
                if let Ok(params) = serde_json::from_value::<UnregistrationParams>(params) {
                    for unregistration in &params.unregisterations {
                        self.watchers.unregister(&unregistration.id);
                    }
                }
            }
            "workspace/diagnostic/refresh" => {
                for document in self.documents.values_mut() {
                    document.wanted = true;
                }
            }
            // Pushed diagnostics, from a server that sends them anyway.
            "textDocument/publishDiagnostics" => {
                if let Ok(params) = serde_json::from_value::<PublishDiagnosticsParams>(params)
                    && let Some(path) = self.document_path(params.uri.as_ref())
                {
                    return vec![Output::Diagnostics { path, diagnostics: params.diagnostics }];
                }
            }
            _ => {}
        }
        Vec::new()
    }

    /// The open document with this URI, by its path as the editor has it.
    fn document_path(&self, uri: &str) -> Option<PathBuf> {
        self.documents.iter().find(|(_, document)| document.uri == uri).map(|(path, _)| path.clone())
    }

    /// The process is gone without being asked: restart it, unless it has
    /// been restarted [`MAX_RESTARTS`] times within [`RESTART_WINDOW`].
    fn crashed(&mut self, reason: String) -> Vec<Output> {
        let outputs = self.disconnect();
        self.generation = next_generation();
        let now = self.host.clock().now();
        self.restarts.retain(|at| now.duration_since(*at) < RESTART_WINDOW);
        if self.restarts.len() < MAX_RESTARTS {
            self.state = State::Restarting;
            self.after(RESTART_DELAY, Timer::Restart);
        } else {
            self.state = State::Failed(reason);
        }
        outputs
    }

    /// Fires `timer` for the current process after `delay` on the host
    /// clock.
    fn after(&self, delay: Duration, timer: Timer) {
        let (jobs, id, generation) = (self.jobs.clone(), self.project, self.generation);
        self.host.clock().after(
            delay,
            Box::new(move || {
                jobs.busy().finish(Box::new(move |core| {
                    if let Some(project) = core.project_mut(id) {
                        project.language_timer(generation, timer);
                    }
                }))
            }),
        );
    }

    /// Sends a request to a ready server, remembering what it is for; the
    /// answer comes back to [`event`](Self::event) as that [`Pending`].
    /// `None` while the server isn't ready.
    #[allow(dead_code)] // The seam for completion, hover, … (#43–#47).
    pub(crate) fn request(&mut self, method: &str, params: Value, pending: Pending) -> Option<i64> {
        let connection = self.connection.as_mut().filter(|_| self.state == State::Ready)?;
        let id = connection.request(method, params);
        self.requests.insert(id, pending);
        Some(id)
    }
}

/// A generation no server has had: generations are unique across every
/// server, so events can be routed by them (ticket #49).
fn next_generation() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Drop for LanguageServer {
    fn drop(&mut self) {
        self.disconnect();
    }
}
