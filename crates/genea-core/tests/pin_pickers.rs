//! Pin pickers (ticket #37, ADR 0005): "Set runtime…" and "Set package
//! manager…" list the versions there are, and picking one writes it to
//! `package.json` as an exact pin and starts its download.
//!
//! Every version list and download comes from the test host's local download
//! fixture server, which publishes fake Node, Bun and pnpm releases.

use genea_core::{Command, ProjectId, ToolState, ToolView, ToolchainPicker, ToolchainPickerKind, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn project(package_json: &str) -> FixtureProject {
    FixtureProject::new().file("package.json", package_json).build()
}

const PINNED: &str = r#"{
  "name": "app",
  "devEngines": { "runtime": { "name": "node", "version": "24.18.0" } },
  "packageManager": "pnpm@12.10.1",
  "scripts": { "dev": "vite" }
}
"#;

fn ready(tool: &str, version: &str) -> Option<ToolView> {
    Some(ToolView { tool: tool.into(), version: version.into(), state: ToolState::Ready })
}

/// `(tool, version, detail)` for every option the picker shows.
fn options(picker: &ToolchainPicker) -> Vec<(&str, &str, &str)> {
    picker.options.iter().map(|o| (o.tool.as_str(), o.version.as_str(), o.detail.as_str())).collect()
}

fn open_picker(workbench: &mut Workbench, project: ProjectId, kind: ToolchainPickerKind) -> ToolchainPicker {
    workbench.dispatch(project, Command::OpenToolchainPicker(kind));
    workbench.settle().unwrap();
    workbench.project(project).unwrap().toolchain_picker.expect("the picker is open")
}

/// Dispatches the command of the option for `tool` `version`.
fn pick(workbench: &mut Workbench, project: ProjectId, picker: &ToolchainPicker, tool: &str, version: &str) {
    let option = picker
        .options
        .iter()
        .find(|o| o.tool == tool && o.version == version)
        .unwrap_or_else(|| panic!("no {tool} {version} in {:?}", options(picker)));
    workbench.dispatch(project, option.command.clone());
}

#[test]
fn set_runtime_lists_every_published_node_and_bun_version_newest_first() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("26.11.1");
    host.tools().node("22.20.0");
    host.tools().bun("1.4.2");
    host.tools().bun("1.3.0");
    host.tools().pnpm("12.10.1");
    let fixture = project(PINNED);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let picker = open_picker(&mut workbench, project, ToolchainPickerKind::Runtime);

    assert_eq!(picker.title, "Set runtime");
    assert!(!picker.loading);
    assert_eq!(picker.message, None);
    assert_eq!(
        options(&picker),
        [
            ("Node", "26.11.1", ""),
            ("Node", "24.18.0", "in use"),
            ("Node", "22.20.0", ""),
            ("Bun", "1.4.2", ""),
            ("Bun", "1.3.0", ""),
        ]
    );
    // Listing versions writes nothing.
    assert_eq!(fixture.read("package.json"), PINNED);
}

#[test]
fn picking_a_runtime_writes_an_exact_pin_and_downloads_it() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("26.11.1");
    host.tools().pnpm("12.10.1");
    let fixture = project(PINNED);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    let picker = open_picker(&mut workbench, project, ToolchainPickerKind::Runtime);

    pick(&mut workbench, project, &picker, "Node", "26.11.1");
    workbench.settle().unwrap();

    assert_eq!(
        fixture.read("package.json"),
        r#"{
  "name": "app",
  "devEngines": {
    "runtime": {
      "name": "node",
      "version": "26.11.1"
    }
  },
  "packageManager": "pnpm@12.10.1",
  "scripts": {
    "dev": "vite"
  }
}
"#
    );
    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain_picker, None, "picking closes the picker");
    assert_eq!(view.toolchain.runtime, ready("Node", "26.11.1"));
    assert_eq!(view.toolchain.package_manager, ready("pnpm", "12.10.1"));
    assert!(host.support_dir().join("toolchains/node/26.11.1/bin/node").is_file());
    assert!(view.notices.is_empty(), "{:?}", view.notices);
}
