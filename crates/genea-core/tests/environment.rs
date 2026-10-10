//! The project environment (ticket #36, ADR 0005): every process Genea
//! starts for a project gets the variables of the user's login shell,
//! captured once per open in the project root, with the pinned runtime and
//! package manager first on PATH.
//!
//! The login shell is scripted through the test host: `script_shell` plays
//! it with a real `/bin/sh` whose environment is what the user's rc files
//! would export. Processes are started through `Workbench::spawn`, the way
//! the terminal, scripts and language servers start theirs.

use std::{io::Read, path::Path, time::Duration};

use genea_core::{Command, LOGIN_SHELL_TIMEOUT, ProcessSpec, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// A host whose launch environment names `/bin/zsh` as the login shell,
/// with a `printenv` that prints the environment it was given.
fn host() -> TestHost {
    let host = TestHost::new();
    host.set_launch_environment([
        ("SHELL", "/bin/zsh"),
        ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
        ("FROM_LAUNCH", "launch"),
    ]);
    host.processes().script("printenv", |spec, mut io| {
        use std::io::Write;
        for (key, value) in &spec.env {
            writeln!(io.stdout, "{}={}", key.to_string_lossy(), value.to_string_lossy()).unwrap();
        }
        0
    });
    host
}

/// What a process started for the project sees: its environment, as
/// `KEY=value` lines (a later line wins, as in the real environment).
fn printenv(workbench: &Workbench, project: ProjectId) -> Vec<(String, String)> {
    let mut child = workbench.spawn(project, ProcessSpec::new("printenv")).unwrap();
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    child.control.wait().unwrap();
    let mut vars: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let (key, value) = line.split_once('=').unwrap();
        vars.retain(|(k, _)| k != key);
        vars.push((key.to_owned(), value.to_owned()));
    }
    vars
}

fn var(vars: &[(String, String)], key: &str) -> Option<String> {
    vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[test]
fn a_process_sees_the_variables_of_the_login_shell() {
    let host = host();
    host.processes().script_shell("zsh", [("PATH", "/opt/mine/bin:/usr/bin:/bin"), ("GREETING", "hello")]);
    let fixture = FixtureProject::new().file("src/main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let vars = printenv(&workbench, project);
    assert_eq!(var(&vars, "GREETING").as_deref(), Some("hello"));
    assert_eq!(var(&vars, "PATH").as_deref(), Some("/opt/mine/bin:/usr/bin:/bin"));
    // The shell's environment replaces the one Genea was started with.
    assert_eq!(var(&vars, "FROM_LAUNCH"), None);
}

#[test]
fn a_login_shell_that_hangs_times_out_to_the_launch_environment_with_a_notice() {
    let host = host();
    host.processes().script("zsh", |_, io| {
        while !io.killed() {
            std::thread::sleep(Duration::from_millis(1));
        }
        1
    });
    let fixture = FixtureProject::new().file("src/main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    host.clock().advance(LOGIN_SHELL_TIMEOUT);
    workbench.settle().unwrap();

    let notices = workbench.project(project).unwrap().notices;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(
        notices[0].message,
        "Your login shell (/bin/zsh) didn't finish within 10 s, so processes get the environment Genea was started with."
    );
    let vars = printenv(&workbench, project);
    assert_eq!(var(&vars, "FROM_LAUNCH").as_deref(), Some("launch"));
}

#[test]
fn reload_environment_picks_up_a_changed_variable() {
    let host = host();
    host.processes().script_shell("zsh", [("API_URL", "http://old.test")]);
    let fixture = FixtureProject::new().file("src/main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    // The user edits their rc file.
    host.processes().script_shell("zsh", [("API_URL", "http://new.test")]);
    assert_eq!(var(&printenv(&workbench, project), "API_URL").as_deref(), Some("http://old.test"));
    workbench.dispatch(project, Command::ReloadEnvironment);
    workbench.settle().unwrap();

    assert_eq!(var(&printenv(&workbench, project), "API_URL").as_deref(), Some("http://new.test"));
}

#[test]
fn the_fallback_notice_offers_reload_and_goes_once_the_shell_answers() {
    let host = host();
    host.processes().script("zsh", |_, mut io| {
        use std::io::Write;
        writeln!(io.stderr, "zsh: command not found: brew").unwrap();
        1
    });
    let fixture = FixtureProject::new().file("src/main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let notices = workbench.project(project).unwrap().notices;
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert_eq!(
        notices[0].message,
        "Couldn't read the environment of your login shell (/bin/zsh): it exited with code 1: \
         zsh: command not found: brew. Processes get the environment Genea was started with."
    );
    let action = notices[0].action.clone().expect("a Reload environment action");
    assert_eq!(action.label, "Reload environment");

    host.processes().script_shell("zsh", [("GREETING", "hello")]);
    workbench.dispatch(project, action.command);
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project).unwrap().notices, []);
    assert_eq!(var(&printenv(&workbench, project), "GREETING").as_deref(), Some("hello"));
}

#[test]
fn the_login_shell_runs_once_per_open_in_the_project_root() {
    let host = host();
    host.processes().script_shell("zsh", [("GREETING", "hello")]);
    let fixture = FixtureProject::new().file("src/main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    printenv(&workbench, project);
    printenv(&workbench, project);

    let shells: Vec<ProcessSpec> =
        host.processes().spawned().into_iter().filter(|spec| spec.program_name() == "zsh").collect();
    assert_eq!(shells.len(), 1, "{shells:?}");
    assert_eq!(shells[0].program, Path::new("/bin/zsh"));
    assert_eq!(shells[0].cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().as_path()));
}

/// A project that pins Node 24.18.0 and pnpm 11.13.0, both published on the
/// test host's download server.
fn pinned_project(host: &TestHost) -> FixtureProject {
    host.tools().node("24.18.0");
    host.tools().pnpm("11.13.0");
    let package_json = r#"{
  "name": "app",
  "devEngines": { "runtime": { "name": "node", "version": "24.18.0" } },
  "packageManager": "pnpm@11.13.0"
}
"#;
    FixtureProject::new().file("package.json", package_json).build()
}

/// Runs `tool` from a PATH entry and returns what it prints: the fake tools
/// print their version.
fn run_from(dir: &str, tool: &str) -> String {
    let path = Path::new(dir).join(tool);
    let output = std::process::Command::new(&path).output().unwrap_or_else(|e| panic!("run {}: {e}", path.display()));
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
fn the_pinned_runtime_and_package_manager_come_first_on_path() {
    let host = host();
    host.processes().script_shell("zsh", [("PATH", "/opt/mine/bin:/usr/bin:/bin")]);
    let fixture = pinned_project(&host);
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let path = var(&printenv(&workbench, project), "PATH").unwrap();
    let entries: Vec<&str> = path.split(':').collect();
    assert_eq!(entries.len(), 5, "{path}");
    assert_eq!(run_from(entries[0], "node"), "v24.18.0");
    assert_eq!(run_from(entries[1], "pnpm"), "11.13.0");
    assert_eq!(entries[2..], ["/opt/mine/bin", "/usr/bin", "/bin"]);
}
