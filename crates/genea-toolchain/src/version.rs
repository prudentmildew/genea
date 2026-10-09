use std::fmt;

/// A tool version (npm-style semver).
pub use nodejs_semver::Version;

/// What a pin asks for: one exact version, or an npm-style range
/// (`^24`, `24.x`, `>=22 <25`, …), which ADR 0005 allows but which isn't
/// reproducible.
#[derive(Clone, Debug)]
pub enum Request {
    Exact(Version),
    Range { range: nodejs_semver::Range, written: String },
}

impl Request {
    /// Parses a pin's version as written in `package.json`. A full
    /// `major.minor.patch` (optionally with a leading `v` or `=`) is exact;
    /// anything else must be a valid range.
    pub fn parse(written: &str) -> Option<Request> {
        let trimmed = written.trim();
        let bare = trimmed.trim_start_matches('=').trim_start_matches('v');
        let parts: Vec<&str> = bare.splitn(3, '.').collect();
        let numeric = |p: &str| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit());
        let patch_numeric = parts.get(2).is_some_and(|p| p.bytes().next().is_some_and(|b| b.is_ascii_digit()));
        if parts.len() == 3 && numeric(parts[0]) && numeric(parts[1]) && patch_numeric && !bare.contains(' ') {
            if let Ok(version) = Version::parse(bare) {
                return Some(Request::Exact(version));
            }
        }
        if trimmed.is_empty() {
            return None;
        }
        nodejs_semver::Range::parse(trimmed)
            .ok()
            .map(|range| Request::Range { range, written: trimmed.to_owned() })
    }

    /// Any version: a pin that names a tool but no version.
    pub fn any() -> Request {
        Request::Range { range: nodejs_semver::Range::any(), written: "*".into() }
    }

    pub fn matches(&self, version: &Version) -> bool {
        match self {
            Request::Exact(exact) => exact == version,
            Request::Range { range, .. } => range.satisfies(version),
        }
    }

    /// The newest of `versions` this request accepts.
    pub fn newest_match<'v>(&self, versions: impl IntoIterator<Item = &'v Version>) -> Option<&'v Version> {
        versions.into_iter().filter(|v| self.matches(v)).max()
    }
}

impl PartialEq for Request {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Request::Exact(a), Request::Exact(b)) => a == b,
            (Request::Range { written: a, .. }, Request::Range { written: b, .. }) => a == b,
            _ => false,
        }
    }
}

impl fmt::Display for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Request::Exact(version) => write!(f, "{version}"),
            Request::Range { written, .. } => f.write_str(written),
        }
    }
}
