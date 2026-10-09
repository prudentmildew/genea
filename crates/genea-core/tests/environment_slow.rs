//! The project environment's slow lane (ticket #36): the capture runs this
//! machine's real login shell (`$SHELL -l -i`), with its rc files.
//!
//! Opt-in, because it depends on the user's shell setup:
//!
//! ```sh
//! cargo test -p genea-core --test environment_slow -- --ignored
//! ```

use std::{
    ffi::OsString,
    io::Read,
    path::Path,
};

use genea_core::{ProcessSpec, Workbench};
use genea_host::{Clipboard, Clock, Downloads, Host, Processes, RealHost};
use genea_testkit::FixtureProject;

/// The real host, but with its own support folder so the test leaves the
/// user's recent projects alone, and a marker in the launch environment.
struct RealShellHost {
    real: RealHost,
    support: tempfile::TempDir,
}

impl Host for RealShellHost {
    fn clock(&self) -> &dyn Clock {
        self.real.clock()
    }

    fn processes(&self) -> &dyn Processes {
        self.real.processes()
    }

    fn downloads(&self) -> &dyn Downloads {
        self.real.downloads()
    }

    fn clipboard(&self) -> &dyn Clipboard {
        self.real.clipboard()
    }

    fn support_dir(&self) -> &Path {
        self.support.path()
    }

    fn launch_environment(&self) -> Vec<(OsString, OsString)> {
        let mut vars = self.real.launch_environment();
        vars.push(("GENEA_LAUNCH_MARKER".into(), "passed through the shell".into()));
        vars
    }
}

#[test]
#[ignore = "runs the real login shell; opt in with --ignored"]
fn the_real_login_shell_answers_with_its_environment() {
    let host = RealShellHost { real: RealHost::new(), support: tempfile::tempdir().unwrap() };
    let fixture = FixtureProject::new().file("src/main.ts", "").build();
    let mut workbench = Workbench::new(std::sync::Arc::new(host));

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project).unwrap().notices, []);
    let mut child = workbench.spawn(project, ProcessSpec::new("/usr/bin/env")).unwrap();
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert!(child.control.wait().unwrap().success());
    assert!(out.lines().any(|line| line == "GENEA_LAUNCH_MARKER=passed through the shell"), "{out}");
    assert!(out.lines().any(|line| line.starts_with("HOME=")), "{out}");
    assert!(out.lines().any(|line| line.starts_with("PATH=")), "{out}");
}
