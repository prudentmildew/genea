//! Go to definition, type definition and implementation, find usages and
//! rename (ticket #44), on the project's language server.
//!
//! Requests go out at the focused file's primary caret, after a sync, so
//! the server has the text the user sees. Answers name places by absolute
//! path and position in the server's encoding; turning them into text
//! positions needs the files' text, which for files that aren't open is on
//! disk, so that happens in the background ("find places"). A newer
//! request makes an older one's answer moot (`Navigation::generation`).
//!
//! What the user sees: one place opens in a tab (`open_file`), several are
//! listed in the Usages view in the left column, none sets a hint.
//!
//! Rename goes `prepareRename` (is there something to rename, and what is
//! it called) → the prompt → `rename`. Its edits apply to open files as one
//! undo step each, only if no open file changed since the request went out;
//! files that aren't open are read in the background and open in tabs
//! behind the focused one, edited and unsaved. Nothing is written to disk
//! but by saving, so every write is Genea's own.

use std::{
    fs,
    path::{Path, PathBuf},
    time::Instant,
};

use ropey::Rope;

use super::super::Project;
use crate::{
    command::Command,
    editor::Editor,
    jobs::Jobs,
    lsp::{
        LanguageServer, Pending,
        navigation::{self, Answer, Ask, Locate, Location, Position, TextEdit},
        text::{self, Encoding},
    },
    problems::TextPosition,
    search,
    view::{LeftColumnView, RenamePrompt, SearchFile, UsagesView},
};

/// A project's navigation state.
#[derive(Default)]
pub(crate) struct Navigation {
    /// Bumped by every request, so only the newest answer counts.
    generation: u64,
    /// The word at the caret when the newest request went out, for titles.
    word: String,
    usages: Option<UsagesView>,
    hint: Option<String>,
    /// The rename under way, from ⇧F6 until its edits apply.
    renaming: Option<Renaming>,
}

struct Renaming {
    /// The request it waits for.
    generation: u64,
    /// The focused file when ⇧F6 was pressed, its version, and where the
    /// caret was.
    path: PathBuf,
    version: u64,
    position: Position,
    /// The symbol's name, once the server said it can be renamed: the
    /// prompt shows.
    name: Option<String>,
    /// The rename request went out, when the open files had these
    /// versions.
    versions: Option<Vec<(PathBuf, u64)>>,
}

/// A place an answer named, with its text.
struct Place {
    /// Relative to the project root when it is inside it.
    path: PathBuf,
    start: TextPosition,
    end: TextPosition,
    /// The text of the line it starts on, without its line break.
    line: String,
}

impl Project {
    /// A navigation or rename command.
    pub(in crate::project) fn navigation_command(&mut self, command: Command) {
        match command {
            Command::GoToDefinition => self.locate(Locate::Definition),
            Command::GoToTypeDefinition => self.locate(Locate::TypeDefinition),
            Command::GoToImplementation => self.locate(Locate::Implementation),
            Command::FindUsages => self.locate(Locate::Usages),
            Command::StartRename => self.start_rename(),
            Command::Rename(name) => self.rename(name),
            Command::CancelRename => self.language.navigation.renaming = None,
            _ => {}
        }
    }

    pub(in crate::project) fn usages_view(&self) -> Option<UsagesView> {
        self.language.navigation.usages.clone()
    }

    pub(in crate::project) fn rename_prompt(&self) -> Option<RenamePrompt> {
        let renaming = self.language.navigation.renaming.as_ref().filter(|r| r.versions.is_none())?;
        renaming.name.clone().map(|name| RenamePrompt { name })
    }

    pub(in crate::project) fn hint(&self) -> Option<String> {
        self.language.navigation.hint.clone()
    }

    /// The hint goes with the next command.
    pub(in crate::project) fn clear_hint(&mut self) {
        self.language.navigation.hint = None;
    }

