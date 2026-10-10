//! What review covers (spec #19, Review: "Scope"): the files under the
//! project root that `.gitignore` doesn't ignore, outside `.git` and
//! `node_modules`, with or without a git repo. The config's `exclude`
//! doesn't affect it.
//!
//! Only `.gitignore` files inside the project count (not the global
//! excludes or `.git/info/exclude`), each for the folder it is in, the
//! deepest one that matches deciding, as in git. A path in an ignored
//! folder is ignored whatever deeper files say.

use std::{
    collections::HashMap,
    ffi::OsStr,
    fs,
    path::{Path, PathBuf},
    sync::Arc,
};

use ignore::{Match, gitignore::Gitignore};

const GITIGNORE: &str = ".gitignore";

#[derive(Clone)]
pub(crate) struct Scope {
    root: PathBuf,
    /// Each `.gitignore` found, by its folder (relative; the root is ``).
    ignores: HashMap<PathBuf, Arc<Gitignore>>,
}

impl Scope {
    /// A scope that knows no `.gitignore` files yet.
    pub(crate) fn new(root: PathBuf) -> Scope {
        Scope { root, ignores: HashMap::new() }
    }

    /// Whether review leaves out `path` (relative), a folder if `is_dir`.
    pub(crate) fn excludes(&self, path: &Path, is_dir: bool) -> bool {
        let mut prefix = PathBuf::new();
        let mut components = path.components().peekable();
        while let Some(component) = components.next() {
            prefix.push(component);
            let last = components.peek().is_none();
            if self.excludes_entry(&prefix, !last || is_dir) {
                return true;
            }
        }
        false
    }

    /// Whether review leaves out this entry, given that its folder is in.
    fn excludes_entry(&self, path: &Path, is_dir: bool) -> bool {
        if path.file_name().is_some_and(is_skipped_name) {
            return true;
        }
        let absolute = self.root.join(path);
        for folder in path.parent().into_iter().flat_map(Path::ancestors) {
            if let Some(ignore) = self.ignores.get(folder) {
                match ignore.matched(&absolute, is_dir) {
                    Match::Ignore(_) => return true,
                    Match::Whitelist(_) => return false,
                    Match::None => {}
                }
            }
        }
        false
    }

    /// The files in scope in folder `from` (relative) and below it, reading
    /// every `.gitignore` on the way. Returns them and whether any
    /// `.gitignore` was found.
    pub(crate) fn walk(&mut self, from: &Path) -> (Vec<PathBuf>, bool) {
        let (mut files, mut found_ignores) = (Vec::new(), false);
        let mut folders = vec![from.to_owned()];
        while let Some(folder) = folders.pop() {
            let Ok(read) = fs::read_dir(self.root.join(&folder)) else { continue };
            let entries: Vec<_> = read.filter_map(Result::ok).collect();
            if entries.iter().any(|e| e.file_name() == GITIGNORE && e.file_type().is_ok_and(|t| t.is_file())) {
                let (ignore, _) = Gitignore::new(self.root.join(&folder).join(GITIGNORE));
                self.ignores.insert(folder.clone(), Arc::new(ignore));
                found_ignores = true;
            }
            for entry in entries {
                let Ok(kind) = entry.file_type() else { continue };
                let path = folder.join(entry.file_name());
                if kind.is_dir() && !self.excludes_entry(&path, true) {
                    folders.push(path);
                } else if kind.is_file() && !self.excludes_entry(&path, false) {
                    files.push(path);
                }
            }
        }
        (files, found_ignores)
    }
}

/// Whether a changed path could be in review's scope at all: nothing in
/// `.git` or `node_modules` is. Cheap enough for the main thread.
pub(crate) fn may_include(path: &Path) -> bool {
    !path.components().any(|c| is_skipped_name(c.as_os_str()))
}

/// Whether a change to this path changes the scope.
pub(crate) fn is_gitignore(path: &Path) -> bool {
    path.file_name().is_some_and(|name| name == GITIGNORE)
}

fn is_skipped_name(name: &OsStr) -> bool {
    name == ".git" || name == "node_modules"
}
