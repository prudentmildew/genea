//! "Update toolchain…" (ticket #37, ADR 0005): lists the versions newer than
//! the ones in use, and rewrites a pin only when the user picks one. Genea
//! never moves a pin by itself.

use genea_core::{Command, ProjectId, ToolState, ToolView, ToolchainPicker, ToolchainPickerKind, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn project(package_json: &str) -> FixtureProject {
    FixtureProject::new().file("package.json", package_json).build()
}

fn pins(runtime_version: &str, package_manager: &str) -> String {
    format!(
        "{{\n  \"name\": \"app\",\n  \"devEngines\": {{ \"runtime\": {{ \"name\": \"node\", \"version\": \"{runtime_version}\" }} }},\n  \"packageManager\": \"{package_manager}\"\n}}\n"
    )
}

fn ready(tool: &str, version: &str) -> Option<ToolView> {
    Some(ToolView { tool: tool.into(), version: version.into(), state: ToolState::Ready })
}

fn options(picker: &ToolchainPicker) -> Vec<(&str, &str, &str)> {
    picker.options.iter().map(|o| (o.tool.as_str(), o.version.as_str(), o.detail.as_str())).collect()
}

fn open_update(workbench: &mut Workbench, project: ProjectId) -> ToolchainPicker {
    workbench.dispatch(project, Command::OpenToolchainPicker(ToolchainPickerKind::Update));
    workbench.settle().unwrap();
    workbench.project(project).unwrap().toolchain_picker.expect("the picker is open")
}

fn pick(workbench: &mut Workbench, project: ProjectId, picker: &ToolchainPicker, tool: &str, version: &str) {
    let option = picker.options.iter().find(|o| o.tool == tool && o.version == version).expect("the option");
    workbench.dispatch(project, option.command.clone());
}

#[test]
fn update_toolchain_lists_the_newer_versions_of_each_role() {
    let host = TestHost::new();
    for version in ["22.20.0", "24.18.0", "24.21.0", "26.11.1"] {
        host.tools().node(version);
    }
    host.tools().pnpm("11.13.0");
    host.tools().pnpm("12.10.1");
    host.tools().bun("1.4.2");
    let package_json = pins("24.18.0", "pnpm@11.13.0");
    let fixture = project(&package_json);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let picker = open_update(&mut workbench, project);

    assert_eq!(picker.title, "Update toolchain");
    assert_eq!(picker.message, None);
    assert_eq!(
        options(&picker),
        [
            ("Node", "26.11.1", "runtime, now 24.18.0"),
            ("Node", "24.21.0", "runtime, now 24.18.0"),
            ("pnpm", "12.10.1", "package manager, now 11.13.0"),
        ]
    );
    // Newer versions exist, but nothing moves until one is picked.
    assert_eq!(fixture.read("package.json"), package_json);
}

#[test]
fn picking_an_update_rewrites_only_that_pin_and_downloads_it() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("26.11.1");
    host.tools().pnpm("11.13.0");
    let pnpm = host.tools().pnpm("12.10.1");
    let fixture = project(&pins("24.18.0", "pnpm@11.13.0"));
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    assert_eq!(host.download_server().request_count(&pnpm), 0);
    let picker = open_update(&mut workbench, project);

    pick(&mut workbench, project, &picker, "pnpm", "12.10.1");
    workbench.settle().unwrap();

    assert_eq!(
        fixture.read("package.json"),
        "{\n  \"name\": \"app\",\n  \"devEngines\": {\n    \"runtime\": {\n      \"name\": \"node\",\n      \"version\": \"24.18.0\"\n    }\n  },\n  \"packageManager\": \"pnpm@12.10.1\"\n}\n"
    );
    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain_picker, None);
    assert_eq!(view.toolchain.runtime, ready("Node", "24.18.0"));
    assert_eq!(view.toolchain.package_manager, ready("pnpm", "12.10.1"));
    assert_eq!(host.download_server().request_count(&pnpm), 1);
}

#[test]
fn a_range_pin_is_compared_by_the_version_it_resolves_to_and_stays_as_written_until_a_pick() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("24.21.0");
    host.tools().node("26.11.1");
    host.tools().pnpm("12.10.1");
    let package_json = pins("^24.0.0", "pnpm@12.10.1");
    let fixture = project(&package_json);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    assert_eq!(workbench.project(project).unwrap().toolchain.runtime, ready("Node", "24.21.0"));

    let picker = open_update(&mut workbench, project);
    assert_eq!(options(&picker), [("Node", "26.11.1", "runtime, now 24.21.0")]);
    workbench.dispatch(project, Command::CloseToolchainPicker);
    workbench.settle().unwrap();
    assert_eq!(fixture.read("package.json"), package_json, "a range pin is never moved by itself");

    let picker = open_update(&mut workbench, project);
    pick(&mut workbench, project, &picker, "Node", "26.11.1");
    workbench.settle().unwrap();

    assert!(fixture.read("package.json").contains("\"version\": \"26.11.1\""), "{}", fixture.read("package.json"));
    assert_eq!(workbench.project(project).unwrap().toolchain.runtime, ready("Node", "26.11.1"));
}

#[test]
fn an_up_to_date_toolchain_says_so() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    let fixture = project(&pins("24.21.0", "pnpm@12.10.1"));
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let picker = open_update(&mut workbench, project);

    assert!(picker.options.is_empty());
    assert_eq!(picker.message.as_deref(), Some("Node 24.21.0 and pnpm 12.10.1 are up to date."));
}

#[test]
fn a_bun_project_is_offered_newer_bun_for_both_roles() {
    let host = TestHost::new();
    host.tools().bun("1.4.2");
    host.tools().bun("1.5.0");
    let fixture = project(
        "{ \"devEngines\": { \"runtime\": { \"name\": \"bun\", \"version\": \"1.4.2\" } }, \"packageManager\": \"bun@1.4.2\" }\n",
    );
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let picker = open_update(&mut workbench, project);
    assert_eq!(
        options(&picker),
        [("Bun", "1.5.0", "runtime, now 1.4.2"), ("Bun", "1.5.0", "package manager, now 1.4.2")]
    );

    // Updating the runtime leaves the package manager's pin alone.
    workbench.dispatch(project, picker.options[0].command.clone());
    workbench.settle().unwrap();
    let package_json = fixture.read("package.json");
    assert!(package_json.contains("\"version\": \"1.5.0\""), "{package_json}");
    assert!(package_json.contains("\"packageManager\": \"bun@1.4.2\""), "{package_json}");
    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.runtime, ready("Bun", "1.5.0"));
    assert_eq!(view.toolchain.package_manager, ready("Bun", "1.4.2"));
}
