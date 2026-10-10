//! The review baseline store (spec #19, Review: "Baseline store"): per
//! project, under Genea's application-support folder, a content-addressed
//! blob store plus an index of path → (content hash, size, mtime).
//!
//! ```text
//! <support>/review/<key>/root        the project folder, for people (and the harness)
//! <support>/review/<key>/index.json  the index
//! <support>/review/<key>/blobs/ab/cdef…  one file per content, named by its SHA-256
//! ```
//!
//! `<key>` is derived from the project folder's path. Everything here is
//! blocking and runs on background threads.

use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::UNIX_EPOCH,
};

use ropey::Rope;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};

use crate::{LARGE_FILE_BYTES, text::BINARY_SNIFF_BYTES};

/// A file's content hash (SHA-256).
pub(crate) type Hash = [u8; 32];

/// Above this many bytes of blobs, files over [`LARGE_FILE_BYTES`] get no
/// blob (and so no Revert). Smaller files always get one.
pub(crate) const LARGE_BLOBS_CAP: u64 = 1 << 30;

/// The index's format version.
const INDEX_VERSION: u64 = 1;

/// A file's content as review knows it: in the baseline, or on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FileState {
    pub(crate) hash: Hash,
    pub(crate) size: u64,
    /// Modification time, in nanoseconds since the Unix epoch.
    pub(crate) mtime: u64,
    /// No NUL byte near the start: a text file, which can be diffed.
    pub(crate) text: bool,
    /// Its content is in the blob store, so Revert can write it back.
    pub(crate) stored: bool,
}

impl FileState {
    /// Small enough and text, so it can be shown as a diff.
    pub(crate) fn diffable(&self) -> bool {
        self.text && self.size <= LARGE_FILE_BYTES as u64
    }
}

/// How many bytes of large blobs may still be stored.
pub(crate) struct Budget {
    pub(crate) large_bytes: u64,
}

impl Budget {
    /// Whether a file of this size gets a blob; takes it from the budget.
    fn take(&mut self, size: u64) -> bool {
        if size <= LARGE_FILE_BYTES as u64 {
            return true;
        }
        if size > self.large_bytes {
            return false;
        }
        self.large_bytes -= size;
        true
    }
}

pub(crate) struct Store {
    dir: PathBuf,
}

/// Makes temp file names unique.
static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

impl Store {
    /// The store of the project at `root` (canonical).
    pub(crate) fn new(support_dir: &Path, root: &Path) -> Store {
        let key = hex(&hash_bytes(root.as_os_str().as_encoded_bytes()));
        Store { dir: support_dir.join("review").join(&key[..16]) }
    }

    /// Creates the store's folders and notes which project it is for.
    pub(crate) fn create(&self, root: &Path) -> io::Result<()> {
        fs::create_dir_all(self.dir.join("blobs"))?;
        fs::write(self.dir.join("root"), root.as_os_str().as_encoded_bytes())
    }

    /// The index, or `None` if there is none yet (or it can't be used).
    pub(crate) fn read_index(&self) -> Option<BTreeMap<PathBuf, FileState>> {
        let text = fs::read_to_string(self.dir.join("index.json")).ok()?;
        let index: Value = serde_json::from_str(&text).ok()?;
        if index["version"].as_u64() != Some(INDEX_VERSION) {
            return None;
        }
        let files = index["files"].as_object()?;
        let mut entries = BTreeMap::new();
        for (path, entry) in files {
            let state = FileState {
                hash: unhex(entry["hash"].as_str()?)?,
                size: entry["size"].as_u64()?,
                mtime: entry["mtime"].as_u64()?,
                text: entry["text"].as_bool()?,
                stored: entry["stored"].as_bool()?,
            };
            entries.insert(PathBuf::from(path), state);
        }
        Some(entries)
    }

    /// Replaces the index. Paths that aren't valid UTF-8 are left out.
    pub(crate) fn write_index(&self, root: &Path, entries: &BTreeMap<PathBuf, FileState>) -> io::Result<()> {
        let mut files = Map::new();
        for (path, state) in entries {
            let Some(path) = path.to_str() else { continue };
            files.insert(
                path.to_owned(),
                json!({
                    "hash": hex(&state.hash),
                    "size": state.size,
                    "mtime": state.mtime,
                    "text": state.text,
                    "stored": state.stored,
                }),
            );
        }
        let index = json!({ "version": INDEX_VERSION, "root": root.to_string_lossy(), "files": files });
        let temp = self.temp_path();
        fs::write(&temp, serde_json::to_vec(&index).map_err(io::Error::other)?)?;
        fs::rename(&temp, self.dir.join("index.json"))
    }

