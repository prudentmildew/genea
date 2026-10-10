//! A project's Oxlint (ticket #49): `oxlint --lsp` from the project's
//! `node_modules`, run on the pinned runtime, with the same lifecycle and
//! restart policy as tsgo (it is another [`LanguageServer`]). Its
//! diagnostics land in Problems as [`ProblemSource::Oxlint`].
//!
//! It starts once Oxlint and Oxfmt are in `package.json`, Oxlint is
//! installed, and the project environment is ready (the runtime is
//! downloaded): unlike tsgo, Oxlint is a Node script. A change to what it
//! runs with (the runtime pin, the Oxlint install, type-aware linting in
//! `.oxlintrc.json`) starts it afresh. Without Oxlint and Oxfmt, a notice
//! offers "Add Oxlint and Oxfmt".
//!
//! Its events and timers come through `language_event` and
//! `language_timer` (`project/language.rs`), routed here by generation
//! ([`LanguageServer::owns`]).

use std::{
    fs,
    path::{Path, PathBuf},
};

use gen_lsp_types::FileEvent;
use genea_host::ProcessSpec;
use serde_json::json;

use super::{Project, language::problem};
use crate::{
    command::Command,
    jobs::Jobs,
    lsp::{
        Event, LanguageServer, Output, ServerSpec, Timer,
        oxc::{self, OXLINT_CONFIGS, OxcDetection, Oxlint},
        text,
    },
    problems::ProblemSource,
    toolchain::Toolchain,
    view::{LanguageServerStatus, Notice, NoticeAction},
    watcher::FileChanges,
};

/// What the status bar and notices call it.
const NAME: &str = "Oxlint";

/// A project's Oxlint state.
#[derive(Default)]
pub(crate) struct Linting {
    /// What the last look for Oxlint and Oxfmt found; `None` until it lands.
    detection: Option<OxcDetection>,
    /// Bumped by every look, so a slow one can't replace a newer one.
    detect_generation: u64,
    /// `oxlint --lsp`, while it runs, and what it was started with.
    server: Option<(LanguageServer, Launch)>,
    /// Why Oxlint can't run on the project's runtime.
    runtime_problem: Option<String>,
    /// Why "Add Oxlint and Oxfmt" failed.
    problem: Option<String>,
}

/// What `oxlint --lsp` runs with: a change starts it afresh.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Launch {
    /// The pinned runtime's executable.
    runtime: PathBuf,
    oxlint: Oxlint,
}

impl Linting {
    /// Whether events and timers of `generation` are Oxlint's.
    pub(super) fn owns(&self, generation: u64) -> bool {
        self.server.as_ref().is_some_and(|(server, _)| server.owns(generation))
    }

    /// `oxlint --lsp`, while it runs (fix on save asks it, ticket #50).
    pub(super) fn server_mut(&mut self) -> Option<&mut LanguageServer> {
        self.server.as_mut().map(|(server, _)| server)
    }
}

impl Project {
    /// Looks for Oxlint and Oxfmt in the background.
    pub(super) fn detect_oxc(&mut self) {
        let Some((_, jobs)) = self.language.context() else { return };
        let linting = &mut self.language.oxlint;
        linting.detect_generation += 1;
        let generation = linting.detect_generation;
        let (id, root) = (self.id, self.root.clone());
        jobs.spawn("find Oxlint and Oxfmt", move || {
            let detection = oxc::detect(&root);
            Box::new(move |core| {
                if let Some(project) = core.project_mut(id)
                    && project.language.oxlint.detect_generation == generation
                {
                    project.language.oxlint.detection = Some(detection);
                    project.sync_oxlint();
                }
            })
        });
    }

    /// Starts, restarts or stops Oxlint for what the project has now, then
    /// brings its documents in line with the open editors. Runs after every
    /// command and every background result (from `sync_language`).
    pub(super) fn sync_oxlint(&mut self) {
        self.update_oxlint();
        let Some((server, _)) = &mut self.language.oxlint.server else { return };
        let editors = self.editor.iter().chain(self.panes.parked());
        let outputs = server.sync(editors);
        self.oxlint_outputs(outputs);
    }

