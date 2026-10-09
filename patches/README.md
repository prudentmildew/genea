# Patched crates

Genea carries exactly one patch on its dependencies (ADR 0004). This folder
holds it.

| Path | What it is |
|---|---|
| `i-slint-common.patch` | The change itself, as a reviewable diff against the crates.io sources. |
| `i-slint-common/` | `i-slint-common` at the pinned Slint version, copied verbatim from the cargo registry with the patch applied. `[patch.crates-io]` in the root `Cargo.toml` points here. |

## What the patch does

Slint scans every system font when its platform starts, which costs ~25 ms
warm and ~185 ms cold and breaks the start budgets. The patch changes
`create_collection` in `sharedfontique.rs` in two ways:

- `system_fonts: true` becomes `system_fonts: false`, so there is no scan;
- Menlo (`/System/Library/Fonts/Menlo.ttc`) and Apple Symbols
  (`/System/Library/Fonts/Apple Symbols.ttf`) are registered explicitly, in
  that order. They become Slint's generic families, so every `Text`
  resolves through them, the chrome included.

The fonts are hard-coded in the patch rather than passed through
`SLINT_DEFAULT_FONT` / `SLINT_FONT_PATH`, because env vars would leak into
every child process Genea starts. Apple Color Emoji (192 MB) is not
registered here: Genea registers it lazily (spec #19, Editing core).

## Re-applying it on a Slint pin bump

Every `i-slint-*` crate is `=`-pinned to the `slint` version, so a bump needs
a fresh copy of `i-slint-common` at the new version.

1. Bump `slint` and `slint-build` to `=X.Y.Z` in the root `Cargo.toml`.
2. Run `scripts/reapply-slint-patch.sh`. It
   - fetches the new sources and stops if `create_collection` no longer has
     `system_fonts: true`. If that happens, check for an upstream opt-out first:
     ADR 0004 drops the patch once upstream has one. Otherwise update the patch
     by hand;
   - replaces `patches/i-slint-common/` with the new sources and applies
     `i-slint-common.patch`;
   - fails if cargo reports "Patch `i-slint-common` was not used in the
     crate graph", which otherwise silently builds the *unpatched* crate.
3. If `patch` rejects a hunk, edit `patches/i-slint-common/sharedfontique.rs`
   by hand, then regenerate the diff:
   ```sh
   diff -u --label a/sharedfontique.rs --label b/sharedfontique.rs \
     ~/.cargo/registry/src/index.crates.io-*/i-slint-common-X.Y.Z/sharedfontique.rs \
     patches/i-slint-common/sharedfontique.rs > patches/i-slint-common.patch
   ```
4. `cargo build`, then run the benchmark harness: every pin bump must pass the
   start-floor margins and idle memory (ADR 0004).

Anyone can check by hand that the patch is in use:
`cargo tree -p genea-view -i i-slint-common` must show the
`patches/i-slint-common` path, and its `Cargo.lock` entry has no `source =`
line.