    pub(crate) fn read_blob(&self, hash: &Hash) -> io::Result<Vec<u8>> {
        fs::read(self.blob_path(hash))
    }

    pub(crate) fn remove_blob(&self, hash: &Hash) {
        let _ = fs::remove_file(self.blob_path(hash));
    }

    fn blob_path(&self, hash: &Hash) -> PathBuf {
        let name = hex(hash);
        self.dir.join("blobs").join(&name[..2]).join(&name[2..])
    }

    fn temp_path(&self) -> PathBuf {
        let n = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        self.dir.join(format!("tmp-{}-{n}", std::process::id()))
    }

    /// Moves a finished temp file into place as the blob `hash`.
    fn keep_blob(&self, temp: &Path, hash: &Hash) -> io::Result<()> {
        let path = self.blob_path(hash);
        if path.exists() {
            return fs::remove_file(temp);
        }
        fs::create_dir_all(path.parent().expect("a blob is in a folder"))?;
        fs::rename(temp, path)
    }
}

/// Reads a regular file (not following a symlink) and describes it; with a
/// store and room in the budget, copies it into the store as it goes.
/// `Ok(None)` when there is no regular file at `path`.
pub(crate) fn read_file(path: &Path, store: Option<(&Store, &mut Budget)>) -> io::Result<Option<FileState>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => metadata,
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let store = store.and_then(|(store, budget)| budget.take(metadata.len()).then_some(store));
    let mtime = mtime(&metadata);
    let mut file = File::open(path)?;
    let mut copy = match store {
        Some(store) => {
            let temp = store.temp_path();
            Some((File::create(&temp)?, temp))
        }
        None => None,
    };
    let mut hasher = Sha256::new();
    let (mut size, mut text) = (0u64, true);
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let n = match file.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        };
        let chunk = &buffer[..n];
        if size < BINARY_SNIFF_BYTES as u64 {
            let sniff = (BINARY_SNIFF_BYTES as u64 - size).min(n as u64) as usize;
            text &= !chunk[..sniff].contains(&0);
        }
        hasher.update(chunk);
        if let Some((temp, _)) = &mut copy {
            temp.write_all(chunk)?;
        }
        size += n as u64;
    }
    let hash: Hash = hasher.finalize().into();
    let mut stored = false;
    if let (Some(store), Some((temp, temp_path))) = (store, copy) {
        drop(temp);
        stored = store.keep_blob(&temp_path, &hash).is_ok();
    }
    Ok(Some(FileState { hash, size, mtime, text, stored }))
}

/// Whether the regular file at `path` still has `state`'s size and mtime,
/// so it can be taken as unchanged without reading it.
pub(crate) fn unmoved(path: &Path, state: &FileState) -> bool {
    fs::symlink_metadata(path)
        .is_ok_and(|metadata| metadata.is_file() && metadata.len() == state.size && mtime(&metadata) == state.mtime)
}

/// Modification time, in nanoseconds since the Unix epoch (0 if unknown).
fn mtime(metadata: &fs::Metadata) -> u64 {
    metadata.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos() as u64)
}

/// Puts the file at `path` into the store if it still holds the content
/// `hash`. Returns whether the store has it now.
pub(crate) fn store_file(path: &Path, hash: &Hash, store: &Store, budget: &mut Budget) -> bool {
    // If the file changed meanwhile, its new content's blob stays behind
    // unreferenced (it may be another file's baseline, so it isn't removed).
    matches!(read_file(path, Some((store, budget))), Ok(Some(state)) if state.hash == *hash && state.stored)
}

pub(crate) fn hash_bytes(bytes: &[u8]) -> Hash {
    Sha256::digest(bytes).into()
}

/// The hash of a buffer as Genea writes it.
pub(crate) fn hash_rope(text: &Rope) -> Hash {
    let mut hasher = Sha256::new();
    for chunk in text.chunks() {
        hasher.update(chunk.as_bytes());
    }
    hasher.finalize().into()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn unhex(text: &str) -> Option<Hash> {
    let mut hash = [0; 32];
    if text.len() != 64 {
        return None;
    }
    for (i, byte) in hash.iter_mut().enumerate() {
        *byte = u8::from_str_radix(text.get(i * 2..i * 2 + 2)?, 16).ok()?;
    }
    Some(hash)
}
