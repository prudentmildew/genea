//! The workspace model (ticket #40): a project's packages and their
//! `package.json` scripts, for the script runner.
//!
//! The packages are the root package plus the folders matched by
//! `pnpm-workspace.yaml`'s `packages` (pnpm) or the root `package.json`'s
//! `workspaces` (Bun, and npm or Yarn, whose package roots count for
//! foreign config, ticket #51), never inside `node_modules`. They are read in the
//! background when the project opens, and again whenever the watcher sees
//! `pnpm-workspace.yaml` or any `package.json` change (or a package's
//! folder go).

use std::{
    fs,
    path::{Path, PathBuf},
};

use globset::{GlobBuilder, GlobSet, GlobSetBuilder};
use serde_json::Value;

use crate::{
    foreign::{self, ForeignConfig},
    jobs::Jobs,
    toolchain::FOREIGN_LOCKFILES,
    view::{PackageScripts, Script},
    watcher::FileChanges,
    workbench::ProjectId,
};

const PACKAGE_JSON: &str = "package.json";
const PNPM_WORKSPACE: &str = "pnpm-workspace.yaml";

pub(crate) struct Workspace {
    project: ProjectId,
    root: PathBuf,
    /// Root first, then the others in path order.
    packages: Vec<PackageScripts>,
    /// Foreign formatter and linter config in the packages' folders
    /// (ticket #51).
    foreign_configs: Vec<ForeignConfig>,
    /// Bumped by every read, so a slow one can't replace a newer one.
    generation: u64,
}

impl Workspace {
    pub(crate) fn new(project: ProjectId, root: PathBuf) -> Self {
        Workspace { project, root, packages: Vec::new(), foreign_configs: Vec::new(), generation: 0 }
    }

    /// Finds the packages and reads their scripts in the background.
    pub(crate) fn discover(&mut self, jobs: &Jobs) {
        self.generation += 1;
        let (id, generation, root) = (self.project, self.generation, self.root.clone());
        jobs.spawn("discover packages", move || {
            let packages = discover(&root);
            // The root always counts, even without a readable `package.json`.
            let others = packages.iter().map(|p| p.path.as_path()).filter(|path| !path.as_os_str().is_empty());
            let foreign_configs = foreign::find(&root, std::iter::once(Path::new("")).chain(others));
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.workspace.generation == generation {
                    project.workspace.packages = packages;
                    project.workspace.foreign_configs = foreign_configs;
                    project.update_foreign_tools();
                }
            })
        });
    }

    /// Files changed on disk: reads the packages again if a `package.json`,
    /// `pnpm-workspace.yaml` or a foreign tool's config changed, or a
    /// package's folder may have gone.
    pub(crate) fn files_changed(&mut self, changes: &FileChanges, jobs: &Jobs) {
        let relevant = |path: &PathBuf| {
            let Ok(relative) = path.strip_prefix(&self.root) else { return false };
            if relative.components().any(|c| c.as_os_str() == "node_modules") {
                return false;
            }
            relative == Path::new(PNPM_WORKSPACE)
                || FOREIGN_LOCKFILES.iter().any(|(lockfile, _)| relative == Path::new(lockfile))
                || path.file_name().is_some_and(|name| name == PACKAGE_JSON)
                || path.file_name().and_then(|name| name.to_str()).is_some_and(foreign::is_config_file)
                || self.packages.iter().any(|package| !relative.as_os_str().is_empty() && package.path.starts_with(relative))
        };
        if changes.rescan || changes.paths.iter().any(relevant) {
            self.discover(jobs);
        }
    }

    /// The packages, root first.
    pub(crate) fn packages(&self) -> &[PackageScripts] {
        &self.packages
    }

    /// Foreign formatter and linter config at the root or a package root.
    pub(crate) fn foreign_configs(&self) -> &[ForeignConfig] {
        &self.foreign_configs
    }

    /// The package in folder `path` (relative to the root), if it is one.
    pub(crate) fn package(&self, path: &Path) -> Option<&PackageScripts> {
        self.packages.iter().find(|package| package.path == path)
    }
}

