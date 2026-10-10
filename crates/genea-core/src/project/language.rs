//! A project's language intelligence (ticket #42): finding TypeScript 7,
//! running tsgo with the project's lifecycle, keeping it in step with the
//! open editors and the watcher, and turning its diagnostics into Problems.
//! The protocol side is in `crate::lsp`.

use std::{
    fs,
    path::{Path, PathBuf},
};

use gen_lsp_types::{Diagnostic, DiagnosticSeverity, FileEvent, Message};
use genea_host::{ProcessSpec, SharedHost};
use serde_json::json;

use super::{Project, oxlint::Linting};
use crate::{
    command::Command,
    jobs::Jobs,
    lsp::{
        Event, LanguageServer, Output, Pending, START_TIMEOUT, ServerSpec, Timer,
        text::{self, Encoding},
        typescript::{self, Detection},
    },
    problems::{Problem, ProblemSource, Severity},
    review::store::hash_bytes,
    view::{LanguageServerState, LanguageServerStatus, Notice, NoticeAction},
    watcher::FileChanges,
};

/// The language-intelligence state of a project.
#[derive(Default)]
pub(crate) struct Language {
    /// The host and jobs, from `start_language`.
    context: Option<(SharedHost, Jobs)>,
    /// What the last look for TypeScript 7 found; `None` until it lands.
    detection: Option<Detection>,
    /// Bumped by every look, so a slow one can't replace a newer one.
    detect_generation: u64,
    /// tsgo, while TypeScript 7 is installed.
    typescript: Option<LanguageServer>,
    /// Why "Add TypeScript 7" failed.
    problem: Option<String>,
    /// [`START_TIMEOUT`] passed after the open before the first look for
    /// TypeScript 7 landed: tsgo starts as not responding.
    start_overdue: bool,
    /// Oxlint (ticket #49, `project/oxlint.rs`).
    pub(super) oxlint: Linting,
}

impl Language {
    /// The host and jobs, once `start_language` has run.
    pub(super) fn context(&self) -> Option<(SharedHost, Jobs)> {
        self.context.clone()
    }
}

impl Project {
    /// Starts language intelligence when the project opens: looks for
    /// TypeScript 7 in the background, and starts tsgo if it is there.
    pub(crate) fn start_language(&mut self, host: SharedHost, jobs: &Jobs) {
        // The start timeout counts from the open, so it runs on the host
        // clock from here, however late the look for TypeScript lands.
        let (id, timer_jobs) = (self.id, jobs.clone());
        host.clock().after(
            START_TIMEOUT,
            Box::new(move || {
                timer_jobs.busy().finish(Box::new(move |core| {
                    if let Some(project) = core.project_mut(id) {
                        project.language_start_overdue();
                    }
                }))
            }),
        );
        self.language.context = Some((host, jobs.clone()));
        self.detect_typescript();
        self.detect_oxc();
    }

    /// [`START_TIMEOUT`] has passed since the project opened.
    fn language_start_overdue(&mut self) {
        match &mut self.language.typescript {
            Some(server) => server.not_responding(),
            None => self.language.start_overdue = self.language.detection.is_none(),
        }
    }

    fn detect_typescript(&mut self) {
        let Some((_, jobs)) = &self.language.context else { return };
        self.language.detect_generation += 1;
        let generation = self.language.detect_generation;
        let (id, root) = (self.id, self.root.clone());
        jobs.spawn("find TypeScript 7", move || {
            let detection = typescript::detect(&root);
            Box::new(move |core| {
                if let Some(project) = core.project_mut(id)
                    && project.language.detect_generation == generation
                {
                    project.typescript_detected(detection);
                }
            })
        });
    }

    /// The project's TypeScript 7 `tsc` (the binary tsgo runs from), with
    /// the host and jobs to run it: what the project check (ticket #48)
    /// needs. `None` until TypeScript 7 is found.
    pub(super) fn typescript_tsc(&self) -> Option<(PathBuf, SharedHost, Jobs)> {
        let Some(Detection::Found { binary, .. }) = &self.language.detection else { return None };
        let (host, jobs) = self.language.context.clone()?;
        Some((binary.clone(), host, jobs))
    }

