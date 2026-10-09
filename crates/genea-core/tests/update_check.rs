//! The release-update check (ticket #64): at most once a day, after start
//! and off the main thread, Genea asks GitHub Releases for the latest
//! release. A newer one shows a notice that links to it.

use std::time::Duration;

use genea_core::{RELEASES_URL, UpdateNotice, Workbench};
use genea_testkit::TestHost;

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

#[test]
fn a_newer_release_shows_a_notice_that_links_to_it() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    let mut workbench = Workbench::new(host.shared());

    workbench.start_update_checks("0.1.0");
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
    let mut workbench = Workbench::new(host.shared());

    workbench.start_update_checks("0.2.0");
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
            let mut workbench = Workbench::new(host.shared());

        workbench.start_update_checks(current);
        host.clock().advance(MINUTE);
        workbench.settle().unwrap();

        assert_eq!(workbench.update_notice().is_some(), newer, "{current} → {latest}");
    }
}

#[test]
fn the_check_waits_until_after_start() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    let mut workbench = Workbench::new(host.shared());

    workbench.start_update_checks("0.1.0");
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
            let mut workbench = Workbench::new(host.shared());

        workbench.start_update_checks("0.1.0");
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
    let mut workbench = Workbench::new(host.shared());
    workbench.start_update_checks("0.1.0");
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

#[test]
fn a_restart_within_a_day_doesnt_check_again_but_keeps_the_notice() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    let mut first_run = Workbench::new(host.shared());
    first_run.start_update_checks("0.1.0");
    host.clock().advance(MINUTE);
    first_run.settle().unwrap();
    drop(first_run);

    host.clock().advance(HOUR);
    let mut second_run = Workbench::new(host.shared());
    second_run.start_update_checks("0.1.0");
    host.clock().advance(MINUTE);
    second_run.settle().unwrap();

    assert_eq!(host.downloads().requests().len(), 1);
    assert_eq!(second_run.update_notice().map(|n| n.version), Some("0.2.0".into()));

    host.clock().advance(23 * HOUR);
    second_run.settle().unwrap();
    assert_eq!(host.downloads().requests().len(), 2);
}

#[test]
fn a_restart_after_updating_drops_the_notice() {
    let host = TestHost::new();
    host.downloads().serve(RELEASES_URL, release("v0.2.0"));
    let mut first_run = Workbench::new(host.shared());
    first_run.start_update_checks("0.1.0");
    host.clock().advance(MINUTE);
    first_run.settle().unwrap();
    drop(first_run);

    let mut updated = Workbench::new(host.shared());
    updated.start_update_checks("0.2.0");
    host.clock().advance(MINUTE);
    updated.settle().unwrap();

    assert_eq!(host.downloads().requests().len(), 1);
    assert_eq!(updated.update_notice(), None);
}
