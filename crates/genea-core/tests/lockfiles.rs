//! The lockfile cross-check (ticket #37; #8 as amended by #11):
//! `packageManager` decides between pnpm and Bun, and the lockfile at the
//! project root is only checked against it. A mismatch, or both a pnpm and a
//! Bun lockfile, is a warning in Problems on `package.json`.

use genea_core::{
    Command, PackageManagerPin, ProblemItem, ProblemSource, ProjectId, Severity, TextPosition, Workbench,
};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

const PNPM: &str = "{\n  \"name\": \"app\",\n  \"packageManager\": \"pnpm@12.10.1\"\n}\n";
const BUN: &str = "{\n  \"name\": \"app\",\n  \"packageManager\": \"bun@1.4.2\"\n}\n";
const UNPINNED: &str = "{\n  \"name\": \"app\"\n}\n";

fn with_package_json(package_json: &str) -> FixtureBuilder {
    FixtureProject::new().file("package.json", package_json)
}

fn open(host: &TestHost, fixture: &FixtureProject) -> (Workbench, ProjectId) {
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    host.tools().bun("1.4.2");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (workbench, project)
}

fn toolchain_problems(workbench: &Workbench, project: ProjectId) -> Vec<ProblemItem> {
    let view = workbench.project(project).unwrap();
    view.problems.into_iter().filter(|p| p.source == ProblemSource::Toolchain).collect()
}

/// The warning on `package.json`'s `packageManager` line (line 3).
fn warning_at_package_manager(message: &str) -> ProblemItem {
    ProblemItem {
        source: ProblemSource::Toolchain,
        severity: Severity::Warning,
        path: "package.json".into(),
        position: TextPosition { line: 2, column: 2 },
        location: "3:3".into(),
        message: message.into(),
        stale: false,
    }
}

#[test]
fn a_lockfile_that_matches_package_manager_is_fine() {
    let host = TestHost::new();
    for (package_json, lockfile) in [(PNPM, "pnpm-lock.yaml"), (BUN, "bun.lock"), (BUN, "bun.lockb"), (UNPINNED, "pnpm-lock.yaml")]
    {
        let fixture = with_package_json(package_json).file(lockfile, "").build();
        let (workbench, project) = open(&host, &fixture);
        assert_eq!(toolchain_problems(&workbench, project), [], "{lockfile} with {package_json}");
    }
}

#[test]
fn a_bun_lockfile_in_a_pnpm_project_is_a_warning() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).file("bun.lock", "{}\n").build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(
        toolchain_problems(&workbench, project),
        [warning_at_package_manager(
            "packageManager pins pnpm, but bun.lock is a Bun lockfile. Genea uses pnpm, so bun.lock goes stale."
        )]
    );
    let view = workbench.project(project).unwrap();
    assert_eq!(view.status.warnings, 1);
    // packageManager decides: the role still runs pnpm.
    assert_eq!(view.toolchain.package_manager.unwrap().tool, "pnpm");
}

#[test]
fn a_pnpm_lockfile_in_a_bun_project_is_a_warning() {
    let host = TestHost::new();
    let fixture = with_package_json(BUN).file("pnpm-lock.yaml", "lockfileVersion: '9.0'\n").build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(
        toolchain_problems(&workbench, project),
        [warning_at_package_manager(
            "packageManager pins Bun, but pnpm-lock.yaml is a pnpm lockfile. Genea uses Bun, so pnpm-lock.yaml goes stale."
        )]
    );
}

#[test]
fn bun_s_binary_lockfile_counts_too() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).file("bun.lockb", [0u8, 1, 2]).build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(
        toolchain_problems(&workbench, project),
        [warning_at_package_manager(
            "packageManager pins pnpm, but bun.lockb is a Bun lockfile. Genea uses pnpm, so bun.lockb goes stale."
        )]
    );
}

#[test]
fn an_unpinned_project_with_a_bun_lockfile_is_warned_that_genea_uses_pnpm() {
    let host = TestHost::new();
    let fixture = with_package_json(UNPINNED).file("bun.lock", "{}\n").build();

    let (workbench, project) = open(&host, &fixture);

    let problems = toolchain_problems(&workbench, project);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert_eq!(problems[0].path, std::path::Path::new("package.json"));
    assert_eq!(problems[0].location, "1:1");
    assert_eq!(
        problems[0].message,
        "This project doesn't pin its package manager, so Genea uses pnpm, but bun.lock is a Bun lockfile."
    );
}

#[test]
fn both_a_pnpm_and_a_bun_lockfile_is_one_warning() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).file("pnpm-lock.yaml", "").file("bun.lock", "{}\n").build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(
        toolchain_problems(&workbench, project),
        [warning_at_package_manager(
            "Both pnpm-lock.yaml and bun.lock are at the project root. packageManager pins pnpm, so Genea uses pnpm \
             and bun.lock goes stale."
        )]
    );
}

#[test]
fn lockfiles_below_the_root_are_not_checked() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).file("pnpm-lock.yaml", "").file("packages/web/bun.lock", "{}\n").build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(toolchain_problems(&workbench, project), []);
}

#[test]
fn a_foreign_package_manager_is_not_cross_checked_here() {
    let host = TestHost::new();
    let fixture = with_package_json("{ \"packageManager\": \"npm@11.0.0\" }\n").file("pnpm-lock.yaml", "").build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(toolchain_problems(&workbench, project), []);
}

#[test]
fn the_warning_follows_lockfiles_appearing_and_going_on_disk() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).file("pnpm-lock.yaml", "").build();
    let (mut workbench, project) = open(&host, &fixture);
    assert_eq!(toolchain_problems(&workbench, project), []);

    fixture.write("bun.lock", "{}\n");
    workbench.settle().unwrap();
    let problems = toolchain_problems(&workbench, project);
    assert_eq!(problems.len(), 1, "{problems:?}");
    assert!(problems[0].message.starts_with("Both pnpm-lock.yaml and bun.lock"), "{problems:?}");

    fixture.remove("bun.lock");
    workbench.settle().unwrap();
    assert_eq!(toolchain_problems(&workbench, project), []);
}

#[test]
fn picking_the_package_manager_the_lockfile_belongs_to_clears_the_warning() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).file("bun.lock", "{}\n").build();
    let (mut workbench, project) = open(&host, &fixture);
    assert_eq!(toolchain_problems(&workbench, project).len(), 1);

    workbench.dispatch(project, Command::SetPackageManager(PackageManagerPin::Bun("1.4.2".into())));
    workbench.settle().unwrap();

    assert_eq!(toolchain_problems(&workbench, project), []);
    assert_eq!(workbench.project(project).unwrap().toolchain.package_manager.unwrap().tool, "Bun");
}
