//! The New Project dialog (ticket #61): binds a `NewProjectWindow` to the
//! core's `NewProjectDialog`.
//!
//! The core decides when the dialog shows (`Workbench::new_project_dialog`
//! is `Some` while it is open); `sync` shows or hides the window to match
//! and pushes what changed. Each control turns into a `NewProjectCommand`.
//! A created project opens in the core; the app gives it a window like any
//! open project without one.

use std::{path::Path, rc::Rc};

use genea_core::{NewProjectDialog, Template, Workbench};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::NewProjectWindow;

/// The templates in the order the picker lists them, with their names and
/// what they make.
pub const TEMPLATES: [(Template, &str, &str); 3] = [
    (
        Template::Frontend,
        "Frontend",
        "A React 19 + Vite 8 single-page app, with Vitest, Oxlint, Oxfmt and TypeScript 7.",
    ),
    (Template::Backend, "Backend", "A Hono 4 server with no build step, run from source by Node type stripping."),
    (
        Template::FullStack,
        "Full-stack",
        "A workspace of apps/web, apps/api and packages/shared; the API's types reach the web app through Hono RPC.",
    ),
];

pub struct NewProjectController {
    pub window: NewProjectWindow,
    /// The dialog as last pushed to the window, to push only changes and
    /// map a picked index to its pin.
    shown: Option<NewProjectDialog>,
}

impl NewProjectController {
    pub fn new() -> Result<Self, slint::PlatformError> {
        let window = NewProjectWindow::new()?;
        let names: Vec<SharedString> = TEMPLATES.iter().map(|(_, name, _)| (*name).into()).collect();
        window.set_templates(ModelRc::from(Rc::new(VecModel::from(names))));
        Ok(NewProjectController { window, shown: None })
    }

    /// The dialog the window shows, if it shows one.
    pub fn shown(&self) -> Option<&NewProjectDialog> {
        self.shown.as_ref()
    }

    pub fn sync(&mut self, workbench: &Workbench) {
        let dialog = workbench.new_project_dialog();
        if dialog == self.shown {
            return;
        }
        let Some(dialog) = dialog else {
            self.shown = None;
            if let Err(error) = self.window.hide() {
                eprintln!("genea: couldn't hide the New Project window: {error}");
            }
            return;
        };
        let window = &self.window;
        let opening = self.shown.is_none();
        let index = TEMPLATES.iter().position(|(t, _, _)| *t == dialog.template).unwrap_or(0);
        window.set_template_index(index as i32);
        window.set_template_description(TEMPLATES[index].2.into());
        if window.get_name() != dialog.name.as_str() {
            window.set_name(dialog.name.as_str().into());
        }
        window.set_name_problem(dialog.name_problem.clone().unwrap_or_default().into());
        window.set_parent(dialog.parent.as_deref().map(tilde).unwrap_or_default().into());
        let destination = match &dialog.parent {
            Some(parent) if !dialog.name.is_empty() => tilde(&parent.join(&dialog.name)),
            _ => String::new(),
        };
        window.set_destination(destination.into());
        if self.shown.as_ref().is_none_or(|shown| shown.runtimes != dialog.runtimes) {
            window.set_runtimes(labels(dialog.runtimes.iter().map(|o| (&o.label, &o.detail))));
        }
        let runtime = dialog.runtimes.iter().position(|o| o.pin == dialog.runtime).unwrap_or(0);
        window.set_runtime_index(runtime as i32);
        if self.shown.as_ref().is_none_or(|shown| shown.package_managers != dialog.package_managers) {
            window.set_package_managers(labels(dialog.package_managers.iter().map(|o| (&o.label, &o.detail))));
        }
        let package_manager =
            dialog.package_managers.iter().position(|o| o.pin == dialog.package_manager).unwrap_or(0);
        window.set_package_manager_index(package_manager as i32);
        let versions = if dialog.listing { Some("Listing versions…".to_owned()) } else { dialog.message.clone() };
        window.set_versions_message(versions.unwrap_or_default().into());
        window.set_error(dialog.error.clone().unwrap_or_default().into());
        window.set_creating(dialog.creating);
        self.shown = Some(dialog);
        if opening {
            if let Err(error) = window.show() {
                eprintln!("genea: couldn't show the New Project window: {error}");
            }
            window.invoke_focus_name();
        }
    }
}

/// A picker's rows: `Node 24.21.0 (default)`.
fn labels<'a>(options: impl Iterator<Item = (&'a String, &'a String)>) -> ModelRc<SharedString> {
    let rows: Vec<SharedString> = options
        .map(|(label, detail)| if detail.is_empty() { label.into() } else { format!("{label} ({detail})").into() })
        .collect();
    ModelRc::from(Rc::new(VecModel::from(rows)))
}

/// `path`, with the home folder shown as `~`.
fn tilde(path: &Path) -> String {
    match std::env::home_dir().and_then(|home| path.strip_prefix(home).ok().map(Path::to_owned)) {
        Some(relative) if relative.as_os_str().is_empty() => "~".to_owned(),
        Some(relative) => format!("~/{}", relative.display()),
        None => path.display().to_string(),
    }
}
