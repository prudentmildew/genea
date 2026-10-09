//! The release-update check (ticket #64): at most once a day, after start
//! and off the main thread, Genea asks GitHub Releases for the latest
//! release. A newer one shows a notice that links to it.

use std::time::Duration;

use genea_core::{RELEASES_URL, UpdateCheck, UpdateNotice, Workbench};
use genea_testkit::{FixtureProject, TestHost};

const MINUTE: Duration = Duration::from_secs(60);
const HOUR: Duration = Duration::from_secs(60 * 60);

/// A GitHub "latest release" answer, trimmed to what matters.
fn release(tag: &str) -> String {
    format!(
        r#"{{"tag_name": "{tag}", "name": "Genea {tag}", "draft": false, "prerelease": false,
            "html_url": "https://github.com/prudentmildew/genea/releases/tag/{tag}",
            "assets": [{{"name": "Genea.dmg"}}]}}"#
    )
}

/// Somewhere to keep update-check state between runs, like the app's
/// application-support folder.
fn state_dir() -> FixtureProject {
    FixtureProject::new().build()
}

fn check(version: &str, state: &FixtureProject) -> UpdateCheck {
    UpdateCheck { current_version: version.into(), state_file: state.path("update-check.json") }
}

#[test]
fn a_newer_release_shows_a_notice_that_links_to_it() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    let state = state_dir();
    let mut workbench = Workbench::new(host.shared());

    workbench.start_update_checks(check("0.1.0", &state));
    host.clock().advance(MINUTE);
    workbench.settle().unwrap();

    assert_eq!(
        workbench.update_notice(),
        Some(UpdateNotice {
            version: "0.2.0".into(),
            url: "https://github.com/prudentmildew/genea/releases/tag/v0.2.0".into(),
            message: "Genea 0.2.0 is available".into(),
        })
    );
}

#[test]
fn the_current_release_shows_no_notice() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    let state = state_dir();
    let mut workbench = Workbench::new(host.shared());

    workbench.start_update_checks(check("0.2.0", &state));
    host.clock().advance(MINUTE);
    workbench.settle().unwrap();

    assert_eq!(host.downloads().requests(), [RELEASES_URL]);
    assert_eq!(workbench.update_notice(), None);
}

#[test]
fn versions_compare_by_number_not_by_text() {
    for (current, latest, newer) in
        [("0.9.0", "v0.10.0", true), ("0.10.0", "v0.9.0", false), ("1.0.0-beta.2", "v1.0.0", true)]
    {
        let host = TestHost::new();
        host.downloads().serve(RELEASES_URL, release(latest));
        let state = state_dir();
        let mut workbench = Workbench::new(host.shared());

        workbench.start_update_checks(check(current, &state));
        host.clock().advance(MINUTE);
        workbench.settle().unwrap();

        assert_eq!(workbench.update_notice().is_some(), newer, "{current} → {latest}");
    }
}

#[test]
fn the_check_waits_until_after_start() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    let state = state_dir();
    let mut workbench = Workbench::new(host.shared());

    workbench.start_update_checks(check("0.1.0", &state));
    workbench.settle().unwrap();

    assert_eq!(host.downloads().requests(), Vec::<String>::new());
    assert_eq!(workbench.update_notice(), None);
}

#[test]
fn a_failed_check_shows_no_notice() {
    for answer in [Err(503), Ok("<html>rate limited</html>")] {
        let host = TestHost::new();
        match answer {
            Ok(body) => host.downloads().serve(RELEASES_URL, body),
            Err(status) => host.downloads().fail(RELEASES_URL, status),
        }
        let state = state_dir();
        let mut workbench = Workbench::new(host.shared());

        workbench.start_update_checks(check("0.1.0", &state));
        host.clock().advance(MINUTE);
        workbench.settle().unwrap();

        assert_eq!(host.downloads().requests(), [RELEASES_URL]);
        assert_eq!(workbench.update_notice(), None);
    }
}

#[test]
fn a_running_genea_checks_at_most_once_a_day() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.1.0"));
    let state = state_dir();
    let mut workbench = Workbench::new(host.shared());
    workbench.start_update_checks(check("0.1.0", &state));
    host.clock().advance(MINUTE);
    workbench.settle().unwrap();

    host.clock().advance(23 * HOUR);
    workbench.settle().unwrap();
    assert_eq!(host.downloads().requests().len(), 1);

    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    host.clock().advance(2 * HOUR);
    workbench.settle().unwrap();
    assert_eq!(host.downloads().requests().len(), 2);
    assert_eq!(workbench.update_notice().map(|n| n.version), Some("0.2.0".into()));
}
