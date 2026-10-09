//! Where each tool comes from, and how its download is checked (ADR 0005):
//!
//! - **Node**: nodejs.org. Versions from `dist/index.json` (entries with an
//!   `osx-arm64-tar` file); the `darwin-arm64.tar.xz` archive is checked
//!   against `SHASUMS256.txt`.
//! - **Bun**: GitHub releases. Versions from the releases API (tags
//!   `bun-v<V>`, no drafts or prereleases); `bun-darwin-aarch64.zip` is
//!   checked against the release's `SHASUMS256.txt`.
//! - **pnpm**: npm's `@pnpm/exe`. Its darwin-arm64 platform package is named
//!   in `@pnpm/exe@<V>`'s `optionalDependencies` (`@pnpm/exe.darwin-arm64`
//!   from 12, `@pnpm/macos-arm64` before). Both tarballs are checked against
//!   their sha512 `dist.integrity`. pnpm's GitHub releases publish no
//!   checksums. The binary only works from inside the unpacked `@pnpm/exe`
//!   package (pnpm 11 loads `dist/pnpm.mjs` from beside itself; pnpm 12 keeps
//!   node-gyp there), so the version folder is that package with the binary
//!   placed at its `pnpm` (and, from 12, at `pn`, `pnpx` and `pnx`), as
//!   pnpm's own install script does.

use std::{fs, io, os::unix::fs::PermissionsExt, path::Path};

use base64::Engine;
use genea_host::{DownloadError, Downloads};
use serde_json::Value;

use crate::{Tool, ToolchainError, Version};

const NODE_DIST: &str = "https://nodejs.org/dist";
const BUN_RELEASES: &str = "https://api.github.com/repos/oven-sh/bun/releases?per_page=100";
const BUN_DOWNLOADS: &str = "https://github.com/oven-sh/bun/releases/download";
const NPM_REGISTRY: &str = "https://registry.npmjs.org";
/// pnpm's macOS arm64 package, by name, newest naming first.
const PNPM_PLATFORM_PACKAGES: [&str; 2] = ["@pnpm/exe.darwin-arm64", "@pnpm/macos-arm64"];

/// What to download for one tool version: its archives, in order.
pub(crate) struct Artifact {
    pub archives: Vec<Archive>,
}

/// One downloadable archive and what it must hash to.
pub(crate) struct Archive {
    pub url: String,
    pub checksum: Checksum,
    pub format: Format,
    /// Where the archive's contents go inside the version folder, after
    /// their top-level folder is stripped: `""` (Node ships `bin/`), `"bin"`
    /// (Bun ships the bare executable), or a scratch folder [`finish`] uses.
    pub into: &'static str,
    /// Whether this archive's download is the one progress reports on (the
    /// big one).
    pub reports_progress: bool,
}

/// Where a tool's executables are inside its version folder.
pub(crate) fn bin_subdir(tool: Tool) -> &'static str {
    match tool {
        Tool::Node | Tool::Bun => "bin",
        Tool::Pnpm => "",
    }
}

/// pnpm's platform package is unpacked here, then its binary is moved out.
const PNPM_PLATFORM_DIR: &str = ".platform";

pub(crate) enum Checksum {
    Sha256(Vec<u8>),
    Sha512(Vec<u8>),
}

#[derive(Clone, Copy)]
pub(crate) enum Format {
    TarXz,
    TarGz,
    Zip,
}

