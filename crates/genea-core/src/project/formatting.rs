//! Format and fix on save (ticket #50): a project's Oxfmt (`oxfmt --lsp`
//! from `node_modules`, on the pinned runtime, another [`LanguageServer`]
//! with tsgo's lifecycle), and what saving does with it and with Oxlint.
//!
//! Saving a file runs three steps, each after the one before has answered:
//!
//! 1. **Format**: Oxfmt formats the file (`textDocument/formatting`), when
//!    `formatOnSave` is on and Oxfmt has the file: the first-class
//!    languages, JSON, CSS, YAML, Markdown and HTML, never `.env`.
//! 2. **Fix**: Oxlint's safe fixes (`source.fixAll.oxc`), when `fixOnSave`
//!    is on and Oxlint has the file.
//! 3. **Write**: the buffer as it is then.
//!
//! The format and fix edits apply to the buffer as undo steps, so the
//! buffer is what was written. An answer to a buffer that changed since it
//! was asked is dropped. If the servers haven't answered within
//! [`FORMAT_TIMEOUT`] of the save on the host clock, the file is written as
//! it is and a notice says what was skipped. A step whose server isn't
//! running (or isn't ready) is skipped silently. tsgo's formatter is never
//! used.
//!
//! "Reformat file" (⌥⌘L) runs step 1 alone, whatever `formatOnSave` says.
//!
//! Turning a step off for other reasons (#51: a foreign formatter or
//! linter's config) belongs in [`Project::format_on_save`] and
//! [`Project::fix_on_save`].

use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use gen_lsp_types::FileEvent;
use genea_host::ProcessSpec;
use serde_json::json;

use super::Project;
use crate::{
    editor::Editor,
    jobs::Jobs,
    lsp::{
        Event, LanguageServer, Output, ServerSpec, Timer,
        actions::{self, CodeActionFix, TextEdit},
        formatting::FIX_ALL,
        oxc::{self, Oxfmt},
        text::Encoding,
    },
    toolchain::Toolchain,
    view::{LanguageServerStatus, Notice, NoticeAction},
    watcher::FileChanges,
    command::Command,
};

/// How long a save waits for Oxfmt and Oxlint to answer, on the host
/// clock, before it writes the file as it is (spec #19: about 1 s).
pub const FORMAT_TIMEOUT: Duration = Duration::from_secs(1);

/// What the status bar and notices call it.
const NAME: &str = "Oxfmt";

/// A project's Oxfmt, and the saves waiting on it or on Oxlint.
#[derive(Default)]
pub(crate) struct Formatting {
    /// What the last look for Oxfmt found (`Some(None)`: not there); `None`
    /// until it lands.
    detection: Option<Option<Oxfmt>>,
    /// Bumped by every look, so a slow one can't replace a newer one.
    detect_generation: u64,
    /// `oxfmt --lsp`, while it runs, and what it was started with.
    server: Option<(LanguageServer, Launch)>,
    /// Saves waiting for an answer.
    saves: Vec<Save>,
    /// "Reformat file" requests waiting for an answer.
    reformats: Vec<Asked>,
}

/// What `oxfmt --lsp` runs with: a change starts it afresh.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Launch {
    /// The pinned runtime's executable.
    runtime: PathBuf,
    oxfmt: Oxfmt,
}

/// A save under way.
struct Save {
    /// The file (relative path).
    path: PathBuf,
    /// Its timeout's id.
    id: u64,
    /// The request it waits for.
    asked: Asked,
    step: Step,
}

/// A save's step that is waiting for a server.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    Format,
    Fix,
}

/// A request a save or "Reformat file" waits for.
struct Asked {
    path: PathBuf,
    /// The request, for giving up on it.
    request: i64,
    /// Matches the answer.
    ticket: u64,
    /// The file's editor version when it was asked: an answer applies only
    /// to that.
    version: u64,
}