    /// Where a request goes: the ready server and the focused file's caret
    /// as the server counts it (`textDocument` and `position` params). Sets
    /// a hint and returns `None` if there's nothing to ask.
    fn ask_at_caret(&mut self, what: &str) -> Option<(serde_json::Value, Position)> {
        let editor = self.editor.as_ref()?;
        let hint = if text::language_id(editor.path()).is_none() {
            Some(format!("Only TypeScript and JavaScript files have {what}."))
        } else if editor.is_large() {
            Some(format!("This file is over 5 MB, so it has no {what}."))
        } else if !self.language.typescript.as_ref().is_some_and(LanguageServer::is_ready) {
            Some(format!("TypeScript isn't running, so there's no {what}."))
        } else {
            None
        };
        if hint.is_some() {
            self.language.navigation.hint = hint;
            return None;
        }
        let encoding = self.encoding();
        let position = Position::of(editor.text(), editor.caret_position(), encoding);
        let uri = text::uri(&self.root.join(editor.path()));
        self.language.navigation.word = editor.word_at_caret().unwrap_or_default();
        self.language.navigation.generation += 1;
        Some((navigation::at(&uri, position), position))
    }

    fn encoding(&self) -> Encoding {
        self.language.typescript.as_ref().map(LanguageServer::encoding).unwrap_or_default()
    }

    /// Asks the server for the places of the symbol at the caret.
    fn locate(&mut self, locate: Locate) {
        // The server must have the text the caret is in.
        self.sync_language();
        let what = match locate {
            Locate::Usages => "Find Usages",
            _ => "code navigation",
        };
        let Some((mut params, _)) = self.ask_at_caret(what) else { return };
        if locate == Locate::Usages {
            params["context"] = serde_json::json!({ "includeDeclaration": false });
        }
        let navigation = &mut self.language.navigation;
        let generation = navigation.generation;
        if locate == Locate::Usages {
            navigation.usages =
                Some(UsagesView { title: title(locate, &navigation.word), files: Vec::new(), count: 0, finding: true });
            self.left_column = Some(LeftColumnView::Usages);
        }
        if let Some(server) = &mut self.language.typescript {
            server.request(locate.method(), params, Pending::Navigation(Ask::Locate { locate, generation }));
        }
    }

    /// The server answered a navigation request.
    pub(super) fn navigation_answered(&mut self, answer: Answer) {
        match answer {
            Answer::Located { locate, generation, found } => {
                if generation != self.language.navigation.generation {
                    return;
                }
                match found {
                    Ok(locations) => self.find_places(locate, generation, locations),
                    Err(message) => self.places_found(locate, Err(message)),
                }
            }
            Answer::RenameRange { generation, range } => self.rename_range(generation, range),
            Answer::Renamed { generation, edits } => {
                let navigation = &mut self.language.navigation;
                let Some(renaming) = navigation.renaming.take_if(|r| r.generation == generation && r.versions.is_some())
                else {
                    return;
                };
                match edits {
                    Ok(files) => self.apply_rename(renaming.versions.unwrap_or_default(), files),
                    Err(message) => navigation.hint = Some(format!("TypeScript couldn't rename it: {message}")),
                }
            }
        }
    }

    /// ⇧F6: asks the server whether the symbol at the caret can be renamed.
    fn start_rename(&mut self) {
        self.sync_language();
        let Some((params, position)) = self.ask_at_caret("Rename") else { return };
        let Some(editor) = &self.editor else { return };
        let generation = self.language.navigation.generation;
        self.language.navigation.renaming = Some(Renaming {
            generation,
            path: editor.path().to_owned(),
            version: editor.version(),
            position,
            name: None,
            versions: None,
        });
        if let Some(server) = &mut self.language.typescript {
            let ask = Ask::PrepareRename { generation };
            server.request("textDocument/prepareRename", params, Pending::Navigation(ask));
        }
    }

