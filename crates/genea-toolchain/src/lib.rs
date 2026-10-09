//! The managed toolchain (ADR 0005, spec #19 Toolchain).
//!
//! A project pins its runtime (`devEngines.runtime`: Node or Bun) and its
//! package manager (`packageManager`: pnpm or Bun) in its root
//! `package.json`. Genea downloads those versions itself into one shared
//! store, `<support dir>/toolchains/<tool>/<version>/`, and verifies each
//! download against its publisher's checksum before anything lands there.
//! [`Installed::bin_dir`] is the folder that holds the tool's executable.
//!
//! - [`pins`]: reading pins from `package.json`, and writing exact ones.
//! - [`Store`]: resolving a [`Request`] to a version (newest match in the
//!   store, else newest match published), installing it, and removing it.
//! - [`published`]: every version of a tool its publisher has for macOS
//!   arm64 (the pin pickers list them).
//!
//! This crate is plain blocking code with no threads of its own: genea-core
//! calls it from background jobs, and every download goes through the
//! host's [`Downloads`](genea_host::Downloads).

mod archive;
pub mod pins;
mod sources;
mod store;
mod tool;
mod version;

pub use pins::{Pin, Pins};
pub use sources::published;
pub use store::{Installed, Progress, Store, ToolchainError};
pub use tool::Tool;
pub use version::{Request, Version};
