//! "Remove unused toolchains" (ticket #37, ADR 0005): deletes the store
//! versions that no recently opened project uses, and keeps every version a
//! recent (or open) project pins.

use std::time::Duration;

use genea_core::{Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn project(package_json: &str) -> FixtureProject {
    FixtureProject::new().file("package.json", package_json).build()
}

fn pins(runtime: &str, runtime_version: &str, package_manager: &str) -> String {
    format!(
        "{{ \"devEngines\": {{ \"runtime\": {{ \"name\": \"{runtime}\", \"version\": \"{runtime_version}\" }} }}, \"packageManager\": \"{package_manager}\" }}\n"
    )
}

/// `tool/version` for every version folder in the store.
fn installed(host: &TestHost) -> Vec<String> {
    let mut found = Vec::new();
    for tool in std::fs::read_dir(host.support_dir().join("toolchains")).into_iter().flatten().flatten() {
        for version in std::fs::read_dir(tool.path()).into_iter().flatten().flatten() {
            let name = version.file_name().to_string_lossy().into_owned();
            found.push(format!("{}/{name}", tool.file_name().to_string_lossy()));
        }
    }
    found.sort();
    found
}

/// Opens a project, lets its downloads finish, and returns its id.
fn open(workbench: &mut Workbench, fixture: &FixtureProject) -> ProjectId {
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    project
}

/// Runs the command and returns the notice it adds.
fn remove_unused(workbench: &mut Workbench, project: ProjectId) -> String {
    let before = workbench.project(project).unwrap().notices;
    workbench.dispatch(project, Command::RemoveUnusedToolchains);
    workbench.settle().unwrap();
    let after = workbench.project(project).unwrap().notices;
    let new: Vec<_> = after.into_iter().filter(|n| !before.contains(n)).collect();
    assert_eq!(new.len(), 1, "one notice says what happened: {new:?}");
    new[0].message.clone()
}

fn publish_tools(host: &TestHost) {
    for version in ["22.18.0", "22.20.0", "24.18.0", "24.21.0", "26.11.1"] {
        host.tools().node(version);
    }
    host.tools().pnpm("11.13.0");
    host.tools().pnpm("12.10.1");
    host.tools().bun("1.4.2");
}

#[test]
fn remove_unused_toolchains_keeps_every_version_a_recent_project_pins() {
    let host = TestHost::new();
    publish_tools(&host);
    let mut workbench = Workbench::new(host.shared());
    // A closed recent project with exact pins.
    let old = project(&pins("node", "22.20.0", "pnpm@11.13.0"));
    let id = open(&mut workbench, &old);
    workbench.close_project(id);
    // A closed recent project whose pin was moved on disk since: what it
    // used to pin is unused now.
    let moved = project(&pins("node", "24.18.0", "pnpm@11.13.0"));
    let id = open(&mut workbench, &moved);
    workbench.close_project(id);
    moved.write("package.json", pins("node", "26.11.1", "pnpm@11.13.0"));
    // An open project that switched its runtime from Bun to Node.
    let switched = project(&pins("bun", "1.4.2", "pnpm@12.10.1"));
    let switched_id = open(&mut workbench, &switched);
    workbench.dispatch(switched_id, Command::SetRuntime(genea_core::RuntimePin::Node("26.11.1".into())));
    workbench.settle().unwrap();
    // An open project with no pins uses Genea's defaults.
    let unpinned = project("{ \"name\": \"app\" }\n");
    let unpinned_id = open(&mut workbench, &unpinned);
    assert_eq!(
        installed(&host),
        [
            "bun/1.4.2",
            "node/22.20.0",
            "node/24.18.0",
            "node/24.21.0",
            "node/26.11.1",
            "pnpm/11.13.0",
            "pnpm/12.10.1"
        ]
    );

    let message = remove_unused(&mut workbench, unpinned_id);

    assert_eq!(message, "Removed 2 unused toolchain versions: Node 24.18.0, Bun 1.4.2.");
    assert_eq!(
        installed(&host),
        ["node/22.20.0", "node/24.21.0", "node/26.11.1", "pnpm/11.13.0", "pnpm/12.10.1"]
    );
    // The open projects still run.
    for id in [switched_id, unpinned_id] {
        let view = workbench.project(id).unwrap();
        assert_eq!(view.toolchain.runtime.unwrap().state, genea_core::ToolState::Ready);
    }
}

#[test]
fn a_range_pin_keeps_the_version_it_resolves_to() {
    let host = TestHost::new();
    publish_tools(&host);
    let mut workbench = Workbench::new(host.shared());
    let first = project(&pins("node", "22.18.0", "pnpm@12.10.1"));
    let id = open(&mut workbench, &first);
    workbench.close_project(id);
    first.write("package.json", pins("node", "^22.0.0", "pnpm@12.10.1"));
    let second = project(&pins("node", "22.20.0", "pnpm@12.10.1"));
    let id = open(&mut workbench, &second);
    workbench.close_project(id);
    second.write("package.json", pins("node", "^22.0.0", "pnpm@12.10.1"));
    let ranged = open(&mut workbench, &first);

    let message = remove_unused(&mut workbench, ranged);

    // ^22.0.0 resolves to the newest matching version in the store.
    assert_eq!(message, "Removed 1 unused toolchain version: Node 22.18.0.");
    assert_eq!(installed(&host), ["node/22.20.0", "pnpm/12.10.1"]);
}

#[test]
fn recent_projects_from_an_earlier_run_keep_their_versions() {
    let host = TestHost::new();
    publish_tools(&host);
    let pinned = project(&pins("node", "22.20.0", "pnpm@11.13.0"));
    {
        let mut earlier = Workbench::new(host.shared());
        open(&mut earlier, &pinned);
        // The recent projects are saved a moment after a change.
        host.clock().advance(Duration::from_secs(2));
        earlier.settle().unwrap();
    }
    let mut workbench = Workbench::new(host.shared());
    let other = project(&pins("node", "24.21.0", "pnpm@12.10.1"));
    let id = open(&mut workbench, &other);

    let message = remove_unused(&mut workbench, id);

    assert_eq!(message, "There are no unused toolchain versions to remove.");
    assert_eq!(installed(&host), ["node/22.20.0", "node/24.21.0", "pnpm/11.13.0", "pnpm/12.10.1"]);
}

#[test]
fn a_project_that_fell_off_the_recent_list_no_longer_keeps_its_versions() {
    let host = TestHost::new();
    publish_tools(&host);
    let mut workbench = Workbench::new(host.shared());
    let forgotten = project(&pins("node", "22.18.0", "pnpm@12.10.1"));
    let id = open(&mut workbench, &forgotten);
    workbench.close_project(id);
    // Ten newer projects push it off the recent list.
    let newer: Vec<FixtureProject> = (0..10).map(|_| project(&pins("node", "24.21.0", "pnpm@12.10.1"))).collect();
    let mut last = None;
    for fixture in &newer {
        last = Some(open(&mut workbench, fixture));
    }

    let message = remove_unused(&mut workbench, last.unwrap());

    assert_eq!(message, "Removed 1 unused toolchain version: Node 22.18.0.");
    assert_eq!(installed(&host), ["node/24.21.0", "pnpm/12.10.1"]);
}
