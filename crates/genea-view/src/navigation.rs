//! Code navigation and rename in a project window (ticket #44): the menu
//! items' commands, the Usages view's rows and the rename prompt, from the
//! core's `ProjectView::usages` and `ProjectView::rename`.

use std::{path::PathBuf, rc::Rc};

use genea_core::{Command, ProjectView, TextPosition, UsagesView};
use slint::{ModelRc, VecModel};

use crate::{NavigationAction, ProjectWindow, SearchRow, fonts};

/// The command a Navigate or Refactor menu item runs.
pub fn command(action: NavigationAction) -> Command {
    match action {
        NavigationAction::Definition => Command::GoToDefinition,
        NavigationAction::TypeDefinition => Command::GoToTypeDefinition,
        NavigationAction::Implementation => Command::GoToImplementation,
        NavigationAction::Usages => Command::FindUsages,
        NavigationAction::Rename => Command::StartRename,
    }
}

/// The Usages view's rows as last pushed, so a click maps to the place the
/// user saw, and whether the rename prompt is open.
pub struct Navigation {
    usages: Option<UsagesView>,
    rows: Rc<VecModel<SearchRow>>,
    /// What each row opens: a file, at a place for a place row.
    targets: Vec<(PathBuf, Option<TextPosition>)>,
    renaming: bool,
}

impl Navigation {
    pub fn new(window: &ProjectWindow) -> Self {
        let rows = Rc::new(VecModel::default());
        window.set_usages_rows(ModelRc::from(rows.clone()));
        Navigation { usages: None, rows, targets: Vec::new(), renaming: false }
    }

    /// Pushes the Usages view and the rename prompt. Returns whether the
    /// prompt just closed, so the editor takes the keyboard back.
    pub fn sync(&mut self, window: &ProjectWindow, view: &ProjectView) -> bool {
        let rename = view.rename.as_ref();
        window.set_rename_open(rename.is_some());
        if let Some(rename) = rename {
            fonts::prepare(&rename.name);
            window.set_rename_name(rename.name.as_str().into());
        }
        let closed = self.renaming && rename.is_none();
        self.renaming = rename.is_some();

        window.set_has_usages(view.usages.is_some());
        if view.usages == self.usages {
            return closed;
        }
        let usages = view.usages.as_ref();
        window.set_usages_title(usages.map(|u| u.title.as_str()).unwrap_or_default().into());
        window.set_usages_status(usages.map(status).unwrap_or_default().into());
        let mut rows = Vec::new();
        let mut targets = Vec::new();
        for file in usages.iter().flat_map(|u| &u.files) {
            let path = file.path.display().to_string();
            fonts::prepare(&path);
            rows.push(SearchRow { file: true, label: path.into(), ..SearchRow::default() });
            targets.push((file.path.clone(), None));
            for place in &file.matches {
                for text in [&place.before, &place.matched, &place.after] {
                    fonts::prepare(text);
                }
                rows.push(SearchRow {
                    file: false,
                    label: place.location.as_str().into(),
                    before: place.before.as_str().into(),
                    matched: place.matched.as_str().into(),
                    after: place.after.as_str().into(),
                });
                targets.push((file.path.clone(), Some(place.position)));
            }
        }
        self.rows.set_vec(rows);
        self.targets = targets;
        self.usages = view.usages.clone();
        closed
    }

    /// What a click on a row of the Usages view opens.
    pub fn open(&self, index: usize) -> Option<Command> {
        let (path, at) = self.targets.get(index)?;
        Some(match at {
            Some(at) => Command::OpenFileAt { path: path.clone(), at: *at },
            None => Command::OpenFile(path.clone()),
        })
    }
}

/// The Usages view's status line: "5 usages in 2 files", "Finding usages…".
fn status(usages: &UsagesView) -> String {
    if usages.finding {
        return "Finding usages…".into();
    }
    let plural = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
    match (usages.count, usages.files.len()) {
        (0, _) => "Nothing found".into(),
        (count, 1) => plural(count, "place"),
        (count, files) => format!("{} in {}", plural(count, "place"), plural(files, "file")),
    }
}