/// Every version of `tool` published for macOS arm64.
pub(crate) fn published(downloads: &dyn Downloads, tool: Tool) -> Result<Vec<Version>, ToolchainError> {
    let parse = |s: &str| Version::parse(s).ok();
    let versions = match tool {
        Tool::Node => {
            let url = format!("{NODE_DIST}/index.json");
            let index = fetch_json(downloads, &url)?;
            let entries = index.as_array().ok_or_else(|| bad_document(&url, "it isn't a list"))?;
            entries
                .iter()
                .filter(|e| e["files"].as_array().is_some_and(|f| f.iter().any(|f| f == "osx-arm64-tar")))
                .filter_map(|e| e["version"].as_str().and_then(|v| parse(v.trim_start_matches('v'))))
                .collect()
        }
        Tool::Bun => {
            let releases = fetch_json(downloads, BUN_RELEASES)?;
            let releases = releases.as_array().ok_or_else(|| bad_document(BUN_RELEASES, "it isn't a list"))?;
            releases
                .iter()
                .filter(|r| r["draft"] != true && r["prerelease"] != true)
                .filter_map(|r| r["tag_name"].as_str()?.strip_prefix("bun-v").and_then(parse))
                .collect()
        }
        Tool::Pnpm => {
            let url = format!("{NPM_REGISTRY}/@pnpm%2fexe");
            let packument = fetch_json(downloads, &url)?;
            let versions = packument["versions"].as_object().ok_or_else(|| bad_document(&url, "it has no versions"))?;
            versions.keys().filter_map(|v| parse(v)).collect()
        }
    };
    Ok(versions)
}

/// The archive for `tool` at `version`, with its published checksum.
pub(crate) fn artifact(downloads: &dyn Downloads, tool: Tool, version: &Version) -> Result<Artifact, ToolchainError> {
    match tool {
        Tool::Node => {
            let base = format!("{NODE_DIST}/v{version}");
            let file = format!("node-v{version}-darwin-arm64.tar.xz");
            let checksum = sha256_from_shasums(downloads, tool, version, &format!("{base}/SHASUMS256.txt"), &file)?;
            let url = format!("{base}/{file}");
            let archive = Archive { url, checksum, format: Format::TarXz, into: "", reports_progress: true };
            Ok(Artifact { archives: vec![archive] })
        }
        Tool::Bun => {
            let base = format!("{BUN_DOWNLOADS}/bun-v{version}");
            let file = "bun-darwin-aarch64.zip";
            let checksum = sha256_from_shasums(downloads, tool, version, &format!("{base}/SHASUMS256.txt"), file)?;
            let url = format!("{base}/{file}");
            let archive = Archive { url, checksum, format: Format::Zip, into: "bin", reports_progress: true };
            Ok(Artifact { archives: vec![archive] })
        }
        Tool::Pnpm => {
            let manifest_url = format!("{NPM_REGISTRY}/@pnpm%2fexe/{version}");
            let manifest = fetch_json(downloads, &manifest_url).map_err(|e| not_published(e, tool, version))?;
            let platform = PNPM_PLATFORM_PACKAGES
                .into_iter()
                .find(|p| manifest["optionalDependencies"].get(p).is_some())
                .ok_or_else(|| bad_document(&manifest_url, "it has no macOS arm64 package"))?;
            let package_url = format!("{NPM_REGISTRY}/{}/{version}", platform.replace('/', "%2f"));
            let package = fetch_json(downloads, &package_url)?;
            let (binary_url, binary_checksum) = npm_dist(&package, &package_url)?;
            let (exe_url, exe_checksum) = npm_dist(&manifest, &manifest_url)?;
            Ok(Artifact {
                archives: vec![
                    Archive {
                        url: binary_url,
                        checksum: binary_checksum,
                        format: Format::TarGz,
                        into: PNPM_PLATFORM_DIR,
                        reports_progress: true,
                    },
                    Archive {
                        url: exe_url,
                        checksum: exe_checksum,
                        format: Format::TarGz,
                        into: "",
                        reports_progress: false,
                    },
                ],
            })
        }
    }
}

