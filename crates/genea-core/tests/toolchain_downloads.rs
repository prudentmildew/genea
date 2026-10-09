//! Toolchain downloads (ticket #35, ADR 0005): opening a project downloads
//! the runtime and package manager its root `package.json` pins into the
//! shared store, checksum-verified, without blocking the main thread.
//!
//! Every download comes from the test host's local download fixture server,
//! which publishes fake Node, Bun and pnpm releases.

use std::{
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use genea_core::{ToolState, ToolView, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn project(package_json: &str) -> FixtureProject {
    FixtureProject::new().file("package.json", package_json).build()
}

fn pins(runtime: &str, runtime_version: &str, package_manager: &str) -> String {
    format!(
        r#"{{
  "name": "app",
  "devEngines": {{ "runtime": {{ "name": "{runtime}", "version": "{runtime_version}" }} }},
  "packageManager": "{package_manager}"
}}
"#
    )
}

fn store(host: &TestHost) -> PathBuf {
    host.support_dir().join("toolchains")
}

/// `tool/version` for every version directory in the store.
fn installed(host: &TestHost) -> Vec<String> {
    let mut found = Vec::new();
    for tool in std::fs::read_dir(store(host)).into_iter().flatten().flatten() {
        if tool.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        for version in std::fs::read_dir(tool.path()).unwrap().flatten() {
            found.push(format!("{}/{}", tool.file_name().to_string_lossy(), version.file_name().to_string_lossy()));
        }
    }
    found.sort();
    found
}

fn is_executable(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn ready(tool: &str, version: &str) -> Option<ToolView> {
    Some(ToolView { tool: tool.into(), version: version.into(), state: ToolState::Ready })
}

#[test]
fn opening_a_pinned_project_downloads_exactly_the_pinned_versions() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    host.tools().pnpm("11.13.0");
    let fixture = project(&pins("node", "24.18.0", "pnpm@11.13.0"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let toolchain = workbench.project(project).unwrap().toolchain;
    assert_eq!(toolchain.runtime, ready("Node", "24.18.0"));
    assert_eq!(toolchain.package_manager, ready("pnpm", "11.13.0"));
    assert_eq!(installed(&host), ["node/24.18.0", "pnpm/11.13.0"]);
    assert!(is_executable(&store(&host).join("node/24.18.0/bin/node")));
    // pnpm 11's binary ships without the executable bit; Genea sets it.
    assert!(is_executable(&store(&host).join("pnpm/11.13.0/bin/pnpm")));
}

#[test]
fn a_second_project_with_the_same_pins_reuses_the_store() {
    let host = TestHost::new();
    let node = host.tools().node("24.18.0");
    let pnpm = host.tools().pnpm("12.10.1");
    let first = project(&pins("node", "24.18.0", "pnpm@12.10.1"));
    let second = project(&pins("node", "24.18.0", "pnpm@12.10.1"));
    let mut workbench = Workbench::new(host.shared());
    workbench.open_project(first.root()).unwrap();
    workbench.settle().unwrap();

    let project = workbench.open_project(second.root()).unwrap();
    workbench.settle().unwrap();
    // A later run of Genea on the same machine reuses them too.
    let mut later = Workbench::new(host.shared());
    let reopened = later.open_project(second.root()).unwrap();
    later.settle().unwrap();

    for view in [workbench.project(project).unwrap(), later.project(reopened).unwrap()] {
        assert_eq!(view.toolchain.runtime, ready("Node", "24.18.0"));
        assert_eq!(view.toolchain.package_manager, ready("pnpm", "12.10.1"));
    }
    assert_eq!(host.download_server().request_count(&node), 1);
    assert_eq!(host.download_server().request_count(&pnpm), 1);
}

#[test]
fn a_checksum_mismatch_fails_the_download_with_retry_and_leaves_nothing_in_the_store() {
    let host = TestHost::new();
    host.tools().node_with_bad_checksum("24.18.0");
    host.tools().pnpm("12.10.1");
    let fixture = project(&pins("node", "24.18.0", "pnpm@12.10.1"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(
        view.toolchain.runtime,
        Some(ToolView { tool: "Node".into(), version: "24.18.0".into(), state: ToolState::Failed })
    );
    assert_eq!(view.toolchain.package_manager, ready("pnpm", "12.10.1"));
    assert_eq!(view.notices.len(), 1, "{:?}", view.notices);
    let notice = &view.notices[0];
    assert!(notice.message.starts_with("Couldn't download Node 24.18.0"), "{}", notice.message);
    assert!(notice.message.contains("checksum"), "{}", notice.message);
    let retry = notice.action.as_ref().expect("a Retry action");
    assert_eq!(retry.label, "Retry");
    assert_eq!(view.status.toolchain, None);
    // Nothing of Node is in the store, not even a partial download.
    assert_eq!(installed(&host), ["pnpm/12.10.1"]);
    let leftovers: Vec<_> = std::fs::read_dir(store(&host)).unwrap().flatten().map(|e| e.file_name()).collect();
    assert_eq!(leftovers, ["pnpm"]);

    // The publisher fixes the archive; Retry downloads it.
    host.tools().node("24.18.0");
    workbench.dispatch(project, retry.command.clone());
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.runtime, ready("Node", "24.18.0"));
    assert!(view.notices.is_empty(), "{:?}", view.notices);
    assert_eq!(installed(&host), ["node/24.18.0", "pnpm/12.10.1"]);
}

#[test]
fn a_pnpm_tarball_that_fails_its_integrity_is_refused_too() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().pnpm_with_bad_checksum("12.10.1");
    let fixture = project(&pins("node", "24.18.0", "pnpm@12.10.1"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.package_manager.unwrap().state, ToolState::Failed);
    assert!(view.notices[0].message.starts_with("Couldn't download pnpm 12.10.1"), "{:?}", view.notices);
    assert_eq!(installed(&host), ["node/24.18.0"]);
}

#[test]
fn a_version_that_isnt_published_fails_only_its_role() {
    let host = TestHost::new();
    host.tools().pnpm("12.10.1");
    let fixture = project(&pins("node", "24.18.0", "pnpm@12.10.1"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.runtime.unwrap().state, ToolState::Failed);
    assert_eq!(view.toolchain.package_manager, ready("pnpm", "12.10.1"));
    assert_eq!(view.notices.len(), 1);
    assert_eq!(view.notices[0].action.as_ref().unwrap().label, "Retry");
    assert_eq!(installed(&host), ["pnpm/12.10.1"]);
}
