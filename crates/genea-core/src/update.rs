//! The release-update check (spec #19, Packaging and updates; ticket #64).
//!
//! At most once a day, after start and off the main thread, Genea asks
//! GitHub Releases for the latest release. A newer one becomes the
//! workbench's [`UpdateNotice`], which links to the release page. There is
//! no installer: the user downloads the new DMG themselves.
//!
//! "At most once a day" holds across restarts: the time of the last check
//! and what it found are kept in [`UpdateCheck::state_file`], on the host's
//! wall clock.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Weak},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use genea_host::SharedHost;
use serde_json::{Value, json};

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

    /// Runs on a background job: checks if the last check (by this run or an
    /// earlier one) is a day old, else waits for the rest of the day.
    fn run(self: Arc<Self>) -> Apply {
        let now = self.host.clock().system_time();
        let state = State::read(&self.check.state_file);
        // A last check in the future means the system clock moved back:
        // check now rather than trust it.
        let since = state.checked_at.and_then(|at| now.duration_since(at).ok());
        let latest = match since {
            Some(since) if since < INTERVAL => {
                self.clone().schedule(INTERVAL - since);
                state.latest
            }
            _ => {
                // Failures are silent, and keep what an earlier check found.
                let latest = self.fetch_latest().or(state.latest);
                State { checked_at: Some(now), latest: latest.clone() }.write(&self.check.state_file);
                self.clone().schedule(INTERVAL);
                latest
            }
        };
        let notice = latest.filter(|latest| self.is_newer(latest)).map(|latest| UpdateNotice {
            message: format!("Genea {} is available", latest.version),
            version: latest.version,
            url: latest.url,
        });
        Box::new(move |core| core.update_notice = notice)
    }

    fn is_newer(&self, release: &Release) -> bool {
        match (&self.current, Version::parse(&release.version)) {
            (Some(current), Some(version)) => version > *current,
            _ => false,
        }
    }

    /// The latest release, or `None` if GitHub couldn't be asked or gave an
    /// answer without a version.
    fn fetch_latest(&self) -> Option<Release> {
        let mut body = Vec::new();
        self.host.downloads().fetch(RELEASES_URL, &mut body).ok()?;
        let json: Value = serde_json::from_slice(&body).ok()?;
        let tag = json.get("tag_name")?.as_str()?;
        let url = json.get("html_url")?.as_str()?;
        Some(Release { version: tag.strip_prefix('v').unwrap_or(tag).to_owned(), url: url.to_owned() })
    }
}

#[derive(Clone)]
struct Release {
    version: String,
    url: String,
}

/// What the check remembers between runs, in [`UpdateCheck::state_file`]:
/// `{"checked_at": <Unix seconds>, "latest": {"version": …, "url": …}}`.
#[derive(Default)]
struct State {
    checked_at: Option<SystemTime>,
    latest: Option<Release>,
}

impl State {
    /// A missing or unreadable file reads as "never checked".
    fn read(path: &Path) -> State {
        let Some(json) = fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<Value>(&bytes).ok()) else {
            return State::default();
        };
        let checked_at =
            json.get("checked_at").and_then(Value::as_u64).map(|secs| UNIX_EPOCH + Duration::from_secs(secs));
        let latest = json.get("latest").and_then(|latest| {
            Some(Release {
                version: latest.get("version")?.as_str()?.to_owned(),
                url: latest.get("url")?.as_str()?.to_owned(),
            })
        });
        State { checked_at, latest }
    }

    /// Best effort: if it can't be written, the next start checks again.
    fn write(&self, path: &Path) {
        let secs = self.checked_at.and_then(|at| at.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_secs());
        let latest = self.latest.as_ref().map(|r| json!({ "version": r.version, "url": r.url }));
        let text = json!({ "checked_at": secs, "latest": latest }).to_string();
        let temp = path.with_extension("tmp");
        let written = path.parent().map_or(Ok(()), fs::create_dir_all).and_then(|()| fs::write(&temp, text));
        if written.and_then(|()| fs::rename(&temp, path)).is_err() {
            let _ = fs::remove_file(&temp);
        }
    }
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