/// Lays out the unpacked archives so the tool runs from the store: puts
/// pnpm's binary into its package, and makes the executables executable
/// (pnpm before 12 ships its binary without the bit).
pub(crate) fn finish(tool: Tool, version: &Version, unpacked: &Path) -> io::Result<()> {
    let bin = unpacked.join(bin_subdir(tool));
    if tool == Tool::Pnpm {
        let platform = unpacked.join(PNPM_PLATFORM_DIR);
        let binary = platform.join("pnpm");
        let names: &[&str] = if version.major() >= 12 { &["pnpm", "pn", "pnpx", "pnx"] } else { &["pnpm"] };
        for name in names {
            let target = bin.join(name);
            if *name == "pnpm" || target.exists() {
                let _ = fs::remove_file(&target);
                fs::copy(&binary, &target)?;
            }
        }
        fs::remove_dir_all(platform)?;
    }
    let executable = bin.join(tool.id());
    let mode = fs::metadata(&executable)
        .map_err(|_| io::Error::other(format!("it has no {}", tool.id())))?
        .permissions()
        .mode();
    fs::set_permissions(&executable, fs::Permissions::from_mode(mode | 0o755))?;
    if tool == Tool::Pnpm && version.major() >= 12 {
        for name in ["pn", "pnpx", "pnx"] {
            if let Ok(metadata) = fs::metadata(bin.join(name)) {
                fs::set_permissions(bin.join(name), fs::Permissions::from_mode(metadata.permissions().mode() | 0o755))?;
            }
        }
    }
    Ok(())
}

/// An npm version document's `dist.tarball` and its sha512 `dist.integrity`.
fn npm_dist(document: &Value, url: &str) -> Result<(String, Checksum), ToolchainError> {
    let tarball = document["dist"]["tarball"].as_str().ok_or_else(|| bad_document(url, "no tarball"))?;
    let integrity = document["dist"]["integrity"].as_str().ok_or_else(|| bad_document(url, "no integrity"))?;
    let digest = integrity
        .split_whitespace()
        .find_map(|sri| sri.strip_prefix("sha512-"))
        .and_then(|b64| base64::engine::general_purpose::STANDARD.decode(b64).ok())
        .ok_or_else(|| bad_document(url, "its integrity isn't sha512"))?;
    Ok((tarball.to_owned(), Checksum::Sha512(digest)))
}

/// The SHA-256 listed for `file` in a `SHASUMS256.txt` (`<hex>  <file>`).
fn sha256_from_shasums(
    downloads: &dyn Downloads,
    tool: Tool,
    version: &Version,
    url: &str,
    file: &str,
) -> Result<Checksum, ToolchainError> {
    let sums = fetch_text(downloads, url).map_err(|e| not_published(e, tool, version))?;
    let hex = sums
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, name)| name.trim().trim_start_matches('*') == file)
        .map(|(hex, _)| hex)
        .ok_or_else(|| bad_document(url, &format!("it lists no {file}")))?;
    let bytes = (0..hex.len())
        .step_by(2)
        .map(|i| hex.get(i..i + 2).and_then(|b| u8::from_str_radix(b, 16).ok()))
        .collect::<Option<Vec<u8>>>()
        .filter(|b| b.len() == 32)
        .ok_or_else(|| bad_document(url, &format!("its checksum for {file} isn't SHA-256")))?;
    Ok(Checksum::Sha256(bytes))
}

/// A 404 on a version's own documents means the version doesn't exist.
fn not_published(error: ToolchainError, tool: Tool, version: &Version) -> ToolchainError {
    match error {
        ToolchainError::Download { error: DownloadError::Status(404), .. } => {
            ToolchainError::NotPublished { tool, version: version.clone() }
        }
        other => other,
    }
}

fn fetch_text(downloads: &dyn Downloads, url: &str) -> Result<String, ToolchainError> {
    let mut body = Vec::new();
    downloads
        .fetch(url, &mut body)
        .map_err(|error| ToolchainError::Download { url: url.to_owned(), error })?;
    String::from_utf8(body).map_err(|_| bad_document(url, "it isn't UTF-8"))
}

fn fetch_json(downloads: &dyn Downloads, url: &str) -> Result<Value, ToolchainError> {
    let text = fetch_text(downloads, url)?;
    serde_json::from_str(&text).map_err(|e| bad_document(url, &format!("it isn't valid JSON ({e})")))
}

fn bad_document(url: &str, reason: &str) -> ToolchainError {
    ToolchainError::BadDocument { url: url.to_owned(), reason: reason.to_owned() }
}