    /// The server said what can be renamed at the caret: the prompt shows,
    /// if the file is still as it was.
    fn rename_range(&mut self, generation: u64, range: Result<Option<(Position, Position, Option<String>)>, Option<String>>) {
        let encoding = self.encoding();
        let navigation = &mut self.language.navigation;
        let Some(renaming) = navigation.renaming.as_mut().filter(|r| r.generation == generation && r.name.is_none())
        else {
            return;
        };
        let Some(editor) = self.editor.as_ref().filter(|e| e.path() == renaming.path && e.version() == renaming.version)
        else {
            navigation.renaming = None;
            return;
        };
        let name = match range {
            Ok(Some((_, _, Some(placeholder)))) => Ok(placeholder),
            Ok(Some((start, end, None))) => {
                let text = editor.text();
                let char_at = |p: Position| {
                    let at = p.in_text(text, encoding);
                    text.line_to_char(at.line) + at.column
                };
                Ok(text.slice(char_at(start)..char_at(end).max(char_at(start))).to_string())
            }
            // A server without prepareRename: the word at the caret.
            Err(None) => editor.word_at_caret().ok_or_else(|| "Nothing to rename here.".to_owned()),
            Ok(None) => Err("Nothing to rename here.".to_owned()),
            Err(Some(message)) => Err(format!("TypeScript can't rename this: {message}")),
        };
        match name {
            Ok(name) if !name.is_empty() => renaming.name = Some(name),
            Ok(_) => {
                navigation.renaming = None;
                navigation.hint = Some("Nothing to rename here.".into());
            }
            Err(hint) => {
                navigation.renaming = None;
                navigation.hint = Some(hint);
            }
        }
    }

    /// The prompt's answer: asks the server for the rename's edits. An
    /// empty or unchanged name closes the prompt.
    fn rename(&mut self, name: String) {
        self.sync_language();
        let navigation = &mut self.language.navigation;
        let Some(mut renaming) = navigation.renaming.take().filter(|r| r.versions.is_none()) else { return };
        let name = name.trim().to_owned();
        if name.is_empty() || renaming.name.as_ref() == Some(&name) {
            return;
        }
        if !self.editor.as_ref().is_some_and(|e| e.path() == renaming.path && e.version() == renaming.version) {
            navigation.hint = Some("The file changed, so nothing was renamed. Try again.".into());
            return;
        }
        let Some(server) = self.language.typescript.as_mut().filter(|s| s.is_ready()) else {
            navigation.hint = Some("TypeScript isn't running, so there's no Rename.".into());
            return;
        };
        navigation.generation += 1;
        renaming.generation = navigation.generation;
        let uri = text::uri(&self.root.join(&renaming.path));
        let mut params = navigation::at(&uri, renaming.position);
        params["newName"] = name.into();
        let ask = Ask::Rename { generation: renaming.generation };
        server.request("textDocument/rename", params, Pending::Navigation(ask));
        let versions = self.open_editors().map(|e| (e.path().to_owned(), e.version())).collect();
        renaming.versions = Some(versions);
        self.language.navigation.renaming = Some(renaming);
    }

    /// Applies a rename's edits: to open files at once, if none changed
    /// since the request went out (`versions`); to the others once they are
    /// read.
    fn apply_rename(&mut self, versions: Vec<(PathBuf, u64)>, files: Vec<(PathBuf, Vec<TextEdit>)>) {
        let changed = versions.iter().any(|(path, version)| self.open_editor(path).is_some_and(|e| e.version() != *version));
        if changed {
            self.language.navigation.hint = Some("Files changed while renaming, so nothing was renamed. Try again.".into());
            return;
        }
        let Some((host, jobs)) = self.language.context.clone() else { return };
        let now = host.clock().now();
        let mut unopened = Vec::new();
        for (path, edits) in files {
            let path = path.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or(path);
            if self.open_editor(&path).is_some() {
                self.edit_file(&path, &edits, now, &jobs);
            } else {
                unopened.push((path, edits));
            }
        }
        if unopened.is_empty() {
            return;
        }
        let (id, root) = (self.id, self.root.clone());
        jobs.spawn("read files to rename in", move || {
            let read: Vec<_> = unopened
                .into_iter()
                .map(|(path, edits)| {
                    let text = fs::read(root.join(&path))
                        .map_err(|e| e.to_string())
                        .and_then(|bytes| String::from_utf8(bytes).map_err(|_| "it isn't valid UTF-8".to_owned()))
                        .map(Rope::from);
                    (path, edits, text)
                })
                .collect();
            Box::new(move |core| {
                let now = core.host.clock().now();
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                for (path, edits, text) in read {
                    project.rename_in_read_file(path, &edits, text, now, &jobs);
                }
            })
        });
    }

