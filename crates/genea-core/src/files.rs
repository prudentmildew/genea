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
    watcher::FileChanges,
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
    /// The config's `exclude` patterns, as written and as a matcher.
    exclude_patterns: Vec<String>,
    exclude: Gitignore,
    /// The tree's rows, rebuilt when anything above changes.
    rows: Arc<[FileRow]>,
    /// A read of the disk is running.
    reading: bool,
    /// Changed paths (relative) waiting for the next read.
    changed: BTreeSet<PathBuf>,
    /// New folders (relative) waiting to be read with their contents.
    walks: BTreeSet<PathBuf>,
    /// Everything may have changed: read the whole project again.
    rescan: bool,
    /// Bumped whenever the files or `exclude` change.
    version: u64,
    /// The finder's list of files, and the version it was made at.
    list: Option<(u64, Arc<[String]>)>,
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
        FileIndex {
            id,
            root,
            folders: HashMap::new(),
            expanded: HashSet::new(),
            exclude_patterns: Vec::new(),
            exclude: Gitignore::empty(),
            rows: Arc::from([]),
            reading: false,
            changed: BTreeSet::new(),
            walks: BTreeSet::new(),
            rescan: false,
            version: 0,
            list: None,
        }
    }

    /// Reads the whole project in the background.
    pub(crate) fn start(&mut self, jobs: &Jobs) {
        self.rescan = true;
        self.read_next(jobs);
    }

    /// Files changed on disk (from the watcher): reads what changed, after
    /// any read that is running.
    pub(crate) fn files_changed(&mut self, changes: &FileChanges, jobs: &Jobs) {
        self.rescan |= changes.rescan;
        let relative = changes.paths.iter().filter_map(|path| path.strip_prefix(&self.root).ok());
        let shown = relative.filter(|path| !path.components().any(|c| is_hidden_name(c.as_os_str())));
        self.changed.extend(shown.map(Path::to_path_buf));
        self.read_next(jobs);
    }

    pub(crate) fn rows(&self) -> Arc<[FileRow]> {
        self.rows.clone()
    }

    /// A number that changes whenever [`file_list`](Self::file_list) may
    /// have.
    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    /// Every file the views show, as relative paths, for the finder: what
    /// the tree would list with every folder expanded. Made when first asked
    /// for after a change.
    pub(crate) fn file_list(&mut self) -> Arc<[String]> {
        if let Some((version, list)) = &self.list
            && *version == self.version
        {
            return list.clone();
        }
        let mut list = Vec::new();
        self.push_files(Path::new(""), &mut list);
        let list: Arc<[String]> = list.into();
        self.list = Some((self.version, list.clone()));
        list
    }

    /// Whether the index has this file (a relative path), `exclude` or not.
    pub(crate) fn contains(&self, path: &Path) -> bool {
        let (Some(folder), Some(name)) = (path.parent(), path.file_name()) else { return false };
        self.folders.get(folder).is_some_and(|entries| entries.get(name) == Some(&false))
    }

    /// Whether the finder lists this file (a relative path): the index has
    /// it, and neither it nor a folder above it is excluded.
    pub(crate) fn lists(&self, path: &Path) -> bool {
        self.contains(path) && !self.exclude.matched_path_or_any_parents(path, false).is_ignore()
    }

    fn push_files(&self, folder: &Path, list: &mut Vec<String>) {
        let Some(entries) = self.folders.get(folder) else { return };
        for (name, &is_dir) in entries {
            let path = folder.join(name);
            if self.exclude.matched(&path, is_dir).is_ignore() {
                continue;
            }
            if is_dir {
                self.push_files(&path, list);
            } else {
                list.push(path.to_string_lossy().into_owned());
            }
        }
    }

    /// Applies the config's `exclude` patterns (`.gitignore` lines) to the
    /// tree.
    pub(crate) fn set_exclude(&mut self, patterns: &[String]) {
        if patterns == self.exclude_patterns {
            return;
        }
        let mut builder = GitignoreBuilder::new(&self.root);
        for pattern in patterns {
            // The config only keeps patterns that build.
            let _ = builder.add_line(None, pattern);
        }
        self.exclude = builder.build().unwrap_or_else(|_| Gitignore::empty());
        self.exclude_patterns = patterns.to_vec();
        self.version += 1;
        self.rebuild_rows();
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
            self.walks.clear();
            read.walks.insert(PathBuf::new());
        }
        // A changed path is read by listing its folder again. A path in a
        // folder the index doesn't know yet is new with its folder, which a
        // listing further up finds.
        for path in std::mem::take(&mut self.changed) {
            let mut folder = path.parent().unwrap_or(Path::new(""));
            while !self.folders.contains_key(folder)
                && let Some(parent) = folder.parent()
            {
                folder = parent;
            }
            read.lists.insert(folder.to_owned());
        }
        read.walks.append(&mut self.walks);
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
                files.version += 1;
                files.rebuild_rows();
                files.read_next(&jobs);
            })
        });
    }

    /// Takes a read's result into the index.
    fn apply(&mut self, found: Found) {
        let Some(folders) = found.folders else {
            // The folder is gone: its parent's listing drops it.
            if found.folder != Path::new("") {
                self.changed.insert(found.folder);
            }
            return;
        };
        if found.recursive {
            // A rescan keeps the folders that are still there expanded.
            self.folders.retain(|path, _| !path.starts_with(&found.folder));
            self.folders.extend(folders);
            let folders = &self.folders;
            self.expanded.retain(|path| folders.contains_key(path));
            return;
        }
        for (folder, entries) in folders {
            let old = self.folders.insert(folder.clone(), entries);
            let entries = &self.folders[&folder];
            // Folders that went away (or became files) leave with their
            // contents; new ones are read with theirs.
            let gone: Vec<PathBuf> = old
                .iter()
                .flatten()
                .filter(|(name, is_dir)| **is_dir && entries.get(*name) != Some(&true))
                .map(|(name, _)| folder.join(name))
                .collect();
            let new: Vec<PathBuf> = entries
                .iter()
                .filter(|(_, is_dir)| **is_dir)
                .map(|(name, _)| folder.join(name))
                .filter(|path| !self.folders.contains_key(path))
                .collect();
            for path in gone {
                self.forget(&path);
            }
            self.walks.extend(new);
        }
    }

    /// Drops a folder and everything below it from the index.
    fn forget(&mut self, folder: &Path) {
        self.folders.retain(|path, _| !path.starts_with(folder));
        self.expanded.retain(|path| !path.starts_with(folder));
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
