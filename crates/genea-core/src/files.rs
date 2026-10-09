//! The project's file index and the Files view's tree (ticket #30).
//!
//! The index knows every file and folder in the project except what is
//! inside `node_modules` and `.git`, which no view shows. It is read once
//! in the background when the project opens, and then kept up to date from
//! the watcher's batches. The tree shows the index minus the config's
//! `exclude` patterns, which apply when the rows are built, so a config
//! change shows or hides paths at once without reading the disk.
//!
//! Reads of the disk run one at a time, in order: changes that arrive
//! while one runs wait for it, so an older read can never overwrite a newer
//! one.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ffi::{OsStr, OsString},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use ignore::gitignore::{Gitignore, GitignoreBuilder};

use crate::{
    jobs::Jobs,
    view::{FileRow, FileRowKind},
    workbench::ProjectId,
};

/// A folder's entries: name → is a folder.
type Entries = BTreeMap<OsString, bool>;

pub(crate) struct FileIndex {
    id: ProjectId,
    root: PathBuf,
    /// Every known folder (relative to the root; the root is ``) and its
    /// entries.
    folders: HashMap<PathBuf, Entries>,
    /// Folders the user expanded (relative).
    expanded: HashSet<PathBuf>,
    /// The config's `exclude` patterns.
    exclude: Gitignore,
    /// The tree's rows, rebuilt when anything above changes.
    rows: Arc<[FileRow]>,
    /// A read of the disk is running.
    reading: bool,
    /// Changed paths (relative) waiting for the next read.
    changed: BTreeSet<PathBuf>,
    /// Everything may have changed: read the whole project again.
    rescan: bool,
}

/// What a read of the disk asks for.
#[derive(Default)]
struct Read {
    /// Folders to list, without their subfolders.
    lists: BTreeSet<PathBuf>,
    /// Folders to read with everything below them.
    walks: BTreeSet<PathBuf>,
}

/// What a read found for one requested folder.
struct Found {
    folder: PathBuf,
    /// Its subfolders were read too.
    recursive: bool,
    /// Each folder read and its entries; `None` if the folder is gone.
    folders: Option<Vec<(PathBuf, Entries)>>,
}

impl FileIndex {
    pub(crate) fn new(id: ProjectId, root: PathBuf) -> Self {
        let exclude = GitignoreBuilder::new(&root).build().expect("an empty matcher builds");
        FileIndex {
            id,
            root,
            folders: HashMap::new(),
            expanded: HashSet::new(),
            exclude,
            rows: Arc::from([]),
            reading: false,
            changed: BTreeSet::new(),
            rescan: false,
        }
    }

    /// Reads the whole project in the background.
    pub(crate) fn start(&mut self, jobs: &Jobs) {
        self.rescan = true;
        self.read_next(jobs);
    }

    pub(crate) fn rows(&self) -> Arc<[FileRow]> {
        self.rows.clone()
    }

    /// Expands a folder, or collapses it if it is expanded.
    pub(crate) fn toggle(&mut self, folder: &Path) {
        if !self.folders.contains_key(folder) {
            return;
        }
        if !self.expanded.remove(folder) {
            self.expanded.insert(folder.to_owned());
        }
        self.rebuild_rows();
    }

    /// Starts the next read of the disk, if one is due and none is running.
    fn read_next(&mut self, jobs: &Jobs) {
        if self.reading {
            return;
        }
        let mut read = Read::default();
        if std::mem::take(&mut self.rescan) {
            self.changed.clear();
            read.walks.insert(PathBuf::new());
        }
        if read.lists.is_empty() && read.walks.is_empty() {
            return;
        }
        self.reading = true;
        let root = self.root.clone();
        let id = self.id;
        jobs.spawn("read files", move || {
            let found = read.run(&root);
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                let files = &mut project.files;
                files.reading = false;
                for found in found {
                    files.apply(found);
                }
                files.rebuild_rows();
                files.read_next(&jobs);
            })
        });
    }

    /// Takes a read's result into the index.
    fn apply(&mut self, found: Found) {
        let Some(folders) = found.folders else { return };
        if found.recursive {
            self.folders.retain(|folder, _| !folder.starts_with(&found.folder));
        }
        self.folders.extend(folders);
    }

    /// Rebuilds the tree's rows: the root's entries, and the entries of
    /// every expanded folder below it, minus what `exclude` hides.
    fn rebuild_rows(&mut self) {
        let mut rows = Vec::new();
        self.push_rows(Path::new(""), 0, &mut rows);
        self.rows = rows.into();
    }

    fn push_rows(&self, folder: &Path, depth: usize, rows: &mut Vec<FileRow>) {
        let Some(entries) = self.folders.get(folder) else { return };
        let mut children: Vec<(&OsString, bool)> = entries.iter().map(|(name, is_dir)| (name, *is_dir)).collect();
        children.sort_by_cached_key(|(name, is_dir)| {
            let name = name.to_string_lossy();
            (!is_dir, name.to_lowercase(), name.into_owned())
        });
        for (name, is_dir) in children {
            let path = folder.join(name);
            if self.exclude.matched(&path, is_dir).is_ignore() {
                continue;
            }
            let expanded = is_dir && self.expanded.contains(&path);
            rows.push(FileRow {
                path: path.clone(),
                name: name.to_string_lossy().into_owned(),
                depth,
                kind: if is_dir { FileRowKind::Folder { expanded } } else { FileRowKind::File },
            });
            if expanded {
                self.push_rows(&path, depth + 1, rows);
            }
        }
    }
}

impl Read {
    /// Reads the disk (background thread).
    fn run(self, root: &Path) -> Vec<Found> {
        let lists = self.lists.into_iter().map(|folder| {
            let folders = list(&root.join(&folder)).map(|entries| vec![(folder.clone(), entries)]);
            Found { folder, recursive: false, folders }
        });
        let walks = self.walks.into_iter().map(|folder| {
            let folders = walk(root, &folder);
            Found { folder, recursive: true, folders }
        });
        lists.chain(walks).collect()
    }
}

/// Folders no view shows, at any depth.
pub(crate) fn is_hidden_name(name: &OsStr) -> bool {
    name == "node_modules" || name == ".git"
}

/// A folder's entries, or `None` if it is gone.
fn list(folder: &Path) -> Option<Entries> {
    let read = match fs::read_dir(folder) {
        Ok(read) => read,
        Err(error) if error.kind() == io::ErrorKind::NotFound || error.kind() == io::ErrorKind::NotADirectory => {
            return None;
        }
        // Unreadable: show it empty.
        Err(_) => return Some(Entries::new()),
    };
    let entries = read
        .filter_map(Result::ok)
        .filter(|entry| !is_hidden_name(&entry.file_name()))
        .map(|entry| (entry.file_name(), entry.file_type().is_ok_and(|t| t.is_dir())))
        .collect();
    Some(entries)
}

/// A folder and every folder below it, with their entries, or `None` if
/// it is gone.
fn walk(root: &Path, folder: &Path) -> Option<Vec<(PathBuf, Entries)>> {
    let mut folders = vec![(folder.to_owned(), list(&root.join(folder))?)];
    let mut next = 0;
    while let Some((path, entries)) = folders.get(next) {
        let subfolders: Vec<PathBuf> =
            entries.iter().filter(|(_, is_dir)| **is_dir).map(|(name, _)| path.join(name)).collect();
        for subfolder in subfolders {
            // A folder that went away meanwhile is left out.
            if let Some(entries) = list(&root.join(&subfolder)) {
                folders.push((subfolder, entries));
            }
        }
        next += 1;
    }
    Some(folders)
}
