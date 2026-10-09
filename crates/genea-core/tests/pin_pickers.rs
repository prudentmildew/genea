//! Pin pickers (ticket #37, ADR 0005): "Set runtime…" and "Set package
//! manager…" list the versions there are, and picking one writes it to
//! `package.json` as an exact pin and starts its download.
//!
//! Every version list and download comes from the test host's local download
//! fixture server, which publishes fake Node, Bun and pnpm releases.

use genea_core::{Command, Notice, ProjectId, ToolState, ToolView, ToolchainPicker, ToolchainPickerKind, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// A project with this root `package.json`, its dependencies installed (so
/// the only notices are the toolchain's).
fn project(package_json: &str) -> FixtureProject {
    FixtureProject::new().file("package.json", package_json).dir("node_modules").build()
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
    assert!(notices(&view.notices).is_empty(), "{:?}", view.notices);
}

/// The project's notices without TypeScript 7's: these projects don't have
/// it, and `tests/language_server.rs` covers that notice.
fn notices(notices: &[Notice]) -> Vec<Notice> {
    notices.iter().filter(|n| !n.message.starts_with("Language intelligence is off")).cloned().collect()
}


const NODE_INDEX: &str = "https://nodejs.org/dist/index.json";
const BUN_RELEASES: &str = "https://api.github.com/repos/oven-sh/bun/releases?per_page=100";

#[test]
fn picking_bun_as_package_manager_writes_package_manager_and_downloads_bun() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().pnpm("12.10.1");
    host.tools().pnpm("11.13.0");
    host.tools().bun("1.4.2");
    let fixture = project(PINNED);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let picker = open_picker(&mut workbench, project, ToolchainPickerKind::PackageManager);
    assert_eq!(picker.title, "Set package manager");
    assert_eq!(options(&picker), [("pnpm", "12.10.1", "in use"), ("pnpm", "11.13.0", ""), ("Bun", "1.4.2", "")]);

    pick(&mut workbench, project, &picker, "Bun", "1.4.2");
    workbench.settle().unwrap();

    let package_json = fixture.read("package.json");
    assert!(package_json.contains("\"packageManager\": \"bun@1.4.2\""), "{package_json}");
    assert!(package_json.contains("\"version\": \"24.18.0\""), "the runtime pin stays: {package_json}");
    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.package_manager, ready("Bun", "1.4.2"));
    assert_eq!(view.toolchain.runtime, ready("Node", "24.18.0"));
    assert!(host.support_dir().join("toolchains/bun/1.4.2/bin/bun").is_file());
}

#[test]
fn picking_a_runtime_for_an_unpinned_project_pins_only_the_runtime() {
    let host = TestHost::new();
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    host.tools().bun("1.4.2");
    let fixture = project("{ \"name\": \"app\" }\n");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let picker = open_picker(&mut workbench, project, ToolchainPickerKind::Runtime);
    assert_eq!(options(&picker)[0], ("Node", "24.21.0", "in use"));
    pick(&mut workbench, project, &picker, "Bun", "1.4.2");
    workbench.settle().unwrap();

    assert_eq!(
        fixture.read("package.json"),
        "{\n  \"name\": \"app\",\n  \"devEngines\": {\n    \"runtime\": {\n      \"name\": \"bun\",\n      \"version\": \"1.4.2\"\n    }\n  }\n}\n"
    );
    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.runtime, ready("Bun", "1.4.2"));
    let messages: Vec<String> = notices(&view.notices).into_iter().map(|n| n.message).collect();
    assert_eq!(messages, ["This project doesn't pin its package manager, so Genea uses pnpm 12.10.1."]);
}

#[test]
fn typing_in_the_picker_filters_its_options() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("26.11.1");
    host.tools().pnpm("12.10.1");
    host.tools().bun("1.4.2");
    let fixture = project(PINNED);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    open_picker(&mut workbench, project, ToolchainPickerKind::Runtime);

    workbench.dispatch(project, Command::FilterToolchainPicker("bun".into()));
    let picker = workbench.project(project).unwrap().toolchain_picker.unwrap();
    assert_eq!(picker.query, "bun");
    assert_eq!(options(&picker), [("Bun", "1.4.2", "")]);

    workbench.dispatch(project, Command::FilterToolchainPicker("26".into()));
    let picker = workbench.project(project).unwrap().toolchain_picker.unwrap();
    assert_eq!(options(&picker), [("Node", "26.11.1", "")]);
}

#[test]
fn closing_the_picker_changes_nothing() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("26.11.1");
    host.tools().pnpm("12.10.1");
    let fixture = project(PINNED);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    open_picker(&mut workbench, project, ToolchainPickerKind::Runtime);

    workbench.dispatch(project, Command::CloseToolchainPicker);
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain_picker, None);
    assert_eq!(view.toolchain.runtime, ready("Node", "24.18.0"));
    assert_eq!(fixture.read("package.json"), PINNED);
}

#[test]
fn the_picker_shows_loading_until_the_lists_arrive() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().pnpm("12.10.1");
    let fixture = project(PINNED);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    host.download_server().hold(NODE_INDEX);

    workbench.dispatch(project, Command::OpenToolchainPicker(ToolchainPickerKind::Runtime));
    host.download_server().wait_held(NODE_INDEX);
    workbench.pump();
    let picker = workbench.project(project).unwrap().toolchain_picker.unwrap();
    assert!(picker.loading);
    assert!(picker.options.is_empty());

    host.download_server().release(NODE_INDEX);
    workbench.settle().unwrap();
    assert!(!workbench.project(project).unwrap().toolchain_picker.unwrap().loading);
}

#[test]
fn offline_the_picker_still_offers_the_downloaded_versions_and_says_why_the_rest_are_missing() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().pnpm("12.10.1");
    let fixture = project(PINNED);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    // The publishers can't be reached any more.
    host.downloads().fail(NODE_INDEX, 503);
    host.downloads().fail(BUN_RELEASES, 503);

    let picker = open_picker(&mut workbench, project, ToolchainPickerKind::Runtime);

    assert_eq!(options(&picker), [("Node", "24.18.0", "in use")]);
    let message = picker.message.expect("a message");
    assert!(message.starts_with("Couldn't list the Node versions"), "{message}");
    assert!(message.contains("Couldn't list the Bun versions"), "{message}");
}

#[test]
fn a_folder_without_package_json_has_no_picker_and_says_why() {
    let host = TestHost::new();
    let fixture = FixtureProject::new().file("main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenToolchainPicker(ToolchainPickerKind::Runtime));
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain_picker, None);
    assert_eq!(view.notices.len(), 1);
    assert!(view.notices[0].message.contains("package.json"), "{:?}", view.notices);
    assert!(!fixture.path("package.json").exists());
}