    /// Starts or stops Oxlint if what it should run with has changed. While
    /// the environment isn't ready (a capture or a download is running),
    /// whatever runs keeps running.
    fn update_oxlint(&mut self) {
        let wanted = match &self.language.oxlint.detection {
            Some(OxcDetection::Found(oxlint)) => {
                if !self.environment_ready() {
                    return;
                }
                let runtime = match self.toolchain.as_ref().map(Toolchain::runtime_program) {
                    Some(Ok(runtime)) => Ok(runtime),
                    Some(Err(reason)) => Err(reason),
                    None => Err("Couldn't read package.json, so Genea doesn't know the runtime.".into()),
                };
                match runtime {
                    Ok(runtime) => Some(Launch { runtime, oxlint: oxlint.clone() }),
                    Err(reason) => {
                        self.language.oxlint.runtime_problem = Some(reason);
                        None
                    }
                }
            }
            _ => None,
        };
        if wanted.is_some() {
            self.language.oxlint.runtime_problem = None;
        }
        let running = self.language.oxlint.server.as_ref().map(|(_, launch)| launch);
        if running == wanted.as_ref() {
            return;
        }
        let mut outputs = self.language.oxlint.server.take().map(|(mut old, _)| old.stop()).unwrap_or_default();
        if let Some(launch) = wanted
            && let Some((host, jobs)) = self.language.context()
        {
            let env = self.process_env();
            let mut server = LanguageServer::new(self.id, self.root.clone(), spec(&self.root, &launch), host, jobs);
            outputs.extend(server.start(&env));
            self.language.oxlint.server = Some((server, launch));
        }
        self.oxlint_outputs(outputs);
    }

    /// What Oxlint sent (routed here by `language_event`).
    pub(super) fn oxlint_event(&mut self, generation: u64, event: Event) {
        let Some((server, _)) = &mut self.language.oxlint.server else { return };
        let outputs = server.event(generation, event);
        self.oxlint_outputs(outputs);
    }

    /// One of Oxlint's timers came due (routed here by `language_timer`).
    pub(super) fn oxlint_timer(&mut self, generation: u64, timer: Timer) {
        let env = self.process_env();
        let Some((server, _)) = &mut self.language.oxlint.server else { return };
        let outputs = server.timer(generation, timer, &env);
        self.oxlint_outputs(outputs);
    }

    /// The watcher's changes: watched files go to Oxlint, and changes to
    /// `package.json`, the top of `node_modules` or the root Oxlint config
    /// look for Oxlint and Oxfmt again.
    pub(super) fn oxlint_files_changed(&mut self, changes: &FileChanges) {
        if let Some((server, _)) = &self.language.oxlint.server {
            server.files_changed(&changes.paths, &changes.created);
        }
        let node_modules = self.root.join("node_modules");
        let at_root = |path: &PathBuf, name: &str| path.parent() == Some(self.root.as_path()) && path.ends_with(name);
        let relevant = |path: &PathBuf| {
            at_root(path, "package.json")
                || OXLINT_CONFIGS.iter().any(|config| at_root(path, config))
                || path.strip_prefix(&node_modules).is_ok_and(|rest| rest.components().count() <= 2)
        };
        if changes.rescan || changes.paths.iter().any(relevant) {
            self.detect_oxc();
        }
    }

    /// Watched-file events matched in the background, if they are Oxlint's.
    pub(super) fn oxlint_files_events(&self, generation: u64, events: Vec<FileEvent>) {
        if let Some((server, _)) = &self.language.oxlint.server {
            server.send_file_events(generation, events);
        }
    }

    /// "Restart language server", for Oxlint.
    pub(super) fn restart_oxlint(&mut self) {
        let env = self.process_env();
        let Some((server, _)) = &mut self.language.oxlint.server else {
            // Nothing runs: look again.
            self.detect_oxc();
            return;
        };
        let outputs = server.restart(&env);
        self.oxlint_outputs(outputs);
    }