    /// Starts, restarts or stops tsgo for what the look found.
    fn typescript_detected(&mut self, detection: Detection) {
        if self.language.detection.as_ref() == Some(&detection) {
            return;
        }
        let outputs = match &detection {
            Detection::Found { binary, version } => {
                let Some((host, jobs)) = self.language.context.clone() else { return };
                let spec = ServerSpec {
                    name: "TypeScript",
                    process: ProcessSpec::new(binary).args(["--lsp", "--stdio"]),
                    // Genea pulls diagnostics; pushing them too would be work for nothing.
                    options: json!({ "disablePushDiagnostics": true }),
                    label: format!("TypeScript {version}"),
                };
                let env = self.process_env();
                let mut server = LanguageServer::new(self.id, self.root.clone(), spec, host, jobs);
                let mut outputs = server.start(&env);
                if std::mem::take(&mut self.language.start_overdue) {
                    server.not_responding();
                }
                if let Some(mut old) = self.language.typescript.replace(server) {
                    outputs.extend(old.stop());
                }
                outputs
            }
            _ => self.language.typescript.take().map(|mut server| server.stop()).unwrap_or_default(),
        };
        self.language.start_overdue = false;
        self.language.detection = Some(detection);
        self.language_outputs(outputs);
    }

    /// What a language server sent (from its connection's batch).
    pub(crate) fn language_event(&mut self, generation: u64, event: Event) {
        if self.language.oxlint.owns(generation) {
            return self.oxlint_event(generation, event);
        }
        let Some(server) = &mut self.language.typescript else { return };
        let outputs = server.event(generation, event);
        self.language_outputs(outputs);
    }

    /// A language server's timer came due.
    pub(crate) fn language_timer(&mut self, generation: u64, timer: Timer) {
        if self.language.oxlint.owns(generation) {
            return self.oxlint_timer(generation, timer);
        }
        let env = self.process_env();
        let Some(server) = &mut self.language.typescript else { return };
        let outputs = server.timer(generation, timer, &env);
        self.language_outputs(outputs);
    }

    /// Brings the language server's documents in line with the open
    /// editors. Runs after every command and every background result.
    pub(crate) fn sync_language(&mut self) {
        self.sync_oxlint();
        let Some(server) = &mut self.language.typescript else { return };
        let editors = self.editor.iter().chain(self.panes.parked());
        let outputs = server.sync(editors);
        self.language_outputs(outputs);
        // Requests go out after the sync, so the server has the current text.
        self.request_symbols();
    }

    /// Sends a request to tsgo while it is ready; `None` otherwise.
    pub(super) fn language_request(&mut self, method: &str, params: serde_json::Value, pending: Pending) -> Option<i64> {
        self.language.typescript.as_mut()?.request(method, params, pending)
    }

    /// The columns tsgo counts in.
    pub(super) fn language_encoding(&self) -> Encoding {
        self.language.typescript.as_ref().map(LanguageServer::encoding).unwrap_or_default()
    }

    /// The watcher's changes: watched files go to the server, and changes
    /// to `package.json` or the top of `node_modules` look for TypeScript
    /// 7 again (it may have been installed or removed).
    pub(super) fn language_files_changed(&mut self, changes: &FileChanges) {
        self.oxlint_files_changed(changes);
        if let Some(server) = &self.language.typescript {
            server.files_changed(&changes.paths, &changes.created);
        }
        let node_modules = self.root.join("node_modules");
        let package_json = self.root.join("package.json");
        let relevant = |path: &PathBuf| {
            *path == package_json
                || path.strip_prefix(&node_modules).is_ok_and(|rest| {
                    rest.components().count() <= 2 || rest.starts_with("@typescript")
                })
        };
        if changes.rescan || changes.paths.iter().any(relevant) {
            self.detect_typescript();
        }
    }

    /// Watched-file events matched in the background.
    pub(crate) fn language_files_events(&mut self, generation: u64, events: Vec<FileEvent>) {
        if self.language.oxlint.owns(generation) {
            return self.oxlint_files_events(generation, events);
        }
        if let Some(server) = &self.language.typescript {
            server.send_file_events(generation, events);
        }
    }

    /// "Restart language server".
    pub(super) fn restart_language_server(&mut self) {
        let env = self.process_env();
        let Some(server) = &mut self.language.typescript else {
            // Nothing runs (no TypeScript 7 yet): look again.
            self.language.detection = None;
            self.detect_typescript();
            return;
        };
        let outputs = server.restart(&env);
        self.language_outputs(outputs);
    }

