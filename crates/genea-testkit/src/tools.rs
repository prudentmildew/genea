//! Fake Node, Bun and pnpm releases, published on the download fixture
//! server under the URLs Genea fetches them from (ADR 0005):
//!
//! - Node: `nodejs.org/dist/index.json`, `SHASUMS256.txt` and the
//!   `darwin-arm64.tar.xz` archive;
//! - Bun: the GitHub releases list, `SHASUMS256.txt` and the
//!   `bun-darwin-aarch64.zip` asset;
//! - pnpm: npm's `@pnpm/exe` documents, its darwin-arm64 platform package
//!   (`@pnpm/exe.darwin-arm64` from 12, `@pnpm/macos-arm64` before) with a
//!   sha512 `integrity`, and the package tarball.
//!
//! Each tool is a small shell script that prints its version. The
//! `*_with_bad_checksum` variants publish an archive that doesn't match its
//! published checksum.

use std::io::Write;

use base64::Engine;
use serde_json::{Value, json};
use sha2::{Digest, Sha256, Sha512};

use crate::DownloadServer;

pub const NODE_INDEX: &str = "https://nodejs.org/dist/index.json";
pub const BUN_RELEASES: &str = "https://api.github.com/repos/oven-sh/bun/releases?per_page=100";
pub const PNPM_PACKUMENT: &str = "https://registry.npmjs.org/@pnpm%2fexe";

/// Publishes fake tool releases. Get one from `TestHost::tools`.
pub struct FakeTools<'a> {
    server: &'a DownloadServer,
}

impl<'a> FakeTools<'a> {
    pub fn new(server: &'a DownloadServer) -> Self {
        FakeTools { server }
    }

    /// Publishes Node `version` (`24.18.0`) and returns its archive's URL.
    pub fn node(&self, version: &str) -> String {
        self.publish_node(version, false)
    }

    /// Publishes Node `version` with an archive that fails its checksum.
    pub fn node_with_bad_checksum(&self, version: &str) -> String {
        self.publish_node(version, true)
    }

    /// Publishes Bun `version` and returns its archive's URL.
    pub fn bun(&self, version: &str) -> String {
        self.publish_bun(version, false)
    }

    /// Publishes Bun `version` with an archive that fails its checksum.
    pub fn bun_with_bad_checksum(&self, version: &str) -> String {
        self.publish_bun(version, true)
    }

    /// Publishes pnpm `version` and returns its tarball's URL.
    pub fn pnpm(&self, version: &str) -> String {
        self.publish_pnpm(version, false)
    }

    /// Publishes pnpm `version` with a tarball that fails its integrity.
    pub fn pnpm_with_bad_checksum(&self, version: &str) -> String {
        self.publish_pnpm(version, true)
    }

    fn publish_node(&self, version: &str, bad: bool) -> String {
        let top = format!("node-v{version}-darwin-arm64");
        let tar = tar_of(&[
            (format!("{top}/bin/node"), script(&format!("v{version}")), 0o755),
            (format!("{top}/include/node/node.h"), b"/* node */\n".to_vec(), 0o644),
        ]);
        let archive = xz(&tar);
        let name = format!("{top}.tar.xz");
        let sums = format!(
            "{}  node-v{version}-darwin-arm64.tar.gz\n{}  {name}\n{}  node-v{version}-linux-x64.tar.xz\n",
            sha256_hex(b"gz"),
            sha256_hex(&archive),
            sha256_hex(b"linux"),
        );
        let base = format!("https://nodejs.org/dist/v{version}");
        self.server.publish(format!("{base}/SHASUMS256.txt"), sums);
        let url = format!("{base}/{name}");
        self.server.publish(&url, tampered(archive, bad));

        let mut index = self.json_array(NODE_INDEX);
        index.retain(|e| e["version"] != format!("v{version}"));
        index.push(json!({
            "version": format!("v{version}"),
            "date": "2026-01-01",
            "files": ["linux-x64", "osx-arm64-tar", "osx-x64-tar"],
            "npm": "11.0.0",
            "lts": false,
            "security": false,
        }));
        index.sort_by_key(|e| std::cmp::Reverse(version_key(e["version"].as_str().unwrap())));
        self.server.publish(NODE_INDEX, serde_json::to_vec(&index).unwrap());
        url
    }