    /// "Add Oxlint and Oxfmt": writes them to `package.json` in the
    /// background.
    pub(super) fn add_oxc(&mut self, jobs: &Jobs) {
        let (id, path) = (self.id, self.root.join("package.json"));
        jobs.spawn("add Oxlint and Oxfmt", move || {
            let written = fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| oxc::add_oxc(&text))
                .and_then(|text| fs::write(&path, text).map_err(|e| e.to_string()));
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                project.language.oxlint.problem =
                    written.err().map(|reason| format!("Couldn't add Oxlint and Oxfmt to package.json: {reason}"));
                // Don't wait for the watcher: the notice should change at once.
                project.detect_oxc();
            })
        });
    }

    /// Applies what Oxlint asked for: its diagnostics into Problems.
    fn oxlint_outputs(&mut self, outputs: Vec<Output>) {
        let source = ProblemSource::Oxlint;
        for output in outputs {
            match output {
                Output::Diagnostics { path, diagnostics } => {
                    let Some((server, _)) = &self.language.oxlint.server else { continue };
                    let encoding = server.encoding();
                    let Some(editor) = self.open_editor(&path) else { continue };
                    let problems = diagnostics
                        .iter()
                        .filter_map(|diagnostic| problem(&path, editor.text(), diagnostic, encoding))
                        .collect();
                    self.problems.replace_file(source, &path, problems);
                }
                Output::Clear(path) => self.problems.replace_file(source, &path, Vec::new()),
                Output::ClearAll => self.problems.replace(source, Vec::new()),
                // Fix on save's safe fixes (ticket #50).
                Output::CodeActions { ticket, fixes } => self.oxlint_code_actions(ticket, fixes),
                #[allow(unreachable_patterns)] // Requests Oxlint isn't sent yet (#45, #50).
                _ => {}
            }
        }
    }

    /// Oxlint's status-bar item, while it runs. When lint is off, a notice
    /// says why instead.
    pub(super) fn oxlint_status(&self) -> Option<LanguageServerStatus> {
        self.language.oxlint.server.as_ref().map(|(server, _)| server.status())
    }

    /// Why lint is off, or why Oxlint stopped.
    pub(super) fn oxlint_notices(&self) -> Vec<Notice> {
        let linting = &self.language.oxlint;
        let mut notices = Vec::new();
        match &linting.detection {
            Some(OxcDetection::Missing) => notices.push(Notice {
                message: "Lint and format are off: this project doesn't have Oxlint and Oxfmt.".into(),
                action: Some(NoticeAction { label: "Add Oxlint and Oxfmt".into(), command: Command::AddOxlintAndOxfmt }),
            }),
            Some(OxcDetection::NotInstalled) => notices.push(Notice {
                message: "Lint is off until Oxlint is installed: install the project's dependencies.".into(),
                action: None,
            }),
            Some(OxcDetection::Found(_)) if linting.server.is_none() => {
                if let Some(reason) = &linting.runtime_problem {
                    notices.push(Notice { message: format!("Lint is off: Oxlint runs on the project's runtime. {reason}"), action: None });
                }
            }
            _ => {}
        }
        if let Some(reason) = linting.server.as_ref().and_then(|(server, _)| server.failure()) {
            notices.push(Notice {
                message: format!("The Oxlint language server stopped after crashing repeatedly ({reason})."),
                action: Some(NoticeAction { label: "Restart language server".into(), command: Command::RestartLanguageServer }),
            });
        }
        if let Some(problem) = &linting.problem {
            notices.push(Notice { message: problem.clone(), action: None });
        }
        notices
    }
}

/// How `oxlint --lsp` starts: the launcher script on the runtime, with the
/// project root as its workspace. Type-aware linting is passed explicitly,
/// so it is on exactly when the root `.oxlintrc.json` turns it on.
fn spec(root: &Path, launch: &Launch) -> ServerSpec {
    let Launch { runtime, oxlint } = launch;
    ServerSpec {
        name: NAME,
        process: ProcessSpec::new(runtime).arg(&oxlint.script).arg("--lsp"),
        options: json!([{
            "workspaceUri": text::uri(root),
            "options": { "typeAware": oxlint.type_aware },
        }]),
        label: format!("{NAME} {}", oxlint.version),
        languages: text::language_id,
        pull_diagnostics: true,
    }
}
