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
fn a_bun_project_downloads_bun_once_for_both_roles() {
    let host = TestHost::new();
    let bun = host.tools().bun("1.4.2");
    host.tools().bun("1.3.0");
    let fixture = project(&pins("bun", "1.4.2", "bun@1.4.2"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let toolchain = workbench.project(project).unwrap().toolchain;
    assert_eq!(toolchain.runtime, ready("Bun", "1.4.2"));
    assert_eq!(toolchain.package_manager, ready("Bun", "1.4.2"));
    assert_eq!(installed(&host), ["bun/1.4.2"]);
    assert!(is_executable(&store(&host).join("bun/1.4.2/bin/bun")));
    assert_eq!(host.download_server().request_count(&bun), 1);
}

#[test]
fn a_bun_archive_that_fails_its_checksum_is_refused() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().bun_with_bad_checksum("1.4.2");
    let fixture = project(&pins("node", "24.18.0", "bun@1.4.2"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.package_manager.unwrap().state, ToolState::Failed);
    assert!(view.notices[0].message.starts_with("Couldn't download Bun 1.4.2"), "{:?}", view.notices);
    assert_eq!(installed(&host), ["node/24.18.0"]);
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

// --- Range pins --------------------------------------------------------------

#[test]
fn a_range_pin_prefers_the_newest_matching_version_in_the_store() {
    let host = TestHost::new();
    for version in ["22.23.3", "24.18.0", "24.21.0", "26.11.1"] {
        host.tools().node(version);
    }
    host.tools().pnpm("12.10.1");
    let mut workbench = Workbench::new(host.shared());
    let exact = project(&pins("node", "24.18.0", "pnpm@12.10.1"));
    workbench.open_project(exact.root()).unwrap();
    workbench.settle().unwrap();
    let requests_before = host.download_server().requests().len();

    let ranged = project(&pins("node", "^24", "pnpm@12.10.1"));
    let project = workbench.open_project(ranged.root()).unwrap();
    workbench.settle().unwrap();

    // 24.21.0 is published, but 24.18.0 is already here.
    assert_eq!(workbench.project(project).unwrap().toolchain.runtime, ready("Node", "24.18.0"));
    assert_eq!(installed(&host), ["node/24.18.0", "pnpm/12.10.1"]);
    assert_eq!(host.download_server().requests().len(), requests_before, "no network needed");
}

#[test]
fn a_range_pin_downloads_the_newest_matching_version_when_the_store_has_none() {
    let host = TestHost::new();
    for version in ["22.23.3", "24.18.0", "24.21.0", "26.11.1"] {
        host.tools().node(version);
    }
    for version in ["11.13.0", "11.26.0", "12.10.1"] {
        host.tools().pnpm(version);
    }
    let fixture = project(&pins("node", ">=22 <25", "pnpm@11.x"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let toolchain = workbench.project(project).unwrap().toolchain;
    assert_eq!(toolchain.runtime, ready("Node", "24.21.0"));
    assert_eq!(toolchain.package_manager, ready("pnpm", "11.26.0"));
    assert_eq!(installed(&host), ["node/24.21.0", "pnpm/11.26.0"]);
}

#[test]
fn a_bun_range_resolves_against_bun_releases() {
    let host = TestHost::new();
    for version in ["1.3.0", "1.4.2", "2.0.0"] {
        host.tools().bun(version);
    }
    let fixture = project(&pins("bun", "~1.4.0", "bun@^1.3.0"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let toolchain = workbench.project(project).unwrap().toolchain;
    assert_eq!(toolchain.runtime, ready("Bun", "1.4.2"));
    assert_eq!(toolchain.package_manager, ready("Bun", "1.4.2"));
}

#[test]
fn a_range_nothing_matches_fails_with_a_notice() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().pnpm("12.10.1");
    let fixture = project(&pins("node", "^30", "pnpm@12.10.1"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(
        view.toolchain.runtime,
        Some(ToolView { tool: "Node".into(), version: "^30".into(), state: ToolState::Failed })
    );
    assert!(view.notices[0].message.contains("no published Node version matches ^30"), "{:?}", view.notices);
    assert_eq!(installed(&host), ["pnpm/12.10.1"]);
}

// --- Unpinned projects -------------------------------------------------------

/// Genea's built-in defaults as of this release: Node's active LTS and the
/// latest pnpm (research digest, 2026-10-09).
const DEFAULT_NODE: &str = "24.21.0";
const DEFAULT_PNPM: &str = "12.10.1";

#[test]
fn an_unpinned_project_gets_the_defaults_and_a_notice_offering_to_pin_them() {
    let host = TestHost::new();
    host.tools().node(DEFAULT_NODE);
    host.tools().node("26.11.1");
    host.tools().pnpm(DEFAULT_PNPM);
    let package_json = "{\n  \"name\": \"app\"\n}\n";
    let fixture = project(package_json);
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.runtime, ready("Node", DEFAULT_NODE));
    assert_eq!(view.toolchain.package_manager, ready("pnpm", DEFAULT_PNPM));
    assert_eq!(view.notices.len(), 1, "{:?}", view.notices);
    assert_eq!(
        view.notices[0].message,
        "This project doesn't pin its runtime or package manager, so Genea uses Node 24.21.0 and pnpm 12.10.1."
    );
    // Genea never writes package.json without a click.
    assert_eq!(fixture.read("package.json"), package_json);

    let pin = view.notices[0].action.clone().expect("an action that pins the defaults");
    assert_eq!(pin.label, "Pin these versions");
    workbench.dispatch(project, pin.command);
    workbench.settle().unwrap();

    assert_eq!(
        fixture.read("package.json"),
        r#"{
  "name": "app",
  "devEngines": {
    "runtime": {
      "name": "node",
      "version": "24.21.0"
    }
  },
  "packageManager": "pnpm@12.10.1"
}
"#
    );
    let view = workbench.project(project).unwrap();
    assert!(view.notices.is_empty(), "{:?}", view.notices);
    assert_eq!(view.toolchain.runtime, ready("Node", DEFAULT_NODE));
    assert_eq!(installed(&host), ["node/24.21.0", "pnpm/12.10.1"]);
}

#[test]
fn pinning_the_defaults_writes_only_the_unpinned_role_and_keeps_the_rest_of_the_file() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    host.tools().node(DEFAULT_NODE);
    host.tools().pnpm(DEFAULT_PNPM);
    let fixture = project(
        "{\n\t\"name\": \"app\",\n\t\"devEngines\": { \"runtime\": { \"name\": \"node\", \"version\": \"^24.0.0\" } },\n\t\"scripts\": { \"dev\": \"vite\" }\n}",
    );
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    let view = workbench.project(project).unwrap();
    assert_eq!(view.notices.len(), 1, "{:?}", view.notices);
    assert_eq!(
        view.notices[0].message,
        "This project doesn't pin its package manager, so Genea uses pnpm 12.10.1."
    );

    workbench.dispatch(project, view.notices[0].action.clone().unwrap().command);
    workbench.settle().unwrap();

    // The range pin stays as written (Genea never moves a pin by itself),
    // and the file keeps its tab indentation and its missing final newline.
    let expected = [
        "{",
        "\t\"name\": \"app\",",
        "\t\"devEngines\": {",
        "\t\t\"runtime\": {",
        "\t\t\t\"name\": \"node\",",
        "\t\t\t\"version\": \"^24.0.0\"",
        "\t\t}",
        "\t},",
        "\t\"scripts\": {",
        "\t\t\"dev\": \"vite\"",
        "\t},",
        "\t\"packageManager\": \"pnpm@12.10.1\"",
        "}",
    ];
    assert_eq!(fixture.read("package.json"), expected.join("\n"));
    assert!(workbench.project(project).unwrap().notices.is_empty());
}

#[test]
fn a_folder_without_package_json_has_no_toolchain() {
    let host = TestHost::new();
    let fixture = FixtureProject::new().file("main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.runtime, None);
    assert_eq!(view.toolchain.package_manager, None);
    assert!(view.notices.is_empty());
    assert!(host.downloads().requests().is_empty());
}

#[test]
fn a_foreign_package_manager_is_off_and_nothing_is_downloaded_for_it() {
    let host = TestHost::new();
    host.tools().node("24.18.0");
    let fixture = project(&pins("node", "24.18.0", "npm@11.0.0"));
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.toolchain.runtime, ready("Node", "24.18.0"));
    assert_eq!(view.toolchain.package_manager.unwrap().state, ToolState::Off);
    assert_eq!(installed(&host), ["node/24.18.0"]);
}