/// Tickets for the requests here, apart from the quick fixes' tickets (an
/// Oxlint code-action answer is told apart by its ticket).
fn next_ticket() -> u64 {
    static NEXT: AtomicU64 = AtomicU64::new(1 << 48);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

impl Formatting {
    /// Whether events and timers of `generation` are Oxfmt's.
    pub(super) fn owns(&self, generation: u64) -> bool {
        self.server.as_ref().is_some_and(|(server, _)| server.owns(generation))
    }
}

impl Project {
    // --- Saving ---------------------------------------------------------------

    /// Saves an open file: formats it, applies Oxlint's safe fixes, then
    /// writes it (see the module docs). A second save of a file while one
    /// is under way does nothing: that one writes the buffer as it is when
    /// it is done.
    pub(super) fn save(&mut self, path: PathBuf, jobs: &Jobs) {
        if self.open_editor(&path).is_none_or(Editor::is_read_only)
            || self.language.oxfmt.saves.iter().any(|save| save.path == path)
        {
            return;
        }
        // The servers must have the text as it is now.
        self.sync_language();
        let id = next_ticket();
        if self.save_step(&path, id, Step::Format) {
            self.save_timeout(id);
        } else {
            self.write_file(path, jobs);
        }
    }

    /// Whether `formatOnSave` applies to the project.
    pub(super) fn format_on_save(&self) -> bool {
        self.config.format_on_save && !self.has_foreign_formatter()
    }

    /// Whether `fixOnSave` applies to the project.
    pub(super) fn fix_on_save(&self) -> bool {
        self.config.fix_on_save && !self.has_foreign_formatter()
    }

    /// Asks for `step` of save `id`, or else the steps after it. Returns
    /// whether a request went out (the save waits for it); `false` means
    /// the file should be written now.
    fn save_step(&mut self, path: &Path, id: u64, step: Step) -> bool {
        let mut asked = None;
        if step == Step::Format && self.format_on_save() {
            asked = self.ask_format(path).map(|asked| (asked, Step::Format));
        }
        if asked.is_none() && self.fix_on_save() {
            asked = self.ask_fix(path).map(|asked| (asked, Step::Fix));
        }
        let Some((asked, step)) = asked else { return false };
        let save = Save { path: path.to_owned(), id, asked, step };
        self.language.oxfmt.saves.push(save);
        true
    }

    /// Asks Oxfmt to format an open file it has.
    fn ask_format(&mut self, path: &Path) -> Option<Asked> {
        let indentation = self.indentation_of(path);
        let version = self.open_editor(path)?.version();
        let (server, _) = self.language.oxfmt.server.as_mut()?;
        let ticket = next_ticket();
        let request = server.format(path, indentation.tab_width, !indentation.use_tabs, ticket)?;
        Some(Asked { path: path.to_owned(), request, ticket, version })
    }

    /// Asks Oxlint for an open file's safe fixes.
    fn ask_fix(&mut self, path: &Path) -> Option<Asked> {
        let editor = self.open_editor(path)?;
        let (text, version) = (editor.text().clone(), editor.version());
        let server = self.language.oxlint.server_mut()?;
        let ticket = next_ticket();
        let request = server.fix_all(path, &text, ticket)?;
        Some(Asked { path: path.to_owned(), request, ticket, version })
    }

    /// Gives a save [`FORMAT_TIMEOUT`] on the host clock.
    fn save_timeout(&self, id: u64) {
        let Some((host, jobs)) = self.language.context() else { return };
        let project = self.id;
        host.clock().after(
            FORMAT_TIMEOUT,
            Box::new(move || {
                jobs.busy().finish(Box::new(move |core| {
                    if let Some(project) = core.project_mut(project) {
                        project.save_timed_out(id);
                    }
                }))
            }),
        );
    }

    /// A save's time is up: it writes the file as it is, and says what it
    /// skipped.
    fn save_timed_out(&mut self, id: u64) {
        let saves = &mut self.language.oxfmt.saves;
        let Some(index) = saves.iter().position(|save| save.id == id) else { return };
        let save = saves.remove(index);
        let server = match save.step {
            Step::Format => self.language.oxfmt.server.as_ref().map(|(server, _)| server),
            Step::Fix => self.language.oxlint.server_mut().map(|server| &*server),
        };
        if let Some(server) = server {
            server.forget(save.asked.request);
        }
        let name = save.path.display();
        let message = match save.step {
            Step::Format => format!("Saved {name} unformatted: Oxfmt didn't answer within a second."),
            Step::Fix => format!("Saved {name} without lint fixes: Oxlint didn't answer within a second."),
        };
        self.notify(message);
        if let Some((_, jobs)) = self.language.context() {
            self.write_file(save.path, &jobs);
        }
    }

    /// The save waiting at `step` for the answer with `ticket`, taken out.
    fn take_save(&mut self, step: Step, ticket: u64) -> Option<Save> {
        let saves = &mut self.language.oxfmt.saves;
        let index = saves.iter().position(|save| save.step == step && save.asked.ticket == ticket)?;
        Some(saves.remove(index))
    }

    /// Oxfmt's answer: a save goes on to its next step, or "Reformat file"
    /// applies it.
    fn formatted(&mut self, ticket: u64, edits: Vec<TextEdit>) {
        let encoding = self.language.oxfmt.server.as_ref().map(|(server, _)| server.encoding()).unwrap_or_default();
        let reformats = &mut self.language.oxfmt.reformats;
        if let Some(index) = reformats.iter().position(|asked| asked.ticket == ticket) {
            let asked = reformats.remove(index);
            self.apply_server_edits(&asked, &edits, encoding);
            return;
        }
        let Some(save) = self.take_save(Step::Format, ticket) else { return };
        self.apply_server_edits(&save.asked, &edits, encoding);
        self.next_save_step(save, Step::Fix);
    }

    /// Oxlint's code actions for `ticket`, if a save asked for them: it
    /// applies the safe fixes and writes. Returns whether they were a
    /// save's.
    fn fixes_answered(&mut self, ticket: u64, fixes: &[CodeActionFix]) -> bool {
        let Some(save) = self.take_save(Step::Fix, ticket) else { return false };
        let file = self.root.join(&save.path);
        let fix_all = fixes.iter().find(|fix| fix.kind == FIX_ALL || fix.kind == "source.fixAll");
        if let Some(fix) = fix_all
            && let Some((_, edits)) = fix.edits.iter().find(|(path, _)| *path == file)
        {
            self.apply_server_edits(&save.asked, edits, fix.encoding);
        }
        self.write_save(save);
        true
    }

    /// Asks for the steps of `save` from `step` on, or writes the file.
    fn next_save_step(&mut self, save: Save, step: Step) {
        // The servers must have the text with the edits so far.
        self.sync_language();
        if !self.save_step(&save.path, save.id, step) {
            self.write_save(save);
        }
    }

    fn write_save(&mut self, save: Save) {
        if let Some((_, jobs)) = self.language.context() {
            self.write_file(save.path, &jobs);
        }
    }

    /// Applies a server's edits to the file it was asked about, as one undo
    /// step, unless the file changed since.
    fn apply_server_edits(&mut self, asked: &Asked, edits: &[TextEdit], encoding: Encoding) {
        let Some((host, jobs)) = self.language.context() else { return };
        let rows = self.viewport_rows;
        let Some(editor) = self.open_editor_mut(&asked.path).filter(|e| e.version() == asked.version) else { return };
        let ranges = edits.iter().map(|edit| (actions::char_range(editor.text(), edit, encoding), edit.text.clone())).collect();
        editor.apply_text_edits(ranges, host.clock().now(), rows);
        self.reparse_file(&asked.path, &jobs);
        self.diff_file(&asked.path, &jobs);
    }

    /// "Reformat file" (⌥⌘L): formats the focused file, whatever
    /// `formatOnSave` says.
    pub(super) fn reformat_file(&mut self) {
        let Some(path) = self.editor.as_ref().filter(|e| !e.is_read_only()).map(|e| e.path().to_owned()) else {
            return;
        };
        self.sync_language();
        if let Some(asked) = self.ask_format(&path) {
            self.language.oxfmt.reformats.push(asked);
        }
    }

    // --- The Oxfmt server ---------------------------------------------------------

    /// Looks for Oxfmt in the background.
    pub(super) fn detect_oxfmt(&mut self) {
        let Some((_, jobs)) = self.language.context() else { return };
        let formatting = &mut self.language.oxfmt;
        formatting.detect_generation += 1;
        let generation = formatting.detect_generation;
        let (id, root) = (self.id, self.root.clone());
        jobs.spawn("find Oxfmt", move || {
            let detection = oxc::detect_oxfmt(&root);
            Box::new(move |core| {
                if let Some(project) = core.project_mut(id)
                    && project.language.oxfmt.detect_generation == generation
                {
                    project.language.oxfmt.detection = Some(detection);
                    project.sync_oxfmt();
                }
            })
        });
    }

    /// Starts, restarts or stops Oxfmt for what the project has now, then
    /// brings its documents in line with the open editors. Runs after every
    /// command and every background result (from `sync_language`).
    pub(super) fn sync_oxfmt(&mut self) {
        self.update_oxfmt();
        let Some((server, _)) = &mut self.language.oxfmt.server else { return };
        let editors = self.editor.iter().chain(self.panes.parked());
        let outputs = server.sync(editors);
        self.oxfmt_outputs(outputs);
    }

    /// Starts or stops Oxfmt if what it should run with has changed. While
    /// the environment isn't ready, whatever runs keeps running.
    fn update_oxfmt(&mut self) {
        let wanted = match &self.language.oxfmt.detection {
            Some(Some(oxfmt)) => {
                if !self.environment_ready() {
                    return;
                }
                // Why Oxfmt can't run on the runtime is the Oxlint notice's to say.
                match self.toolchain.as_ref().map(Toolchain::runtime_program) {
                    Some(Ok(runtime)) => Some(Launch { runtime, oxfmt: oxfmt.clone() }),
                    _ => None,
                }
            }
            _ => None,
        };
        let running = self.language.oxfmt.server.as_ref().map(|(_, launch)| launch);
        if running == wanted.as_ref() {
            return;
        }
        let mut outputs = self.language.oxfmt.server.take().map(|(mut old, _)| old.stop()).unwrap_or_default();
        if let Some(launch) = wanted
            && let Some((host, jobs)) = self.language.context()
        {
            let env = self.process_env();
            let mut server = LanguageServer::new(self.id, self.root.clone(), spec(&launch), host, jobs);
            outputs.extend(server.start(&env));
            self.language.oxfmt.server = Some((server, launch));
        }
        self.oxfmt_outputs(outputs);
    }

    /// What Oxfmt sent (routed here by `language_event`).
    pub(super) fn oxfmt_event(&mut self, generation: u64, event: Event) {
        let Some((server, _)) = &mut self.language.oxfmt.server else { return };
        let outputs = server.event(generation, event);
        self.oxfmt_outputs(outputs);
    }

    /// One of Oxfmt's timers came due (routed here by `language_timer`).
    pub(super) fn oxfmt_timer(&mut self, generation: u64, timer: Timer) {
        let env = self.process_env();
        let Some((server, _)) = &mut self.language.oxfmt.server else { return };
        let outputs = server.timer(generation, timer, &env);
        self.oxfmt_outputs(outputs);
    }

    /// The watcher's changes: watched files (`.oxfmtrc.json`) go to Oxfmt,
    /// and changes to `package.json` or the top of `node_modules` look for
    /// Oxfmt again.
    pub(super) fn oxfmt_files_changed(&mut self, changes: &FileChanges) {
        if let Some((server, _)) = &self.language.oxfmt.server {
            server.files_changed(&changes.paths, &changes.created);
        }
        let node_modules = self.root.join("node_modules");
        let package_json = self.root.join("package.json");
        let relevant = |path: &PathBuf| {
            *path == package_json || path.strip_prefix(&node_modules).is_ok_and(|rest| rest.components().count() <= 2)
        };
        if changes.rescan || changes.paths.iter().any(relevant) {
            self.detect_oxfmt();
        }
    }

    /// Watched-file events matched in the background, if they are Oxfmt's.
    pub(super) fn oxfmt_files_events(&self, generation: u64, events: Vec<FileEvent>) {
        if let Some((server, _)) = &self.language.oxfmt.server {
            server.send_file_events(generation, events);
        }
    }

    /// "Restart language server", for Oxfmt.
    pub(super) fn restart_oxfmt(&mut self) {
        let env = self.process_env();
        let Some((server, _)) = &mut self.language.oxfmt.server else {
            // Nothing runs: look again.
            self.detect_oxfmt();
            return;
        };
        let outputs = server.restart(&env);
        self.oxfmt_outputs(outputs);
    }

    /// Applies what Oxfmt sent: formatting answers. It has no diagnostics.
    fn oxfmt_outputs(&mut self, outputs: Vec<Output>) {
        for output in outputs {
            if let Output::Formatted { ticket, edits } = output {
                self.formatted(ticket, edits);
            }
        }
    }

    /// Oxlint's answer to a code-action request: a save's fixes, or else
    /// the quick fixes'.
    pub(super) fn oxlint_code_actions(&mut self, ticket: u64, fixes: Vec<CodeActionFix>) {
        if !self.fixes_answered(ticket, &fixes) {
            self.code_actions_answered(ticket, fixes);
        }
    }

    /// Oxfmt's status-bar item, while it runs.
    pub(super) fn oxfmt_status(&self) -> Option<LanguageServerStatus> {
        self.language.oxfmt.server.as_ref().map(|(server, _)| server.status())
    }

    /// Why Oxfmt stopped, if it did.
    pub(super) fn oxfmt_notices(&self) -> Vec<Notice> {
        let failure = self.language.oxfmt.server.as_ref().and_then(|(server, _)| server.failure());
        failure
            .map(|reason| Notice {
                message: format!("The Oxfmt language server stopped after crashing repeatedly ({reason})."),
                action: Some(NoticeAction { label: "Restart language server".into(), command: Command::RestartLanguageServer }),
            })
            .into_iter()
            .collect()
    }
}

/// How `oxfmt --lsp` starts: the launcher script on the runtime, with the
/// project root as its workspace. It gets every file it formats, and has no
/// diagnostics to pull.
fn spec(launch: &Launch) -> ServerSpec {
    let Launch { runtime, oxfmt } = launch;
    ServerSpec {
        name: NAME,
        process: ProcessSpec::new(runtime).arg(&oxfmt.script).arg("--lsp"),
        options: json!(null),
        label: format!("{NAME} {}", oxfmt.version),
        languages: oxc::oxfmt_language_id,
        pull_diagnostics: false,
    }
}