    /// A file the rename edits was read: it opens behind the focused tab
    /// with the edits. If it was opened meanwhile, its buffer takes them if
    /// it still has the text the server saw.
    fn rename_in_read_file(&mut self, path: PathBuf, edits: &[TextEdit], text: Result<Rope, String>, now: Instant, jobs: &Jobs) {
        let text = match text {
            Ok(text) => text,
            Err(error) => return self.notify(format!("Couldn't rename in {}: {error}", path.display())),
        };
        match self.open_editor(&path) {
            Some(editor) if *editor.text() == text => self.edit_file(&path, edits, now, jobs),
            Some(_) => self.notify(format!("{} changed while renaming, so it wasn't renamed in.", path.display())),
            None => {
                self.open_tab_behind(Editor::new(path.clone(), text));
                self.edit_file(&path, edits, now, jobs);
                self.git.load_base(&path, jobs);
            }
        }
    }

    /// Applies a server's edits to an open file, as one undo step.
    fn edit_file(&mut self, path: &Path, edits: &[TextEdit], now: Instant, jobs: &Jobs) {
        let (encoding, rows) = (self.encoding(), self.viewport_rows);
        let Some(editor) = self.open_editor_mut(path) else { return };
        let text = editor.text();
        let char_at = |p: Position| {
            let at = p.in_text(text, encoding);
            text.line_to_char(at.line) + at.column
        };
        let edits = edits.iter().map(|edit| (char_at(edit.start)..char_at(edit.end), edit.text.clone())).collect();
        if editor.apply_edits(edits, now, rows) {
            self.reparse_file(path, jobs);
            self.diff_file(path, jobs);
        }
    }

    /// Reads the text at `locations` in the background (from open editors,
    /// else from disk), then shows them.
    fn find_places(&mut self, locate: Locate, generation: u64, locations: Vec<Location>) {
        let Some((_, jobs)) = &self.language.context else { return };
        let jobs: Jobs = jobs.clone();
        let root = self.root.clone();
        let encoding = self.encoding();
        let open: Vec<(PathBuf, Rope)> = self
            .open_editors()
            .filter(|e| locations.iter().any(|l| l.path == root.join(e.path())))
            .map(|e| (e.path().to_owned(), e.text().clone()))
            .collect();
        let id = self.id;
        jobs.spawn("find places", move || {
            let places = places(&root, &locations, &open, encoding);
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                if project.language.navigation.generation == generation {
                    project.places_found_then(locate, places, &jobs);
                }
            })
        });
    }

    fn places_found_then(&mut self, locate: Locate, places: Vec<Place>, jobs: &Jobs) {
        if locate != Locate::Usages && places.len() == 1 {
            let place = places.into_iter().next().expect("one place");
            self.open_file(place.path, Some(place.start), jobs);
            return;
        }
        self.places_found(locate, Ok(places));
    }

    /// Shows what a request found: in the Usages view for Find Usages and
    /// for several places; a hint for none.
    fn places_found(&mut self, locate: Locate, places: Result<Vec<Place>, String>) {
        let navigation = &mut self.language.navigation;
        let word = navigation.word.clone();
        let places = match places {
            Ok(places) => places,
            Err(message) => {
                navigation.hint = Some(format!("TypeScript couldn't answer: {message}"));
                if locate == Locate::Usages {
                    navigation.usages = None;
                }
                return;
            }
        };
        if places.is_empty() && locate != Locate::Usages {
            navigation.hint = Some(match locate {
                Locate::Definition => format!("No definition found for {}.", quoted(&word)),
                Locate::TypeDefinition => format!("No type definition found for {}.", quoted(&word)),
                _ => format!("No implementations found for {}.", quoted(&word)),
            });
            return;
        }
        let count = places.len();
        navigation.usages = Some(UsagesView { title: title(locate, &word), files: group(places), count, finding: false });
        self.left_column = Some(LeftColumnView::Usages);
    }
}

