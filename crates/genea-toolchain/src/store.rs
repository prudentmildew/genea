//! The shared toolchain store: `<support dir>/toolchains/<tool>/<version>/`.
//!
//! A version folder exists only once its download has been verified and
//! unpacked: installs unpack into a hidden `.download-…` folder next to the
//! tool folders and rename it into place, and a failed install removes it.
//! [`Installed::bin_dir`] says where a tool's executables are: `bin/` for
//! Node and Bun, the version folder itself for pnpm (its `@pnpm/exe`
//! package).

use std::{
    cell::Cell,
    collections::HashMap,
    fmt,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use genea_host::{DownloadError, Downloads};
use sha2::{Digest, Sha256, Sha512};

use crate::{
    Request, Tool, Version, archive,
    sources::{self, Archive, Checksum},
};

/// The store. Share one per process (it serialises installs of the same
/// version) and call it from background threads: everything here blocks.
pub struct Store {
    root: PathBuf,
    installing: Mutex<HashMap<(Tool, String), Arc<Mutex<()>>>>,
}

/// A version in the store, ready to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Installed {
    pub tool: Tool,
    pub version: Version,
    /// `<store>/<tool>/<version>`.
    pub dir: PathBuf,
    /// The folder to put on PATH, holding the tool's executable (`node`,
    /// `bun`, `pnpm`).
    pub bin_dir: PathBuf,
}

/// How far a download has got.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub received: u64,
    /// The archive's size, when the server says.
    pub total: Option<u64>,
}

impl Progress {
    /// Whole percent done, if the size is known.
    pub fn percent(&self) -> Option<u8> {
        self.total.filter(|t| *t > 0).map(|t| (self.received.min(t) * 100 / t) as u8)
    }
}

