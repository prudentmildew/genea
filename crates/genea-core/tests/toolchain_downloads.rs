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
