//! The core is framework-free (ADR 0004, spec #19 Architecture): no core
//! crate may depend on Slint or on the windowing stack under it, directly or
//! transitively, including through dev- and build-deps, on the one target
//! Genea ships for (aarch64-apple-darwin, spec #19 Packaging).
//!
//! When you add a core crate, add it to `CORE_CRATES`.

use std::process::Command;

const CORE_CRATES: &[&str] = &["genea-core", "genea-host", "genea-testkit", "genea-toolchain"];

/// Crate names (or prefixes ending in `-`) that mark a GUI dependency.
const FORBIDDEN: &[&str] = &["slint", "i-slint-", "winit", "muda", "skia-", "wgpu", "objc2-app-kit", "femtovg"];

fn is_forbidden(name: &str) -> bool {
    FORBIDDEN.iter().any(|f| if f.ends_with('-') { name.starts_with(f) } else { name == *f })
}

#[test]
fn core_crates_have_no_gui_dependency() {
    let mut command = Command::new(env!("CARGO"));
    command
        .args(["tree", "--offline", "--target", "aarch64-apple-darwin", "--edges", "normal,build,dev"])
        .args(["--prefix", "none", "--format", "{p}"])
        .current_dir(env!("CARGO_MANIFEST_DIR"));
    for krate in CORE_CRATES {
        command.args(["--package", krate]);
    }
    let output = command.output().expect("run cargo tree");
    assert!(output.status.success(), "cargo tree failed:\n{}", String::from_utf8_lossy(&output.stderr));

    let tree = String::from_utf8(output.stdout).unwrap();
    let names: Vec<&str> = tree.lines().filter_map(|line| line.split_whitespace().next()).collect();
    assert!(names.contains(&"ropey"), "sanity check: the tree should list core's dependencies:\n{tree}");
    let gui: Vec<&str> = names.into_iter().filter(|name| is_forbidden(name)).collect();
    assert!(gui.is_empty(), "core crates depend on GUI crates: {gui:?}");
}

#[test]
fn the_check_recognises_gui_crates() {
    for name in ["slint", "i-slint-core", "winit", "skia-safe", "muda", "objc2-app-kit"] {
        assert!(is_forbidden(name), "{name}");
    }
    for name in ["ropey", "slintish", "tempfile", "objc2"] {
        assert!(!is_forbidden(name), "{name}");
    }
}