/// The project's packages: none without a root `package.json`.
fn discover(root: &Path) -> Vec<PackageScripts> {
    let Some(manifest) = read_manifest(&root.join(PACKAGE_JSON)) else { return Vec::new() };
    let root_name = root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let patterns = if uses_workspaces_field(root, &manifest) { workspaces_patterns(&manifest) } else { pnpm_patterns(root) };
    let mut packages = vec![package(PathBuf::new(), &root_name, &manifest)];
    let mut others: Vec<PackageScripts> = match Patterns::new(&patterns) {
        Some(patterns) => find_packages(root, &patterns),
        None => Vec::new(),
    };
    others.sort_by(|a, b| a.path.cmp(&b.path));
    packages.extend(others);
    packages
}

fn read_manifest(path: &Path) -> Option<Value> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok().filter(Value::is_object)
}

fn package(path: PathBuf, fallback_name: &str, manifest: &Value) -> PackageScripts {
    let name = manifest.get("name").and_then(Value::as_str).filter(|n| !n.is_empty()).unwrap_or(fallback_name);
    let scripts = match manifest.get("scripts") {
        Some(Value::Object(scripts)) => scripts
            .iter()
            .filter_map(|(name, command)| Some(Script { name: name.clone(), command: command.as_str()?.to_owned() }))
            .collect(),
        _ => Vec::new(),
    };
    PackageScripts { name: name.to_owned(), path, scripts }
}

/// Whether the root package's workspace is its `workspaces` field: it pins
/// Bun, npm or Yarn as its package manager, or pins none and has an npm or
/// Yarn lockfile. Anything else is a pnpm workspace (pnpm is the default).
fn uses_workspaces_field(root: &Path, manifest: &Value) -> bool {
    match manifest.get("packageManager").and_then(Value::as_str) {
        Some(pin) => ["bun@", "npm@", "yarn@"].iter().any(|tool| pin.starts_with(tool)),
        None => FOREIGN_LOCKFILES.iter().any(|(lockfile, _)| root.join(lockfile).is_file()),
    }
}

/// The root `workspaces` field: a list of globs, or `{ "packages": [..] }`.
fn workspaces_patterns(manifest: &Value) -> Vec<String> {
    let list = match manifest.get("workspaces") {
        Some(Value::Array(list)) => list,
        Some(Value::Object(object)) => match object.get("packages") {
            Some(Value::Array(list)) => list,
            _ => return Vec::new(),
        },
        _ => return Vec::new(),
    };
    list.iter().filter_map(Value::as_str).map(str::to_owned).collect()
}

/// `pnpm-workspace.yaml`'s top-level `packages` list.
fn pnpm_patterns(root: &Path) -> Vec<String> {
    let Ok(text) = fs::read_to_string(root.join(PNPM_WORKSPACE)) else { return Vec::new() };
    yaml_string_list(&text, "packages")
}

/// The strings of a top-level `key`'s list in a YAML document (block or
/// flow style), read with the YAML grammar the editor highlights with.
fn yaml_string_list(text: &str, key: &str) -> Vec<String> {
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&tree_sitter_yaml::LANGUAGE.into()).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(text, None) else { return Vec::new() };
    let source = text.as_bytes();
    // stream → document → block_node → block_mapping → block_mapping_pair
    let mut pairs = Vec::new();
    let mut cursor = tree.walk();
    for document in tree.root_node().children(&mut cursor).filter(|n| n.kind() == "document") {
        let mut cursor = document.walk();
        for node in document.children(&mut cursor).filter(|n| n.kind() == "block_node") {
            let mut cursor = node.walk();
            for mapping in node.children(&mut cursor).filter(|n| n.kind() == "block_mapping") {
                let mut cursor = mapping.walk();
                pairs.extend(mapping.children(&mut cursor).filter(|n| n.kind() == "block_mapping_pair"));
            }
        }
    }
    let Some(value) = pairs.into_iter().find_map(|pair| {
        let name = pair.child_by_field_name("key")?;
        (scalar(name, source)? == key).then(|| pair.child_by_field_name("value")).flatten()
    }) else {
        return Vec::new();
    };
    let mut strings = Vec::new();
    collect_scalars(value, source, &mut strings);
    strings
}

fn collect_scalars(node: tree_sitter::Node, source: &[u8], out: &mut Vec<String>) {
    if let Some(text) = scalar(node, source) {
        out.push(text);
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_scalars(child, source, out);
    }
}

