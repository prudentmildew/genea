//! The templates' slow lane (ticket #60, spec #19 Testing Decisions): each
//! template × package-manager combination is generated, installed, and its
//! `typecheck`, `lint` and `test` scripts must pass.
//!
//! Opt-in, because installing needs the network:
//!
//! ```sh
//! cargo test -p genea-core --test templates_slow -- --ignored
//! ```
//!
//! It runs `node`, `pnpm` and `bun` from PATH and pins the project to their
//! versions, so each package manager runs the version the project asks for.
//! Set `GENEA_KEEP_TEMPLATES=1` to keep the generated projects for
//! inspection.

use std::{
    path::Path,
    process::{Command, Output},
};

use genea_core::{NewProject, PackageManagerPin, ProjectCreation, RuntimePin, Template, Workbench};
use genea_testkit::{FixtureProject, TestHost};

#[derive(Clone, Copy)]
enum Pm {
    Pnpm,
    Bun,
}

fn run(dir: &Path, program: &str, args: &[&str]) -> Output {
    let output = Command::new(program)
        .args(args)
        .current_dir(dir)
        .env("CI", "1")
        .output()
        .unwrap_or_else(|e| panic!("run {program}: {e}"));
    assert!(
        output.status.success(),
        "`{program} {}` failed in {}:\n{}\n{}",
        args.join(" "),
        dir.display(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    output
}

fn version(program: &str) -> String {
    let output = run(Path::new("."), program, &["--version"]);
    String::from_utf8(output.stdout).unwrap().trim().trim_start_matches('v').to_owned()
}

/// Generates the template, installs it and runs its checks.
fn check(template: Template, pm: Pm) -> FixtureProject {
    let parent = FixtureProject::new().build();
    let folder = parent.path("my-app");
    let (runtime, package_manager) = match pm {
        Pm::Pnpm => (RuntimePin::Node(version("node")), PackageManagerPin::Pnpm(version("pnpm"))),
        Pm::Bun => (RuntimePin::Bun(version("bun")), PackageManagerPin::Bun(version("bun"))),
    };
    let mut workbench = Workbench::new(TestHost::new().shared());
    workbench.create_project(NewProject { template, name: "my-app".into(), folder: folder.clone(), runtime, package_manager });
    workbench.settle().unwrap();
    assert_eq!(workbench.project_creation(), Some(ProjectCreation::Created { folder: folder.clone() }));

    let program = match pm {
        Pm::Pnpm => "pnpm",
        Pm::Bun => "bun",
    };
    run(&folder, program, &["install"]);
    for script in ["typecheck", "lint", "test"] {
        run(&folder, program, &["run", script]);
    }
    // Generated files are already formatted the way Oxfmt formats them.
    run(&folder, program, &["run", "format", "--check"]);
    parent
}

fn keep(parent: FixtureProject) {
    if std::env::var_os("GENEA_KEEP_TEMPLATES").is_some() {
        eprintln!("kept {}", parent.keep().display());
    }
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn frontend_with_pnpm() {
    keep(check(Template::Frontend, Pm::Pnpm));
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn frontend_with_bun() {
    keep(check(Template::Frontend, Pm::Bun));
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn backend_with_pnpm() {
    keep(check(Template::Backend, Pm::Pnpm));
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn backend_with_bun() {
    keep(check(Template::Backend, Pm::Bun));
}
