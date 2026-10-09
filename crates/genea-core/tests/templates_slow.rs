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
//! Set `GENEA_KEEP_TEMPLATES=1` to keep the generated projects (also when a
//! check fails) for inspection.

use std::{
    path::{Path, PathBuf},
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

/// A generated project, deleted on drop unless `GENEA_KEEP_TEMPLATES` is set.
struct Generated {
    parent: Option<FixtureProject>,
    /// The project folder.
    folder: PathBuf,
}

impl Drop for Generated {
    fn drop(&mut self) {
        if std::env::var_os("GENEA_KEEP_TEMPLATES").is_some()
            && let Some(parent) = self.parent.take()
        {
            eprintln!("kept {}", parent.keep().display());
        }
    }
}

/// Generates the template, installs it and runs its checks.
fn check(template: Template, pm: Pm) -> Generated {
    let parent = FixtureProject::new().build();
    let folder = parent.path("my-app");
    let generated = Generated { parent: Some(parent), folder: folder.clone() };
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
    generated
}

/// Imports the Hono app with Node's type stripping (no build step) and
/// requests `path`, returning the response body.
fn request_with_type_stripping(package: &Path, path: &str) -> String {
    let script = format!(
        "const {{ app }} = await import('./src/app.ts'); \
         const response = await app.request('{path}'); \
         console.log(await response.text());"
    );
    let output = run(package, "node", &["--input-type=module", "-e", &script]);
    String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn frontend_with_pnpm() {
    check(Template::Frontend, Pm::Pnpm);
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn frontend_with_bun() {
    check(Template::Frontend, Pm::Bun);
}

fn check_backend(pm: Pm) {
    let generated = check(Template::Backend, pm);
    let body = request_with_type_stripping(&generated.folder, "/hello/Ada");
    assert_eq!(body, r#"{"message":"Hello, Ada!"}"#);
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn backend_with_pnpm() {
    check_backend(Pm::Pnpm);
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn backend_with_bun() {
    check_backend(Pm::Bun);
}

/// The API imports `@my-app/shared` through a workspace symlink. Node only
/// strips types outside `node_modules`, so this checks that the symlink
/// resolves to the real path (#12).
fn check_full_stack(pm: Pm) {
    let generated = check(Template::FullStack, pm);
    let body = request_with_type_stripping(&generated.folder.join("apps/api"), "/api/hello?name=%20Ada%20");
    assert_eq!(body, r#"{"message":"Hello, Ada!"}"#);
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn full_stack_with_pnpm() {
    check_full_stack(Pm::Pnpm);
}

#[test]
#[ignore = "slow lane: installs from the network"]
fn full_stack_with_bun() {
    check_full_stack(Pm::Bun);
}
