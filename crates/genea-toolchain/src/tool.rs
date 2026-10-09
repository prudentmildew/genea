use std::fmt;

use crate::Version;

/// A tool Genea downloads: a runtime, a package manager, or (Bun) both.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Tool {
    Node,
    Bun,
    Pnpm,
}

impl Tool {
    pub const ALL: [Tool; 3] = [Tool::Node, Tool::Bun, Tool::Pnpm];

    /// The name `package.json` uses, which is also the store folder and the
    /// executable's name: `node`, `bun`, `pnpm`.
    pub fn id(self) -> &'static str {
        match self {
            Tool::Node => "node",
            Tool::Bun => "bun",
            Tool::Pnpm => "pnpm",
        }
    }

    /// The name the user sees: `Node`, `Bun`, `pnpm`.
    pub fn display_name(self) -> &'static str {
        match self {
            Tool::Node => "Node",
            Tool::Bun => "Bun",
            Tool::Pnpm => "pnpm",
        }
    }

    pub fn from_id(id: &str) -> Option<Tool> {
        Tool::ALL.into_iter().find(|t| t.id() == id)
    }

    /// Genea's built-in version, used when a project doesn't pin the role.
    /// It moves forward with Genea releases (ADR 0005). Node is the active
    /// LTS as of this release.
    pub fn default_version(self) -> Version {
        let version = match self {
            Tool::Node => "24.21.0",
            Tool::Bun => "1.4.2",
            Tool::Pnpm => "12.10.1",
        };
        Version::parse(version).expect("a valid built-in version")
    }
}

impl fmt::Display for Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.display_name())
    }
}
