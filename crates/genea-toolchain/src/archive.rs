//! Unpacking a verified archive, minus its top-level folder.

use std::{
    fs::{self, File},
    io::{self, BufReader, Read},
    os::unix::fs::PermissionsExt,
    path::{Component, Path, PathBuf},
};

use crate::sources::Format;

/// Unpacks `archive` into `dest`, dropping the first path component of
/// every entry (`node-v24.18.0-darwin-arm64/bin/node` → `bin/node`).
/// Entries that would land outside `dest` are refused.
pub(crate) fn unpack(format: Format, archive: &Path, dest: &Path) -> io::Result<()> {
    fs::create_dir_all(dest)?;
    let file = BufReader::new(File::open(archive)?);
    match format {
        Format::TarXz => unpack_tar(lzma_rust2::XzReader::new(file, true), dest),
        Format::TarGz => unpack_tar(flate2::read::GzDecoder::new(file), dest),
        Format::Zip => unpack_zip(File::open(archive)?, dest),
    }
}

fn unpack_tar(reader: impl Read, dest: &Path) -> io::Result<()> {
    let mut archive = tar::Archive::new(reader);
    archive.set_preserve_permissions(true);
    for entry in archive.entries()? {
        let mut entry = entry?;
        let Some(relative) = stripped(&entry.path()?)? else { continue };
        let target = dest.join(&relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        match entry.header().entry_type() {
            // A hard link names another entry of the archive; resolve it
            // inside dest, not against the process's working directory.
            tar::EntryType::Link => {
                let Some(link) = entry.link_name()? else { continue };
                let Some(source) = stripped(&link)? else { continue };
                fs::copy(dest.join(source), &target)?;
            }
            _ => {
                entry.unpack(&target)?;
            }
        }
    }
    Ok(())
}

fn unpack_zip(file: File, dest: &Path) -> io::Result<()> {
    let mut archive = zip::ZipArchive::new(BufReader::new(file)).map_err(io::Error::other)?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(io::Error::other)?;
        let Some(name) = entry.enclosed_name() else {
            return Err(io::Error::other(format!("the archive has an unsafe path: {:?}", entry.name())));
        };
        let Some(relative) = stripped(&name)? else { continue };
        let target = dest.join(relative);
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent)?;
        }
        io::copy(&mut entry, &mut File::create(&target)?)?;
        if let Some(mode) = entry.unix_mode() {
            fs::set_permissions(&target, fs::Permissions::from_mode(mode & 0o777))?;
        }
    }
    Ok(())
}

/// `path` without its first component, or `None` for the top-level folder
/// itself. Refuses absolute paths and `..`.
fn stripped(path: &Path) -> io::Result<Option<PathBuf>> {
    let mut components = path.components();
    let mut rest = PathBuf::new();
    match components.next() {
        Some(Component::Normal(_)) => {}
        Some(Component::CurDir) => return stripped(components.as_path()),
        None => return Ok(None),
        Some(_) => return Err(unsafe_path(path)),
    }
    for component in components {
        match component {
            Component::Normal(part) => rest.push(part),
            Component::CurDir => {}
            _ => return Err(unsafe_path(path)),
        }
    }
    Ok((!rest.as_os_str().is_empty()).then_some(rest))
}

fn unsafe_path(path: &Path) -> io::Error {
    io::Error::other(format!("the archive has an unsafe path: {}", path.display()))
}
