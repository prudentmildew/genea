use std::{
    fs,
    path::{Path, PathBuf},
};

/// A project folder in a fresh temp dir, deleted when dropped.
///
/// Build one with [`FixtureProject::new`], add files, then [`build`]. Paths
/// are relative to the project root and use `/`.
///
/// [`build`]: FixtureBuilder::build
pub struct FixtureProject {
    // Held for its Drop, which deletes the folder.
    dir: tempfile::TempDir,
    root: PathBuf,
}

/// Describes a fixture project's files before it is written to disk.
#[must_use]
#[derive(Default)]
pub struct FixtureBuilder {
    files: Vec<(PathBuf, Vec<u8>)>,
    dirs: Vec<PathBuf>,
}

impl FixtureProject {
    // A builder entry point, not a constructor of FixtureProject itself.
    #[allow(clippy::new_ret_no_self)]
    pub fn new() -> FixtureBuilder {
        FixtureBuilder::default()
    }

    /// The project root: an absolute, canonical path (on macOS, temp dirs
    /// live behind the `/var` → `/private/var` symlink; this resolves it).
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The absolute path of `relative` inside the project.
    pub fn path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.root.join(relative)
    }

    /// Writes (or overwrites) a file, creating parent folders. Use it to make
    /// changes on disk while the project is open.
    pub fn write(&self, relative: impl AsRef<Path>, contents: impl AsRef<[u8]>) {
        write_file(&self.path(relative), contents.as_ref());
    }

    /// Reads a file as UTF-8.
    pub fn read(&self, relative: impl AsRef<Path>) -> String {
        let path = self.path(relative);
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
    }

    /// Removes a file.
    pub fn remove(&self, relative: impl AsRef<Path>) {
        let path = self.path(relative);
        fs::remove_file(&path).unwrap_or_else(|e| panic!("remove {}: {e}", path.display()));
    }

    /// Keeps the folder after the test, for debugging, and returns its path.
    pub fn keep(self) -> PathBuf {
        let _ = self.dir.keep();
        self.root
    }
}

impl FixtureBuilder {
    /// Adds a file with these contents.
    pub fn file(mut self, relative: impl AsRef<Path>, contents: impl AsRef<[u8]>) -> Self {
        self.files.push((relative.as_ref().to_owned(), contents.as_ref().to_owned()));
        self
    }

    /// Adds an empty folder.
    pub fn dir(mut self, relative: impl AsRef<Path>) -> Self {
        self.dirs.push(relative.as_ref().to_owned());
        self
    }

    /// Writes the project into a new temp dir.
    pub fn build(self) -> FixtureProject {
        let dir = tempfile::Builder::new().prefix("genea-fixture-").tempdir().expect("create a temp dir");
        let root = dir.path().canonicalize().expect("canonicalize the temp dir");
        for relative in &self.dirs {
            fs::create_dir_all(root.join(relative)).expect("create a fixture folder");
        }
        for (relative, contents) in &self.files {
            write_file(&root.join(relative), contents);
        }
        FixtureProject { dir, root }
    }
}

fn write_file(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap_or_else(|e| panic!("create {}: {e}", parent.display()));
    }
    fs::write(path, contents).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
}