impl Store {
    /// A store rooted at `root` (normally `<support dir>/toolchains`). The
    /// folder is created on the first install.
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Store { root: root.into(), installing: Mutex::default() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The versions of `tool` in the store.
    pub fn installed(&self, tool: Tool) -> Vec<Version> {
        let Ok(entries) = fs::read_dir(self.root.join(tool.id())) else { return Vec::new() };
        entries.flatten().filter_map(|e| Version::parse(e.file_name().to_str()?).ok()).collect()
    }

    /// The version `request` resolves to: itself when exact; for a range,
    /// the newest matching version in the store, else the newest matching
    /// published version (which needs the network).
    pub fn resolve(&self, downloads: &dyn Downloads, tool: Tool, request: &Request) -> Result<Version, ToolchainError> {
        if let Request::Exact(version) = request {
            return Ok(version.clone());
        }
        if let Some(version) = request.newest_match(&self.installed(tool)) {
            return Ok(version.clone());
        }
        let published = sources::published(downloads, tool)?;
        request
            .newest_match(&published)
            .cloned()
            .ok_or_else(|| ToolchainError::NoMatch { tool, request: request.to_string() })
    }

    /// Makes sure `tool` `version` is in the store, downloading, verifying
    /// and unpacking it if it isn't. `progress` hears about the download as
    /// it goes; it isn't called when the version is already there.
    pub fn install(
        &self,
        downloads: &dyn Downloads,
        tool: Tool,
        version: &Version,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<Installed, ToolchainError> {
        let lock = self.installing.lock().unwrap().entry((tool, version.to_string())).or_default().clone();
        let _installing = lock.lock().unwrap_or_else(|e| e.into_inner());

        let installed = self.installed_at(tool, version);
        if installed.dir.is_dir() {
            return Ok(installed);
        }
        let staging = self.staging_dir(tool, version);
        let result = self.download_into(downloads, tool, version, &staging, progress);
        let result = result.and_then(|()| {
            fs::create_dir_all(installed.dir.parent().unwrap()).map_err(ToolchainError::Store)?;
            match fs::rename(staging.join("unpacked"), &installed.dir) {
                Ok(()) => Ok(()),
                // Another Genea finished the same install first.
                Err(_) if installed.dir.is_dir() => Ok(()),
                Err(error) => Err(ToolchainError::Store(error)),
            }
        });
        let _ = fs::remove_dir_all(&staging);
        result.map(|()| installed)
    }

    fn installed_at(&self, tool: Tool, version: &Version) -> Installed {
        let dir = self.root.join(tool.id()).join(version.to_string());
        Installed { tool, version: version.clone(), bin_dir: dir.join(sources::bin_subdir(tool)), dir }
    }

    fn staging_dir(&self, tool: Tool, version: &Version) -> PathBuf {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let n = NEXT.fetch_add(1, Ordering::Relaxed);
        self.root.join(format!(".download-{}-{version}-{}-{n}", tool.id(), std::process::id()))
    }

    /// Downloads and verifies each archive into `staging`, unpacks them
    /// into `staging/unpacked` and lays the result out to run.
    fn download_into(
        &self,
        downloads: &dyn Downloads,
        tool: Tool,
        version: &Version,
        staging: &Path,
        progress: &mut dyn FnMut(Progress),
    ) -> Result<(), ToolchainError> {
        let artifact = sources::artifact(downloads, tool, version)?;
        fs::create_dir_all(staging).map_err(ToolchainError::Store)?;
        let unpacked = staging.join("unpacked");
        for (i, archive) in artifact.archives.iter().enumerate() {
            let archive_path = staging.join(format!("archive-{i}"));
            let mut silent = |_: Progress| {};
            let report: &mut dyn FnMut(Progress) = if archive.reports_progress { &mut *progress } else { &mut silent };
            download_verified(downloads, archive, &archive_path, report)?;
            archive::unpack(archive.format, &archive_path, &unpacked.join(archive.into))
                .map_err(|error| ToolchainError::Archive { url: archive.url.clone(), error })?;
        }
        sources::finish(tool, version, &unpacked).map_err(|error| ToolchainError::Archive {
            url: artifact.archives.first().map(|a| a.url.clone()).unwrap_or_default(),
            error,
        })
    }
}

/// Downloads `archive` to `path`, hashing it on the way, and fails unless it
/// matches its published checksum.
fn download_verified(
    downloads: &dyn Downloads,
    archive: &Archive,
    path: &Path,
    progress: &mut dyn FnMut(Progress),
) -> Result<(), ToolchainError> {
    let file = File::create(path).map_err(ToolchainError::Store)?;
    let total = Cell::new(None);
    let mut sink = HashingSink {
        file: BufWriter::new(file),
        sha256: Sha256::new(),
        sha512: Sha512::new(),
        received: 0,
        total: &total,
        report: progress,
    };
    (sink.report)(Progress { received: 0, total: None });
    downloads
        .fetch_with_length(&archive.url, &mut sink, &mut |length| total.set(Some(length)))
        .map_err(|error| ToolchainError::Download { url: archive.url.clone(), error })?;
    sink.file.flush().map_err(ToolchainError::Store)?;

    let matches = match &archive.checksum {
        Checksum::Sha256(expected) => sink.sha256.finalize().as_slice() == expected.as_slice(),
        Checksum::Sha512(expected) => sink.sha512.finalize().as_slice() == expected.as_slice(),
    };
    if matches { Ok(()) } else { Err(ToolchainError::Checksum { url: archive.url.clone() }) }
}

/// Writes the download to disk while hashing it and reporting progress.
struct HashingSink<'p> {
    file: BufWriter<File>,
    sha256: Sha256,
    sha512: Sha512,
    received: u64,
    /// Set by the host once the response's length is known.
    total: &'p Cell<Option<u64>>,
    report: &'p mut dyn FnMut(Progress),
}

impl Write for HashingSink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.file.write(buf)?;
        self.sha256.update(&buf[..n]);
        self.sha512.update(&buf[..n]);
        self.received += n as u64;
        (self.report)(Progress { received: self.received, total: self.total.get() });
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.file.flush()
    }
}

/// Why a tool couldn't be resolved or installed.
#[derive(Debug)]
pub enum ToolchainError {
    /// A request failed (network, HTTP status).
    Download { url: String, error: DownloadError },
    /// The archive doesn't match its published checksum.
    Checksum { url: String },
    /// A version index, checksum list or registry document isn't what the
    /// publisher normally serves.
    BadDocument { url: String, reason: String },
    /// No such version is published.
    NotPublished { tool: Tool, version: Version },
    /// No published version matches a range pin.
    NoMatch { tool: Tool, request: String },
    /// The verified archive couldn't be unpacked.
    Archive { url: String, error: io::Error },
    /// The store folder couldn't be written.
    Store(io::Error),
}

impl fmt::Display for ToolchainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ToolchainError::Download { url, error } => write!(f, "{url}: {error}"),
            ToolchainError::Checksum { url } => write!(f, "{url} doesn't match its published checksum"),
            ToolchainError::BadDocument { url, reason } => write!(f, "{url} can't be used: {reason}"),
            ToolchainError::NotPublished { tool, version } => write!(f, "{tool} {version} isn't published for macOS arm64"),
            ToolchainError::NoMatch { tool, request } => write!(f, "no published {tool} version matches {request}"),
            ToolchainError::Archive { url, error } => write!(f, "{url} couldn't be unpacked: {error}"),
            ToolchainError::Store(error) => write!(f, "the toolchain store couldn't be written: {error}"),
        }
    }
}

impl std::error::Error for ToolchainError {}
