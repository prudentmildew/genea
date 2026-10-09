//! The release-update check (spec #19, Packaging and updates; ticket #64).
//!
//! At most once a day, after start and off the main thread, Genea asks
//! GitHub Releases for the latest release. A newer one becomes the
//! workbench's [`UpdateNotice`], which links to the release page. There is
//! no installer: the user downloads the new DMG themselves.

use std::{
    path::PathBuf,
    sync::{Arc, Weak},
    time::Duration,
};

use genea_host::SharedHost;

use crate::jobs::{Apply, Jobs};

/// GitHub's "latest release" endpoint for Genea's repo. It skips drafts and
/// pre-releases.
pub const RELEASES_URL: &str = "https://api.github.com/repos/prudentmildew/genea/releases/latest";

/// How long after start the first check waits, so it never competes with
/// opening the first window.
const START_DELAY: Duration = Duration::from_secs(10);

/// The most often Genea checks.
const INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// How the app sets up the update check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateCheck {
    /// The running Genea's version, e.g. `env!("CARGO_PKG_VERSION")`.
    pub current_version: String,
    /// Where the check keeps what it learned between runs, so that a restart
    /// doesn't check more than once a day. The app puts it in its
    /// application-support folder.
    pub state_file: PathBuf,
}

/// A newer Genea release exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdateNotice {
    /// The release's version, without a leading `v`.
    pub version: String,
    /// The release page, where the DMG is.
    pub url: String,
    /// What the notice says.
    pub message: String,
}

/// The running check. Timers and jobs hold it; it stops once the workbench
/// that started it is gone.
struct Checker {
    host: SharedHost,
    jobs: Jobs,
    check: UpdateCheck,
    current: Option<Version>,
    alive: Weak<()>,
}

/// Starts checking: the first check runs [`START_DELAY`] after this call,
/// on a background job, and later ones a day apart.
pub(crate) fn start(host: SharedHost, jobs: Jobs, alive: Weak<()>, check: UpdateCheck) {
    let current = Version::parse(&check.current_version);
    let checker = Arc::new(Checker { host, jobs, check, current, alive });
    checker.schedule(START_DELAY);
}

impl Checker {
    fn schedule(self: Arc<Self>, delay: Duration) {
        let host = self.host.clone();
        host.clock().after(
            delay,
            Box::new(move || {
                if self.alive.upgrade().is_none() {
                    return;
                }
                let checker = self.clone();
                self.jobs.spawn("update check", move || checker.run());
            }),
        );
    }

    /// Runs on a background job.
    fn run(self: Arc<Self>) -> Apply {
        let latest = self.fetch_latest();
        self.clone().schedule(INTERVAL);
        let Some(latest) = latest else { return Box::new(|_| {}) };
        let newer = match (&self.current, Version::parse(&latest.version)) {
            (Some(current), Some(version)) => version > *current,
            _ => false,
        };
        let notice = newer.then(|| UpdateNotice {
            message: format!("Genea {} is available", latest.version),
            version: latest.version,
            url: latest.url,
        });
        Box::new(move |core| core.update_notice = notice)
    }

    /// The latest release, or `None` if GitHub couldn't be asked or gave an
    /// answer without a version. Failures are silent: the next check retries.
    fn fetch_latest(&self) -> Option<Release> {
        let mut body = Vec::new();
        self.host.downloads().fetch(RELEASES_URL, &mut body).ok()?;
        let json: serde_json::Value = serde_json::from_slice(&body).ok()?;
        let tag = json.get("tag_name")?.as_str()?;
        let url = json.get("html_url")?.as_str()?;
        Some(Release { version: tag.strip_prefix('v').unwrap_or(tag).to_owned(), url: url.to_owned() })
    }
}

struct Release {
    version: String,
    url: String,
}

/// A `MAJOR.MINOR.PATCH` version. A pre-release (`0.2.0-beta.1`) sorts
/// before its release; build metadata is ignored.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
struct Version {
    numbers: [u64; 3],
    is_release: bool,
}

impl Version {
    fn parse(text: &str) -> Option<Version> {
        let text = text.trim();
        let text = text.strip_prefix('v').unwrap_or(text);
        let text = text.split('+').next()?;
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, Some(pre)),
            None => (text, None),
        };
        let mut numbers = [0; 3];
        let mut parts = core.split('.');
        for number in &mut numbers {
            *number = parts.next()?.parse().ok()?;
        }
        if parts.next().is_some() {
            return None;
        }
        Some(Version { numbers, is_release: pre.is_none() })
    }
}
