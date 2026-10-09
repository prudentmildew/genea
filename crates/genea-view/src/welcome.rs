//! The welcome window: binds a `WelcomeWindow` to the core's welcome.
//!
//! The core decides when the welcome shows (`Workbench::welcome` is `Some`
//! exactly while no project is open); `sync` shows or hides the window to
//! match and pushes the recent projects when they change.

use std::{path::PathBuf, rc::Rc};

use genea_core::{RecentProject, Workbench};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{RecentEntry, WelcomeWindow};

pub struct WelcomeController {
    pub window: WelcomeWindow,
    /// The list as last pushed to the window, to map a click to a folder.
    recent: Vec<RecentProject>,
    visible: bool,
    /// Why the last open failed, if it did.
    pub notice: Option<String>,
}

impl WelcomeController {
    pub fn new() -> Result<Self, slint::PlatformError> {
        let window = WelcomeWindow::new()?;
        window.set_version(env!("CARGO_PKG_VERSION").into());
        Ok(WelcomeController { window, recent: Vec::new(), visible: false, notice: None })
    }

    /// The folder of the recent project at `index` in the list.
    pub fn recent_root(&self, index: usize) -> Option<PathBuf> {
        self.recent.get(index).map(|recent| recent.root.clone())
    }

    pub fn sync(&mut self, workbench: &Workbench) {
        let Some(welcome) = workbench.welcome() else {
            if self.visible {
                self.visible = false;
                if let Err(error) = self.window.hide() {
                    eprintln!("genea: couldn't hide the welcome window: {error}");
                }
            }
            return;
        };
        if welcome.recent_projects != self.recent {
            let entries: Vec<RecentEntry> = welcome.recent_projects.iter().map(entry).collect();
            self.window.set_recent_projects(ModelRc::from(Rc::new(VecModel::from(entries))));
            self.recent = welcome.recent_projects;
        }
        self.window.set_notice(self.notice.clone().unwrap_or_default().into());
        if !self.visible {
            self.visible = true;
            if let Err(error) = self.window.show() {
                eprintln!("genea: couldn't show the welcome window: {error}");
            }
        }
    }

    /// Notes that the user closed the window.
    pub fn closed(&mut self) {
        self.visible = false;
    }
}

fn entry(recent: &RecentProject) -> RecentEntry {
    let parent = recent.root.parent().unwrap_or(&recent.root);
    let location = match std::env::home_dir().and_then(|home| parent.strip_prefix(home).ok().map(PathBuf::from)) {
        Some(relative) if relative.as_os_str().is_empty() => "~".to_owned(),
        Some(relative) => format!("~/{}", relative.display()),
        None => parent.display().to_string(),
    };
    RecentEntry { name: recent.name.clone().into(), location: location.into() }
}
