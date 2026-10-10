//! Client-side file watching for language servers (spec #19: Genea's
//! watcher feeds every server through `workspace/didChangeWatchedFiles`, so
//! no server runs a watcher of its own).
//!
//! Genea advertises `didChangeWatchedFiles.dynamicRegistration`, so a server
//! registers the globs it cares about (`client/registerCapability`), and
//! changes from the project watcher that match them are sent to it, each as
//! created, changed or deleted.

use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::Arc,
};

use gen_lsp_types::{
    BaseUri, DidChangeWatchedFilesRegistrationOptions, FileChangeType, FileEvent, GlobPattern, Registration,
    WatchKind,
};
use globset::{GlobBuilder, GlobMatcher};

use super::text;

/// Git's folder, whose changes no server is told about.
const REPOSITORY: &str = ".git";

/// The method a file-watching registration is for.
pub(crate) const METHOD: &str = "workspace/didChangeWatchedFiles";

/// Every glob a server has registered. Cheap to clone, so matching runs in
/// the background.
#[derive(Clone, Default)]
pub(crate) struct Watchers {
    registrations: Vec<(String, Arc<[Watch]>)>,
}

struct Watch {
    glob: GlobMatcher,
    /// A relative pattern's base folder, lowercased: the glob matches paths
    /// below it. Without one, the glob matches the whole absolute path.
    base: Option<PathBuf>,
    /// Which kinds of change it wants (`WatchKind` bits; all by default).
    kinds: u32,
}

impl Watchers {
    /// Takes a `client/registerCapability` registration for file watching.
    /// Globs that can't be parsed are skipped.
    pub(crate) fn register(&mut self, registration: &Registration) {
        if registration.method != METHOD {
            return;
        }
        let Some(options) = registration
            .register_options
            .clone()
            .and_then(|options| serde_json::from_value::<DidChangeWatchedFilesRegistrationOptions>(options).ok())
        else {
            return;
        };
        let watches: Vec<Watch> = options
            .watchers
            .iter()
            .filter_map(|watcher| {
                let (pattern, base) = match &watcher.glob_pattern {
                    GlobPattern::Pattern(pattern) => (pattern.as_str(), None),
                    GlobPattern::RelativePattern(relative) => {
                        let base = match &relative.base_uri {
                            BaseUri::Uri(uri) => text::path(uri.as_ref()),
                            BaseUri::WorkspaceFolder(folder) => text::path(folder.uri.as_ref()),
                        };
                        (relative.pattern.as_str(), Some(base?))
                    }
                };
                // tsgo registers some globs with the path lowercased: macOS
                // file systems ignore case, and so does it.
                let glob = GlobBuilder::new(pattern)
                    .literal_separator(true)
                    .case_insensitive(true)
                    .build()
                    .ok()?
                    .compile_matcher();
                let base = base.map(|base: PathBuf| PathBuf::from(base.to_string_lossy().to_lowercase()));
                let kinds = watcher.kind.map_or(7, u32::from);
                Some(Watch { glob, base, kinds })
            })
            .collect();
        self.unregister(&registration.id);
        self.registrations.push((registration.id.clone(), watches.into()));
    }

    pub(crate) fn unregister(&mut self, id: &str) {
        self.registrations.retain(|(registered, _)| registered != id);
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.registrations.iter().all(|(_, watches)| watches.is_empty())
    }

    /// The events to send for changed paths (absolute): those a glob
    /// matches, each as deleted if it is gone, created if the watcher saw it
    /// appear (created, or renamed into place), and changed otherwise. Reads
    /// the disk: call it in the background.
    pub(crate) fn events(&self, paths: &BTreeSet<PathBuf>, created: &BTreeSet<PathBuf>) -> Vec<FileEvent> {
        paths
            .iter()
            .filter_map(|path| {
                let kind = if path.symlink_metadata().is_err() {
                    FileChangeType::Deleted
                } else if created.contains(path) {
                    FileChangeType::Created
                } else {
                    FileChangeType::Changed
                };
                let bit = match kind {
                    FileChangeType::Created => u32::from(WatchKind::Create),
                    FileChangeType::Deleted => u32::from(WatchKind::Delete),
                    _ => u32::from(WatchKind::Change),
                };
                self.wants(path, bit).then(|| FileEvent { uri: text::uri(path).into(), kind })
            })
            .collect()
    }

    fn wants(&self, path: &Path, bit: u32) -> bool {
        // The repository's own files are never a server's business.
        if path.components().any(|c| c.as_os_str() == REPOSITORY) {
            return false;
        }
        let lowercase = PathBuf::from(path.to_string_lossy().to_lowercase());
        self.registrations.iter().flat_map(|(_, watches)| watches.iter()).any(|watch| {
            watch.kinds & bit != 0
                && match &watch.base {
                    Some(base) => lowercase.strip_prefix(base).is_ok_and(|rest| watch.glob.is_match(rest)),
                    None => watch.glob.is_match(path),
                }
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn registration(watchers: serde_json::Value) -> Registration {
        Registration {
            id: "w".into(),
            method: METHOD.into(),
            register_options: Some(json!({ "watchers": watchers })),
        }
    }

    #[test]
    fn plain_and_relative_globs_match_absolute_paths() {
        let folder = tempfile::tempdir().unwrap();
        let root = folder.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        for file in ["src/a.ts", "src/b.css", "tsconfig.json"] {
            std::fs::write(root.join(file), "").unwrap();
        }
        let mut watchers = Watchers::default();
        watchers.register(&registration(json!([
            { "globPattern": "**/*.{ts,tsx}" },
            { "globPattern": { "baseUri": text::uri(&root), "pattern": "tsconfig.json" }, "kind": 2 },
        ])));
        let paths: BTreeSet<PathBuf> =
            ["src/a.ts", "src/b.css", "tsconfig.json", "src/gone.ts", "src/deep/tsconfig.json"].map(|p| root.join(p)).into();
        let created = BTreeSet::from([root.join("src/a.ts")]);

        let events: Vec<(String, FileChangeType)> =
            watchers.events(&paths, &created).into_iter().map(|e| (e.uri.0, e.kind)).collect();

        assert_eq!(
            events,
            [
                (text::uri(&root.join("src/a.ts")), FileChangeType::Created),
                (text::uri(&root.join("src/gone.ts")), FileChangeType::Deleted),
                (text::uri(&root.join("tsconfig.json")), FileChangeType::Changed),
            ]
        );
        watchers.unregister("w");
        assert!(watchers.is_empty());
    }

    #[test]
    fn globs_ignore_case_as_macos_does_and_the_repository_folder_is_never_watched() {
        // tsgo registers its program's folder lowercased.
        let mut watchers = Watchers::default();
        watchers.register(&registration(json!([{ "globPattern": "/users/me/app/**/*", "kind": 7 }])));
        let index = format!("/Users/Me/App/{REPOSITORY}/index");
        let paths: BTreeSet<PathBuf> = [PathBuf::from("/Users/Me/App/src/a.ts"), PathBuf::from(index)].into();

        let events: Vec<String> = watchers.events(&paths, &BTreeSet::new()).into_iter().map(|e| e.uri.0).collect();

        assert_eq!(events, ["file:///Users/Me/App/src/a.ts"]);
    }
}