/// A scalar node's string, unquoted; `None` for other nodes.
fn scalar(node: tree_sitter::Node, source: &[u8]) -> Option<String> {
    let text = node.utf8_text(source).ok()?;
    match node.kind() {
        "plain_scalar" => Some(text.trim().to_owned()),
        "single_quote_scalar" => Some(text.get(1..text.len().saturating_sub(1))?.replace("''", "'")),
        "double_quote_scalar" => Some(text.get(1..text.len().saturating_sub(1))?.replace("\\\"", "\"").replace("\\\\", "\\")),
        "flow_node" | "block_node" if node.named_child_count() == 1 => scalar(node.named_child(0)?, source),
        _ => None,
    }
}

/// A workspace's globs: folders matching an include and no `!` exclude.
struct Patterns {
    include: GlobSet,
    exclude: GlobSet,
    /// Where each include can match: its literal leading folders, and how
    /// many more levels below them (`None`: any, after a `**`).
    reach: Vec<(PathBuf, Option<usize>)>,
}

impl Patterns {
    /// `None` without any include pattern.
    fn new(patterns: &[String]) -> Option<Self> {
        let (mut include, mut exclude) = (GlobSetBuilder::new(), GlobSetBuilder::new());
        let mut reach = Vec::new();
        for pattern in patterns {
            let (negated, pattern) = match pattern.strip_prefix('!') {
                Some(negated) => (true, negated),
                None => (false, pattern.as_str()),
            };
            let pattern = pattern.trim().trim_start_matches("./").trim_end_matches('/');
            let Ok(glob) = GlobBuilder::new(pattern).literal_separator(true).build() else { continue };
            if negated {
                exclude.add(glob);
            } else {
                include.add(glob);
                reach.push(reach_of(pattern));
            }
        }
        if reach.is_empty() {
            return None;
        }
        Some(Patterns { include: include.build().ok()?, exclude: exclude.build().ok()?, reach })
    }

    fn matches(&self, relative: &Path) -> bool {
        self.include.is_match(relative) && !self.exclude.is_match(relative)
    }

}

/// Whether a folder (relative to the root) may be, or hold, a match of an
/// include with this `reach`: it leads to the include's literal folders,
/// or is within its reach below them.
fn may_reach(reach: &[(PathBuf, Option<usize>)], relative: &Path) -> bool {
    let depth = relative.components().count();
    reach.iter().any(|(prefix, below)| {
        prefix.starts_with(relative)
            || (relative.starts_with(prefix) && below.is_none_or(|below| depth <= prefix.components().count() + below))
    })
}

/// An include pattern's literal leading folders, and how many levels below
/// them it can match (`None` after a `**`).
fn reach_of(pattern: &str) -> (PathBuf, Option<usize>) {
    let is_glob = |part: &str| part.contains(['*', '?', '[', '{', '\\']);
    let parts: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty() && *part != ".").collect();
    let literal = parts.iter().take_while(|part| !is_glob(part)).count();
    let rest = &parts[literal..];
    let below = if rest.contains(&"**") { None } else { Some(rest.len()) };
    (parts[..literal].iter().collect(), below)
}

/// Every folder below the root that the patterns match and that has a
/// `package.json`. Only folders the patterns can reach are walked, never
/// `node_modules` or `.git`, nor what the project's `.gitignore` files
/// ignore (as in project search: nothing above the root or user-wide).
fn find_packages(root: &Path, patterns: &Patterns) -> Vec<PackageScripts> {
    let walk_root = root.to_owned();
    let reach = patterns.reach.clone();
    ignore::WalkBuilder::new(root)
        .hidden(false)
        .parents(false)
        .ignore(false)
        .git_global(false)
        .require_git(false)
        .filter_entry(move |entry| {
            if !entry.file_type().is_some_and(|t| t.is_dir()) {
                return false;
            }
            if entry.file_name() == "node_modules" || entry.file_name() == ".git" {
                return false;
            }
            entry.path().strip_prefix(&walk_root).is_ok_and(|relative| may_reach(&reach, relative))
        })
        .build()
        .filter_map(Result::ok)
        .filter(|entry| entry.depth() > 0)
        .filter_map(|entry| {
            let relative = entry.path().strip_prefix(root).ok()?.to_path_buf();
            if !patterns.matches(&relative) {
                return None;
            }
            let manifest = read_manifest(&entry.path().join(PACKAGE_JSON))?;
            let fallback = relative.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            Some(package(relative.clone(), &fallback, &manifest))
        })
        .collect()
}