    fn publish_bun(&self, version: &str, bad: bool) -> String {
        let archive = zip_of(&[("bun-darwin-aarch64/bun".into(), script(version), 0o755)]);
        let base = format!("https://github.com/oven-sh/bun/releases/download/bun-v{version}");
        let sums = format!(
            "{}  bun-darwin-aarch64-profile.zip\n{}  bun-darwin-aarch64.zip\n{}  bun-linux-x64.zip\n",
            sha256_hex(b"profile"),
            sha256_hex(&archive),
            sha256_hex(b"linux"),
        );
        self.server.publish(format!("{base}/SHASUMS256.txt"), sums);
        let url = format!("{base}/bun-darwin-aarch64.zip");
        self.server.publish(&url, tampered(archive, bad));

        let mut releases = self.json_array(BUN_RELEASES);
        releases.retain(|r| r["tag_name"] != format!("bun-v{version}"));
        releases.push(json!({ "tag_name": format!("bun-v{version}"), "draft": false, "prerelease": false }));
        releases.sort_by_key(|r| std::cmp::Reverse(version_key(r["tag_name"].as_str().unwrap())));
        self.server.publish(BUN_RELEASES, serde_json::to_vec(&releases).unwrap());
        url
    }

    fn publish_pnpm(&self, version: &str, bad: bool) -> String {
        let major: u64 = version.split('.').next().unwrap().parse().unwrap();
        // pnpm's macOS package was renamed at 12, and before 12 its binary
        // isn't marked executable.
        let (package, file, mode) = if major >= 12 {
            ("@pnpm/exe.darwin-arm64", "exe.darwin-arm64", 0o755)
        } else {
            ("@pnpm/macos-arm64", "macos-arm64", 0o644)
        };
        let tgz = gzip(&tar_of(&[
            ("package/pnpm".into(), script(version), mode),
            ("package/package.json".into(), format!("{{\"name\":\"{package}\"}}").into_bytes(), 0o644),
        ]));
        let integrity = format!("sha512-{}", base64::engine::general_purpose::STANDARD.encode(Sha512::digest(&tgz)));
        let tarball = format!("https://registry.npmjs.org/{package}/-/{file}-{version}.tgz");
        self.server.publish(&tarball, tampered(tgz, bad));
        let escaped = package.replace('/', "%2f");
        self.server.publish(
            format!("https://registry.npmjs.org/{escaped}/{version}"),
            serde_json::to_vec(&json!({
                "name": package,
                "version": version,
                "dist": { "tarball": tarball, "integrity": integrity, "shasum": "0000" },
            }))
            .unwrap(),
        );

        let manifest = json!({
            "name": "@pnpm/exe",
            "version": version,
            "optionalDependencies": { package: version, "@pnpm/linux-x64": version },
        });
        self.server.publish(format!("{PNPM_PACKUMENT}/{version}"), serde_json::to_vec(&manifest).unwrap());
        let mut packument = self
            .server
            .published(PNPM_PACKUMENT)
            .map(|b| serde_json::from_slice(&b).unwrap())
            .unwrap_or_else(|| json!({ "name": "@pnpm/exe", "dist-tags": {}, "versions": {} }));
        packument["versions"][version] = manifest;
        self.server.publish(PNPM_PACKUMENT, serde_json::to_vec(&packument).unwrap());
        tarball
    }

    fn json_array(&self, url: &str) -> Vec<Value> {
        self.server.published(url).map(|b| serde_json::from_slice(&b).unwrap()).unwrap_or_default()
    }
}

/// A tool stand-in: a shell script that prints `version`.
fn script(version: &str) -> Vec<u8> {
    format!("#!/bin/sh\necho {version}\n").into_bytes()
}

/// The archive with one byte changed, if `bad`.
fn tampered(mut archive: Vec<u8>, bad: bool) -> Vec<u8> {
    if bad {
        let last = archive.len() - 1;
        archive[last] ^= 0xff;
    }
    archive
}

fn version_key(version: &str) -> Vec<u64> {
    version.trim_start_matches("bun-").trim_start_matches('v').split('.').map(|p| p.parse().unwrap()).collect()
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

fn tar_of(entries: &[(String, Vec<u8>, u32)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(Vec::new());
    for (path, contents, mode) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(contents.len() as u64);
        header.set_mode(*mode);
        header.set_entry_type(tar::EntryType::Regular);
        builder.append_data(&mut header, path, contents.as_slice()).unwrap();
    }
    builder.into_inner().unwrap()
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

fn xz(bytes: &[u8]) -> Vec<u8> {
    let mut writer = lzma_rust2::XzWriter::new(Vec::new(), lzma_rust2::XzOptions::with_preset(6)).unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap()
}

fn zip_of(entries: &[(String, Vec<u8>, u32)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (path, contents, mode) in entries {
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated)
            .unix_permissions(*mode);
        writer.start_file(path.as_str(), options).unwrap();
        writer.write_all(contents).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