/// The Usages view's title for what was asked.
fn title(locate: Locate, word: &str) -> String {
    let what = match locate {
        Locate::Definition => "Definitions",
        Locate::TypeDefinition => "Type definitions",
        Locate::Implementation => "Implementations",
        Locate::Usages => "Usages",
    };
    if word.is_empty() { what.to_owned() } else { format!("{what} of {word}") }
}

fn quoted(word: &str) -> String {
    if word.is_empty() { "this".to_owned() } else { format!("`{word}`") }
}

/// Places as the Usages view lists them: by file in tree order, top to
/// bottom, without repeats.
fn group(mut places: Vec<Place>) -> Vec<SearchFile> {
    places.sort_by(|a, b| {
        search::tree_order(&a.path, &b.path).then((a.start.line, a.start.column).cmp(&(b.start.line, b.start.column)))
    });
    places.dedup_by(|a, b| a.path == b.path && a.start == b.start);
    let mut files: Vec<SearchFile> = Vec::new();
    for place in places {
        let end = if place.end.line == place.start.line { place.end.column } else { usize::MAX };
        let byte = |column: usize| place.line.char_indices().nth(column).map_or(place.line.len(), |(i, _)| i);
        let (start, end) = (byte(place.start.column), byte(end.max(place.start.column)));
        let found = search::search_match(place.line.as_bytes(), place.start.line, start, end);
        match files.last_mut().filter(|f| f.path == place.path) {
            Some(file) => file.matches.push(found),
            None => files.push(SearchFile { path: place.path, matches: vec![found] }),
        }
    }
    files
}

/// The places at `locations`, with their text: from `open` (the open
/// editors' text, by path relative to the root) or else from disk. Runs in
/// the background. Places in files that can't be read are left out.
fn places(root: &Path, locations: &[Location], open: &[(PathBuf, Rope)], encoding: Encoding) -> Vec<Place> {
    // The server may name files by another spelling of the root.
    let canonical = root.canonicalize().unwrap_or_else(|_| root.to_owned());
    let shown = |path: &Path| {
        path.strip_prefix(root)
            .or_else(|_| path.strip_prefix(&canonical))
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_owned())
    };
    let mut read: Vec<(PathBuf, Option<Rope>)> = Vec::new();
    let mut places = Vec::new();
    for location in locations {
        let path = shown(&location.path);
        let text = match open.iter().find(|(p, _)| *p == path) {
            Some((_, text)) => Some(text.clone()),
            None => match read.iter().find(|(p, _)| *p == path) {
                Some((_, text)) => text.clone(),
                None => {
                    let text = fs::read(root.join(&path)).ok().and_then(|b| String::from_utf8(b).ok()).map(Rope::from);
                    read.push((path.clone(), text.clone()));
                    text
                }
            },
        };
        let Some(text) = text else { continue };
        let start = location.start.in_text(&text, encoding);
        let end = location.end.in_text(&text, encoding);
        let line = text.line(start.line).to_string();
        let line = line.trim_end_matches(['\n', '\r']).to_owned();
        places.push(Place { path, start, end, line });
    }
    places
}
