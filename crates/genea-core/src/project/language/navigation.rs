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

use std::{
    fs,
    path::{Path, PathBuf},
};

use ropey::Rope;

use super::super::Project;
use crate::{
    command::Command,
    jobs::Jobs,
    lsp::{
        LanguageServer, Pending,
        navigation::{self, Answer, Ask, Locate, Location, Position},
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
            _ => {}
        }
    }

    pub(in crate::project) fn usages_view(&self) -> Option<UsagesView> {
        self.language.navigation.usages.clone()
    }

    pub(in crate::project) fn rename_prompt(&self) -> Option<RenamePrompt> {
        None
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
    fn ask_at_caret(&mut self, what: &str) -> Option<serde_json::Value> {
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
        Some(navigation::at(&uri, position))
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
        let Some(mut params) = self.ask_at_caret(what) else { return };
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
            Answer::RenameRange { .. } | Answer::Renamed { .. } => {}
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