    /// "Add TypeScript 7": writes it to `package.json` in the background.
    pub(super) fn add_typescript(&mut self, jobs: &Jobs) {
        let (id, path) = (self.id, self.root.join("package.json"));
        let own_writes = self.review.own_writes();
        jobs.spawn("add TypeScript 7", move || {
            let written = fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| typescript::add_typescript(&text))
                .and_then(|text| {
                    // Review knows this write as Genea's own (ticket #53).
                    let _writing = own_writes.writing(Path::new("package.json"), Some(hash_bytes(text.as_bytes())));
                    fs::write(&path, text).map_err(|e| e.to_string())
                });
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                project.language.problem =
                    written.err().map(|reason| format!("Couldn't add TypeScript 7 to package.json: {reason}"));
                // Don't wait for the watcher: the notice should change at once.
                project.detect_typescript();
            })
        });
    }

    /// Applies what a language server asked for: its diagnostics into
    /// Problems.
    fn language_outputs(&mut self, outputs: Vec<Output>) {
        let source = ProblemSource::TypeScript;
        for output in outputs {
            match output {
                Output::Diagnostics { path, diagnostics } => {
                    let encoding = self.language.typescript.as_ref().map(LanguageServer::encoding).unwrap_or_default();
                    let Some(editor) = self.open_editor(&path) else { continue };
                    let problems = diagnostics
                        .iter()
                        .filter_map(|diagnostic| problem(&path, editor.text(), diagnostic, encoding))
                        .collect();
                    self.problems.replace_file(source, &path, problems);
                }
                Output::Clear(path) => self.problems.replace_file(source, &path, Vec::new()),
                Output::ClearAll => self.problems.replace(source, Vec::new()),
                Output::Symbols { id, symbols } => self.symbols_answered(id, symbols),
            }
        }
    }

    /// The language servers' status-bar items.
    pub(super) fn language_status(&self) -> Vec<LanguageServerStatus> {
        match (&self.language.typescript, &self.language.detection) {
            (Some(server), _) => vec![server.status()],
            (None, Some(Detection::Missing { .. } | Detection::NotInstalled)) => vec![LanguageServerStatus {
                name: "TypeScript".into(),
                state: LanguageServerState::Off,
                label: "TypeScript off".into(),
            }],
            _ => Vec::new(),
        }
    }

    /// The language notices: why language intelligence is off, or why a
    /// server stopped.
    pub(super) fn language_notices(&self) -> Vec<Notice> {
        let mut notices = Vec::new();
        let add = NoticeAction { label: "Add TypeScript 7".into(), command: Command::AddTypeScript };
        match &self.language.detection {
            Some(Detection::Missing { older: None }) => notices.push(Notice {
                message: "Language intelligence is off: this project doesn't have TypeScript 7.".into(),
                action: Some(add),
            }),
            Some(Detection::Missing { older: Some(version) }) => notices.push(Notice {
                message: format!(
                    "Language intelligence is off: it needs TypeScript 7, and this project has TypeScript {version}."
                ),
                action: Some(add),
            }),
            Some(Detection::NotInstalled) => notices.push(Notice {
                message: "Language intelligence is off until TypeScript 7 is installed: install the project's dependencies."
                    .into(),
                action: None,
            }),
            _ => {}
        }
        if let Some(reason) = self.language.typescript.as_ref().and_then(LanguageServer::failure) {
            notices.push(Notice {
                message: format!("The TypeScript language server stopped after crashing repeatedly ({reason})."),
                action: Some(NoticeAction { label: "Restart language server".into(), command: Command::RestartLanguageServer }),
            });
        }
        if let Some(problem) = &self.language.problem {
            notices.push(Notice { message: problem.clone(), action: None });
        }
        notices
    }
}

/// A server's diagnostic as a Problem in `path`, or `None` for information
/// and hints, which Problems doesn't show.
pub(super) fn problem(path: &Path, text: &ropey::Rope, diagnostic: &Diagnostic, encoding: Encoding) -> Option<Problem> {
    let severity = match diagnostic.severity {
        None | Some(DiagnosticSeverity::Error) => Severity::Error,
        Some(DiagnosticSeverity::Warning) => Severity::Warning,
        Some(_) => return None,
    };
    let message = match &diagnostic.message {
        Message::String(message) => message.clone(),
        Message::MarkupContent(markup) => markup.value.clone(),
    };
    let range = &diagnostic.range;
    Some(Problem {
        severity,
        path: path.to_owned(),
        start: text::text_position(text, range.start.line, range.start.character, encoding),
        end: text::text_position(text, range.end.line, range.end.character, encoding),
        message,
    })
}
