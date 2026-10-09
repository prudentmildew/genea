//! Read-only git (ticket #56; spec #19, Git): the current branch, and the
//! open files' text at HEAD for the gutter markers. Read in-process with
//! `gix`, so nothing needs to be installed (ADR 0005), and always on a
//! background job: the repository is opened afresh by each read.

use std::path::{Path, PathBuf};

use crate::{jobs::Jobs, watcher::FileChanges, workbench::ProjectId};

/// A project's git state. A project outside any repository has none, and
/// shows no branch and no markers.
pub(crate) struct Git {
    project: ProjectId,
    root: PathBuf,
    /// The branch HEAD is on, or the short commit id while detached.
    branch: Option<String>,
    /// The repository's git folder, once a read has found one.
    git_dir: Option<PathBuf>,
    /// Bumped by every read of HEAD, so a slow read can't replace a newer one.
    generation: u64,
}

/// What a read of HEAD found.
struct Head {
    branch: Option<String>,
    git_dir: Option<PathBuf>,
}

impl Git {
    pub(crate) fn new(project: ProjectId, root: PathBuf) -> Self {
        Git { project, root, branch: None, git_dir: None, generation: 0 }
    }

    /// Whether these changes on disk (from the watcher) may have moved
    /// HEAD: a checkout or a commit rewrites `HEAD`, a ref or
    /// `packed-refs`, and `git init` creates `.git`. Other writes to the
    /// git folder (the index, objects, logs) don't. A repository whose git
    /// folder is outside the project isn't watched.
    pub(crate) fn head_may_have_moved(&self, changes: &FileChanges) -> bool {
        let default = self.root.join(".git");
        let git_dirs = [Some(&default), self.git_dir.as_ref()];
        changes.rescan
            || changes.paths.iter().any(|path| {
                git_dirs.iter().flatten().any(|dir| match path.strip_prefix(dir) {
                    Ok(inside) => {
                        let first = inside.components().next().map(|c| c.as_os_str().to_string_lossy());
                        match first.as_deref() {
                            None => true,
                            Some(name) => name.starts_with("HEAD") || name == "refs" || name.starts_with("packed-refs"),
                        }
                    }
                    Err(_) => false,
                })
            })
    }

    pub(crate) fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Reads HEAD again in the background: at open, and when it may have
    /// moved (a checkout, a commit, `git init`).
    pub(crate) fn reload(&mut self, jobs: &Jobs) {
        self.generation += 1;
        let (id, generation, root) = (self.project, self.generation, self.root.clone());
        jobs.spawn("read git HEAD", move || {
            let head = read_head(&root);
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                let git = &mut project.git;
                if git.generation != generation {
                    return;
                }
                git.branch = head.branch;
                git.git_dir = head.git_dir;
            })
        });
    }
}

/// Reads the repository that `root` is in, if any. Every failure (no
/// repository, a broken one) reads as no repository.
fn read_head(root: &Path) -> Head {
    let Ok(repo) = gix::discover(root) else { return Head { branch: None, git_dir: None } };
    let branch = match repo.head_name() {
        Ok(Some(name)) => Some(name.shorten().to_string()),
        Ok(None) => repo.head_id().ok().map(|id| id.to_hex_with_len(7).to_string()),
        Err(_) => None,
    };
    Head { branch, git_dir: Some(repo.git_dir().to_owned()) }
}
