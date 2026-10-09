//! One open project: its folder and what its window shows.

use std::{
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
};

use genea_host::Host;
use ropey::Rope;

use crate::{
    command::{CaretMove, Command},
    editor::Editor,
    jobs::Jobs,
    view::{Notice, ProjectView, StatusBar},
    workbench::ProjectId,
};

/// Rows assumed until the view reports its viewport.
const DEFAULT_VIEWPORT_ROWS: f64 = 50.0;

pub(crate) struct Project {
    id: ProjectId,
    root: PathBuf,
    editor: Option<Editor>,
    viewport_rows: f64,
    notices: Vec<Notice>,
    /// Bumped by every OpenFile, so a slow read can't replace a newer one.
    open_generation: u64,
}

impl Project {
    pub(crate) fn new(id: ProjectId, root: PathBuf) -> Self {
        Project {
            id,
            root,
            editor: None,
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            notices: Vec::new(),
            open_generation: 0,
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn dispatch(&mut self, command: Command, jobs: &Jobs, host: &dyn Host) {
        match command {
            Command::OpenFile(path) => self.open_file(path, jobs),
            Command::SetViewport { rows } => {
                self.viewport_rows = rows.max(1.0);
                if let Some(editor) = &mut self.editor {
                    editor.scroll_by(0.0, self.viewport_rows);
                }
            }
            Command::ScrollBy { rows } => {
                if let Some(editor) = &mut self.editor {
                    editor.scroll_by(rows, self.viewport_rows);
                }
            }
            Command::MoveCaret(movement) => {
                if let Some(editor) = &mut self.editor {
                    editor.move_caret(movement, false, self.viewport_rows);
                }
            }
            Command::Select(movement) => {
                if let Some(editor) = &mut self.editor {
                    editor.move_caret(movement, true, self.viewport_rows);
                }
            }
            Command::SelectAll => {
                if let Some(editor) = &mut self.editor {
                    editor.select_all(self.viewport_rows);
                }
            }
            Command::PlaceCaret { line, column } => {
                if let Some(editor) = &mut self.editor {
                    editor.place_caret(line, column, false, self.viewport_rows);
                }
            }
            Command::ExtendSelection { line, column } => {
                if let Some(editor) = &mut self.editor {
                    editor.place_caret(line, column, true, self.viewport_rows);
                }
            }
            Command::SelectWord { line, column } => {
                if let Some(editor) = &mut self.editor {
                    editor.select_word(line, column, self.viewport_rows);
                }
            }
            Command::SelectLine { line } => {
                if let Some(editor) = &mut self.editor {
                    editor.select_line(line, self.viewport_rows);
                }
            }
            Command::InsertText(text) => {
                if let Some(editor) = &mut self.editor {
                    editor.insert(&text, self.viewport_rows);
                }
            }
            Command::Delete(movement) => {
                if let Some(editor) = &mut self.editor {
                    editor.delete(movement, self.viewport_rows);
                }
            }
            Command::NewLine => {
                if let Some(editor) = &mut self.editor {
                    editor.insert("\n", self.viewport_rows);
                }
            }
            Command::Copy => {
                if let Some(text) = self.editor.as_ref().and_then(Editor::selected_text) {
                    host.clipboard().write_text(&text);
                }
            }
            Command::Cut => {
                if let Some(editor) = &mut self.editor
                    && let Some(text) = editor.selected_text()
                {
                    host.clipboard().write_text(&text);
                    editor.delete(CaretMove::Left, self.viewport_rows);
                }
            }
            Command::Paste => {
                if let Some(editor) = &mut self.editor
                    && let Some(text) = host.clipboard().read_text()
                {
                    editor.insert(&text, self.viewport_rows);
                }
            }
            Command::Save => self.save(jobs),
        }
    }

    pub(crate) fn view(&self) -> ProjectView {
        let editor = self.editor.as_ref().map(|e| e.view(self.viewport_rows));
        let status = StatusBar {
            caret: editor.as_ref().map(|e| format!("{}:{}", e.caret.line + 1, e.caret.column + 1)),
        };
        ProjectView {
            root: self.root.clone(),
            name: self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            editor,
            status,
            notices: self.notices.clone(),
        }
    }

    /// Writes the open file in the background, as it is now. Edits made
    /// while it is written stay unsaved; a failed write adds a notice.
    fn save(&mut self, jobs: &Jobs) {
        let Some(editor) = &self.editor else { return };
        let snapshot = editor.snapshot();
        let absolute = self.root.join(&snapshot.path);
        let id = self.id;
        jobs.spawn("save file", move || {
            let written = File::create(&absolute).and_then(|file| {
                let mut writer = BufWriter::new(file);
                snapshot.text.write_to(&mut writer)?;
                writer.flush()
            });
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                match written {
                    Ok(()) => {
                        if let Some(editor) = &mut project.editor {
                            editor.saved(&snapshot);
                        }
                    }
                    Err(error) => project
                        .notices
                        .push(Notice { message: format!("Couldn't save {}: {error}", snapshot.path.display()) }),
                }
            })
        });
    }

    /// Reads the file in the background. The current editor stays until the
    /// new file is read; a failed read leaves it and adds a notice.
    fn open_file(&mut self, path: PathBuf, jobs: &Jobs) {
        let absolute = self.root.join(&path);
        let shown = absolute.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or_else(|_| absolute.clone());
        self.open_generation += 1;
        let generation = self.open_generation;
        let id = self.id;
        jobs.spawn("open file", move || {
            let read = File::open(&absolute).and_then(|f| Rope::from_reader(BufReader::new(f)));
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.open_generation != generation {
                    return;
                }
                match read {
                    Ok(text) => project.editor = Some(Editor::new(shown, text)),
                    Err(error) => project
                        .notices
                        .push(Notice { message: format!("Couldn't open {}: {error}", shown.display()) }),
                }
            })
        });
    }
}
