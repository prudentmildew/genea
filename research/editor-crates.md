# Rust crates for the editor's building blocks

Research for issue #5: which Rust crates exist, as of October 2026, for the building blocks of Genea's editor, how mature each one is, and what Zed, Helix and Lapce use. This note records facts only. Choices are made in later grilling tickets.

Gathered on 2026-10-09 from primary sources:

- **crates.io API**: version, publish date, license, MSRV, total downloads and owners for each crate (`https://crates.io/api/v1/crates/<name>`).
- **GitHub**: each crate's own README, CHANGELOG and Cargo.toml on the default branch, plus repo metadata from the GraphQL API (last commit to the default branch, archived flag, stars).
- **Editor manifests**: the workspace and per-crate `Cargo.toml` files of Zed (`main`), Helix (`master`) and Lapce (`master`).

"Last commit" means the latest commit on the repo's default branch. Download counts are all-time totals from crates.io.

## Summary

| Area | Main candidates (latest stable) | Zed | Helix | Lapce |
|---|---|---|---|---|
| Text buffer | `ropey` 1.6.1 (2.0 in beta), `crop` 0.4.3, Zed `sum_tree` (published as `gpui_sum_tree` / `zed-sum-tree`) | own `rope` + `text` crates on `sum_tree` | `ropey` 1.6.1 | `lapce-xi-rope` 0.3.2 (xi-rope fork) |
| Parsing / highlighting | `tree-sitter` 0.27.1, `tree-sitter-typescript` 0.23.2, `tree-sitter-javascript` 0.25.0; `oxc_parser` 0.153.0 | `tree-sitter` (git rev) + forked `tree-sitter-typescript` | `tree-house` 0.4 (own bindings over the tree-sitter C library) | `tree-sitter` 0.22.6, grammars loaded at runtime via `libloading` |
| Terminal | `alacritty_terminal` 0.26.0; `wezterm-term` (not on crates.io); `termwiz` 0.23.3, `vt100` 0.16.2, `vte` 0.15.0, `portable-pty` 0.9.0 | `alacritty_terminal` (Zed fork, git rev) + `vte` 0.15 | none (Helix is itself a TUI) | `alacritty_terminal` (upstream git rev) |
| Git | `gix` 0.89.0, `git2` 0.21.0 (libgit2 1.9) | neither: shells out to the `git` binary | `gix` 0.85 | `git2` 0.21 (vendored libgit2 + OpenSSL) |
| Project search | `grep` 0.4.1 facade, `grep-searcher` 0.1.17, `grep-regex` 0.1.14, `ignore` 0.4.33, `globset` 0.4.20 | `ignore` + `globset`; search on `regex` / `fancy-regex` / `aho-corasick` (no `grep-*`) | `ignore` + `grep-searcher` / `grep-regex` / `grep-matcher` | `ignore` + `grep-searcher` / `grep-regex` / `grep-matcher` |
| Fuzzy matching | `nucleo` 0.5.0 / `nucleo-matcher` 0.3.1, `frizbee` 0.13.0, `fuzzy-matcher` 0.3.7 (repo archived) | own `fuzzy` crate + `nucleo-matcher` 0.3 (`fuzzy_nucleo`) | `nucleo` 0.5 | `nucleo` 0.5 |
| File watching | `notify` 8.2.0 (9.0.0-rc.5 in RC), `notify-debouncer-full` 0.7.0 | `notify` 9.0.0-rc.4, patched to a Zed fork | none in manifests | `notify` 5.2 |
| LSP plumbing | `lsp-types` 0.97.0, `gen-lsp-types` 0.11.0, `async-lsp` 0.2.4, `tower-lsp` 0.20.0, `tower-lsp-server` 0.23.0, `lsp-server` 0.10.0 | own `lsp` crate + forked `lsp-types` (git rev) | own `helix-lsp` + in-tree `helix-lsp-types` | `lsp-types` 0.95.1 + `jsonrpc-lite`, own proxy process |

None of the three editors uses an off-the-shelf LSP *client* framework. Each one hand-rolls JSON-RPC over the language server's stdio on top of a types crate.

## 1. Text buffer (rope)

### ropey

- Stable 1.6.1 (2023-10-18). On crates.io, `2.0.0-alpha.1` (2024-10-20) through `2.0.0-beta.1` (2025-08-02) have been published. The CHANGELOG on `master` still lists 1.6.1 as the latest release. [crates.io versions; ropey CHANGELOG.md]
- MIT. 12.9M downloads. Owners: `cessen`, `archseer`, `pascalkuthe` (the last two are Helix maintainers). Last commit 2026-09-09. [crates.io owners; GitHub]
- The README pitches it as "the backing text-buffer for applications such as text editors". Edits are in single-digit microseconds even on multi-gigabyte texts. It indexes by `char` and tracks lines, including CRLF and all Unicode line endings. Clones are cheap (8 bytes, shared data) and thread-safe. The README warns that it allocates in kilobyte chunks, so it is wasteful for very small texts, and it is in-memory only. [ropey README.md]
- Used by **Helix**: `ropey = { version = "1.6.1", features = ["simd"] }` in the workspace manifest. Helix's `tree-house-bindings` has an optional `ropey` integration. [helix Cargo.toml; tree-house bindings/Cargo.toml]

### crop

- 0.4.3 (2025-04-25), 7 releases since 2023-02. MIT, MSRV 1.85. 384k downloads. Owner `noib3`. The repo moved to `noib3/crop`; last commit 2026-08-23. [crates.io; GitHub]
- A B-tree rope with "an extreme focus on performance". Clones cost 16 bytes and are copy-on-write, so a snapshot can be sent to a background thread. It slices by byte offset or by line. [crop README.md]
- Not used by Zed, Helix or Lapce (absent from all three manifests).

### Zed's `sum_tree` (+ `rope`, `text`)

- `crates/sum_tree` in the Zed repo is "A sum tree data structure, a concurrency-friendly B-tree". It is Apache-2.0 and `publish = false` in-tree. [zed crates/sum_tree/Cargo.toml]
- Zed's `Rope` is `struct Rope { chunks: SumTree<Chunk> }` (`crates/rope`). The `text` crate builds the editable buffer on `rope` and `sum_tree`. Both `rope` and `text` are **GPL-3.0-or-later**. [zed crates/rope/src/rope.rs; crates/rope/Cargo.toml; crates/text/Cargo.toml]
- On crates.io, snapshots exist as `gpui_sum_tree` 0.2.2 (2025-10-22) and `zed-sum-tree` 0.2.0 (2025-10-09). Both are Apache-2.0 and owned by Zed staff plus the `github:zed-industries:crates-io` team. They were published alongside `gpui` 0.2.2, which depends on `sum_tree`. Neither has had a release in about a year, while the in-tree crate keeps changing. [crates.io; zed crates/gpui/Cargo.toml]
- No Zed `rope` or `text` crate is published (`zed-rope` returns 404).

### Others

- `lapce-xi-rope`: Lapce's fork of xi-editor's rope. 0.4.0 (2025-12-19), Apache-2.0, single owner `dzhou121`. Lapce itself pins 0.3.2. [crates.io; lapce Cargo.toml]
- `xi-rope` 0.3.0: last release 2019-06-29 (xi-editor is discontinued). `jumprope` 1.1.2: last release 2023-05-15. [crates.io]

## 2. Incremental parsing and highlighting

### tree-sitter (core)

- `tree-sitter` 0.27.1 (2026-10-08). Active: 0.26.10 through 0.27.1 shipped between June and October 2026. MIT, MSRV 1.90. 44.5M downloads. The repo is at 0.28.0 on `master`. [crates.io; tree-sitter Cargo.toml]
- It has an optional `wasm` feature (via `wasmtime-c-api`) for loading grammars compiled to WebAssembly. `tree-sitter-highlight` 0.27.1 ships from the same repo. Grammar crates depend on the small `tree-sitter-language` 0.1 crate, which decouples them from the core version. [tree-sitter lib/Cargo.toml; crates.io]

### TypeScript / TSX / JavaScript grammars

- `tree-sitter-typescript` 0.23.2 (2024-11-11). One crate exposes two grammars, TypeScript and TSX ("two different dialects"). It depends only on `tree-sitter-language = "0.1"`. MIT. The last commit was 2025-01-30, so there has been no release in nearly two years. [tree-sitter-typescript Cargo.toml, README.md; GitHub]
- `tree-sitter-javascript` 0.25.0 (2025-09-01). Last commit 2025-09-15. [crates.io; GitHub]
- **Zed** uses a fork, `zed-industries/tree-sitter-typescript`, for upstream PR #347 ("Add `import_clause` field to `import_statement`"). That PR has been open since 2025-10-04. Zed's manifest has no `tree-sitter-javascript`. Zed also pins `tree-sitter` to a git rev and patches `tree-sitter-language` so the crates.io grammar crates and the git core share one `LanguageFn` type. [zed Cargo.toml; GitHub PR #347]
- Basic file types are covered by upstream grammar crates. Zed's workspace pins `tree-sitter-css` 0.23, `tree-sitter-html` 0.23, `tree-sitter-json` 0.24 and `tree-sitter-jsdoc` 0.23, plus forks for Markdown and YAML. Zed's fork of `tree-sitter-md` carries a comment about a `serialize()` buffer-overflow fix. [zed Cargo.toml]

### Helix: tree-house

- `tree-house` 0.4.0 (2026-05-31) and `tree-house-bindings` 0.3.2 (2026-06-01). MPL-2.0. "Homey Rust bindings for the tree-sitter C library". These are Helix's own bindings: they compile the C library via `cc`, not the `tree-sitter` crate. They load grammars with `libloading` and have optional `ropey` integration. [tree-house bindings/Cargo.toml; crates.io]
- The README says `tree-house` "provides Helix's syntax highlighting and all other tree-sitter features since the 25.07 release". It also says "Documentation is a work-in-progress and these crates may see breaking changes". [tree-house README.md]

### Lapce

- `tree-sitter = "0.22.6"` (two minor versions behind current) and `libloading` in `lapce-core`. Grammars are loaded as dynamic libraries. [lapce-core/Cargo.toml]

### oxc_parser

- `oxc_parser` 0.153.0 (2026-10-05). MIT, MSRV 1.97.0. 8.4M downloads. It is released **weekly**: 0.148 through 0.153 shipped between 2026-09-01 and 2026-10-05. It is still 0.x, so each release may carry breaking changes. [crates.io]
- A recursive-descent parser for JavaScript, TypeScript, JSX and TSX with error recovery ("IDE-friendly parsing"). It produces a full AST in an arena allocator (`oxc_allocator`). Spans are `u32`, which caps a file at 4 GiB. Scope binding and symbol resolution happen in `oxc_semantic`, not in the parser. The API is one-shot: `Parser::new(&allocator, &source_text, source_type).parse()`. Neither the crate docs nor the README mention incremental reparsing. [oxc_parser src/lib.rs, README.md]
- Part of the Oxc toolchain (VoidZero) that powers Rolldown, oxlint and oxfmt. [oxc README.md]
- None of Zed, Helix or Lapce depends on any `oxc_*` crate.

## 3. Terminal emulation

### alacritty_terminal

- 0.26.0 (2026-04-06), "Library for writing terminal emulators". Apache-2.0, MSRV 1.85. 1.9M downloads. It tracks Alacritty releases (Alacritty v0.17.0 shipped the same day). The `master` branch is at 0.26.1-dev. The CHANGELOG marks breaking changes in bold; for example, 0.26 changed `ChildEvent::Exited` to carry `ExitStatus`. [crates.io; alacritty_terminal Cargo.toml, CHANGELOG.md]
- Includes PTY handling (`rustix-openpty`, `polling`) and builds on `vte` 0.15 for escape-sequence parsing. [alacritty_terminal Cargo.toml]
- Used by **Zed**, via the fork `zed-industries/alacritty` at a git rev, at 0.26.1-dev, alongside `vte` 0.15. Used by **Lapce**, via upstream `alacritty/alacritty` at a git rev. [zed Cargo.toml; lapce Cargo.toml]

### wezterm-term and friends

- `wezterm-term` (in the `term/` directory of the wezterm repo, MIT) is "full featured": escape parsing, key and mouse encoding, scrollback, sixel and iTerm2 images, and OSC 8 hyperlinks. It provides no GUI and no PTY: you feed it bytes via `advance_bytes`. **It is not published on crates.io** (the crate name returns 404), so it can only be used as a git dependency. [wezterm term/README.md, term/Cargo.toml; crates.io]
- The wezterm repo is active (last commit 2026-10-05), but its last tagged release is `20240203-110809-5046fc22`. Published sibling crates: `termwiz` 0.23.3 (2025-03-20) and `portable-pty` 0.9.0 (2025-02-11), both MIT. [GitHub; crates.io]
- `vt100` 0.16.2 (2025-07-12, MIT) is a standalone in-memory terminal parser. `vte` 0.15.0 (2025-02-02) is the parser alone. [crates.io]

## 4. Git

### gix (gitoxide)

- `gix` 0.89.0 (2026-10-08). MIT OR Apache-2.0, MSRV 1.88. 50.9M downloads. It releases about monthly, with several 0.x breaking bumps per quarter (0.85 on 2026-06-22, 0.89 on 2026-10-08). [crates.io]
- The README's feature list:
  - **Done**: clone, fetch, status, blob and tree diff, blame (plumbing), commit (no hooks), commit-graph traversal, worktree checkout, objects, refs, index, config, pathspecs, revspecs, `.gitignore` and `.gitattributes`.
  - **Not done**: push, commit merge (blob and tree merge are done), rebase, reset.

  The `gix` crate itself is listed under "Initial Development". Only `gix-lock` and `gix-tempfile` are "Production Grade". [gitoxide README.md]
- Pure Rust (no C dependency). Used by **Helix**: `helix-vcs` pins `gix = "0.85.0"` with features `attributes`, `status`, `max-performance`, `sha1`. Helix uses it for diff gutters and status. [helix-vcs/Cargo.toml]

### git2 (libgit2 bindings)

- `git2` 0.21.0 (2026-05-18). MIT OR Apache-2.0. 118.8M downloads. Maintained under `rust-lang/git2-rs`, last commit 2026-10-05. It requires libgit2 1.9.6+, vendored in `libgit2-sys` 0.18.7 and linked statically when no suitable system libgit2 is found or the `vendored-libgit2` feature is set. Network support (`https`, `ssh`) is opt-in. [git2-rs README.md, Cargo.toml; crates.io]
- Used by **Lapce**: `git2 = { version = "0.21.0", features = ["vendored-openssl", "vendored-libgit2"] }`. [lapce Cargo.toml]

### Zed: the git CLI

- Zed's `git` crate depends on neither `git2` nor `gix`, and neither crate appears anywhere in Zed's workspace manifest. `crates/git/src/repository.rs` runs commands through a `GitBinary` wrapper around the `git` executable. [zed crates/git/Cargo.toml, Cargo.toml, crates/git/src/repository.rs]

## 5. Project search

- **`ignore`** 0.4.33 (2026-08-04): "a fast recursive directory iterator that respects various filters such as globs, file types and `.gitignore` files". It is part of ripgrep, Unlicense OR MIT, MSRV 1.88, 186.9M downloads. **`globset`** 0.4.20 comes from the same repo. ripgrep 15.2.0 was released 2026-07-15. [ignore README.md; crates.io; GitHub]
- **`grep`** 0.4.1 is a facade over `grep-searcher` 0.1.17 (2026-07-15), `grep-regex` 0.1.14 and `grep-matcher`: "ripgrep, as a library". Its README warns: "This crate isn't ready for wide use yet… there is no high level documentation describing how all of the pieces fit together." The sub-crates are what ripgrep itself runs on. [grep README.md; crates.io]
- **Helix** (`helix-term`) and **Lapce** (`lapce-proxy`) both depend on `ignore` + `grep-searcher` + `grep-regex` + `grep-matcher` directly. **Zed** uses `ignore` and `globset` (in `fs` and `worktree`). It has no `grep-*` crates; its `project` crate searches with `regex`, `fancy-regex` and `aho-corasick`. [helix-term/Cargo.toml; lapce-proxy/Cargo.toml; zed Cargo.toml, crates/project/Cargo.toml]

## 6. Fuzzy matching

- **`nucleo`** 0.5.0 (2024-04-02) and **`nucleo-matcher`** 0.3.1 (2024-02-20). **MPL-2.0**, owned by Helix maintainers. The repo is active (last commit 2026-06-22), but nothing has been released since early 2024.
  - It uses fzf's scoring with a "more faithful" Smith-Waterman, handles Unicode graphemes and reports grapheme indices for highlighting. The README claims it is about 6x faster than `skim` / `fuzzy-matcher`.
  - Status per the README: "`nucleo-matcher` crate is finished and ready for widespread use"; the high-level `nucleo` crate "will likely see a few API changes".

  [nucleo README.md; crates.io]
- **`frizbee`** 0.13.0 (2026-08-13), MIT, MSRV 1.89. A SIMD, typo-resistant Smith-Waterman matcher with affine gaps. It claims about 4x nucleo's speed with typo resistance off and 20x on Unicode, and supports multithreaded matching. Used by blink.cmp, atuin, television and skim. It is young (first release 2025-02) and moves fast: 0.9 to 0.13 in four months. [frizbee README.md; crates.io]
- **`fuzzy-matcher`** 0.3.7 (2020-10-04). MIT. The repo (now `skim-rs/fuzzy-matcher`) is **archived**. [crates.io; GitHub]
- **Zed** has its own `fuzzy` crate (GPL) plus `fuzzy_nucleo` on `nucleo-matcher` 0.3. **Helix** and **Lapce** both use `nucleo` 0.5. [zed Cargo.toml, crates/fuzzy_nucleo/Cargo.toml; helix Cargo.toml; lapce-app/Cargo.toml]

## 7. File watching

- **`notify`** 8.2.0 (2025-08-03) is the latest stable. `9.0.0-rc.1` through `9.0.0-rc.5` shipped from 2026-01-25 to 2026-08-30, with an upgrade guide (v8 to v9) in the repo. CC0-1.0 (its debouncers and `notify-types` are MIT OR Apache-2.0). MSRV 1.88. 167.8M downloads. [crates.io; notify README.md]
- On macOS it defaults to FSEvents (feature `macos_fsevent`, now via `objc2-core-services`); kqueue is optional. It also supports inotify, ReadDirectoryChangesW and polling. `notify-debouncer-full` 0.7.0 / `notify-debouncer-mini` 0.7.0 provide event coalescing. The README lists users including alacritty, deno, rust-analyzer, watchexec and Zed. [notify README.md, notify/Cargo.toml]
- **Zed**: `crates/fs` depends on `notify = "9.0.0-rc.4"`, and the workspace `[patch.crates-io]` replaces it with `zed-industries/notify` at a git rev. **Lapce**: `notify` 5.2.0. **Helix**: no file-watching crate in any manifest. [zed crates/fs/Cargo.toml, Cargo.toml; lapce Cargo.toml; helix manifests]

## 8. LSP client plumbing

### Type crates

- **`lsp-types`** 0.97.0 (2024-06-04). MIT, 39.3M downloads. Its README says it supports LSP 3.16 with 3.17 behind a `proposed` flag. Last commit 2024-06-04, so it has been dormant for over two years. [lsp-types README.md; crates.io; GitHub]
- **`ls-types`** 0.0.6 was a fork of `lsp-types` from the tower-lsp community. It is **archived**: its README says it is "superseded by gen-lsp-types". [ls-types README.md; GitHub]
- **`gen-lsp-types`** 0.11.0 (2026-07-28, first release 2026-04-19). MIT. All types are generated from the official LSP metaModel and target LSP 3.18. rust-analyzer's `lsp-server` 0.10.0 and `tower-lsp-server` 0.24.0-rc.1 have both moved to it. [gen-lsp-types README.md; rust-analyzer lib/lsp-server/Cargo.toml; tower-lsp-server Cargo.toml]

### Frameworks

- **`async-lsp`** 0.2.4 (2026-04-24). MIT OR Apache-2.0, 1.7M downloads, single maintainer (`oxalica`). Built on tower. The README says it "can be used to build both Language Server and Language Client". It runs notification handlers in order (synchronously), unlike tower-lsp. It does little beyond (de)serialization and request-id handling. It still depends on `lsp-types` 0.95. [async-lsp README.md, Cargo.toml; crates.io]
- **`tower-lsp`** 0.20.0 (2023-08-11). The repo's last commit was 2024-01-05, so it is effectively unmaintained. It is server-only. [crates.io; GitHub; async-lsp README.md]
- **`tower-lsp-server`** 0.23.0 (2025-12-07), with 0.24.0-rc.1 on 2026-09-11. "A community fork of tower-lsp". Server-side. [tower-lsp-server README.md; crates.io]
- **`lsp-server`** 0.10.0 (2026-07-16), from rust-analyzer. Described by async-lsp's README as "a simple and synchronous framework for only Language Server". [crates.io; async-lsp README.md]

### What the editors do

- **Zed**: its own `lsp` crate on `async-pipe` (Zed fork) + `lsp-types` (Zed fork `zed-industries/lsp-types` at a git rev) + `serde_json`.
- **Helix**: its own `helix-lsp` on tokio + `helix-lsp-types`, an in-tree MIT crate (a fork of lsp-types).
- **Lapce**: `lsp-types` 0.95.1 (`proposed` feature) + `jsonrpc-lite`, running inside its separate `lapce-proxy` process.

[zed Cargo.toml, crates/lsp/Cargo.toml; helix-lsp/Cargo.toml, helix-lsp-types/Cargo.toml; lapce Cargo.toml, lapce-proxy/Cargo.toml]

## Reference: the three editors

| | Zed | Helix | Lapce |
|---|---|---|---|
| Activity | last commit 2026-10-09, release v1.23.2 (2026-10-07) | last commit 2026-09-29, release 25.07.1 (2025-07-18) | last commit 2026-09-30, release v0.4.6 (2026-01-21) |
| License | mixed: most crates GPL-3.0-or-later; `gpui` and `sum_tree` Apache-2.0 | MPL-2.0 | Apache-2.0 |
| UI | `gpui` (own, GPU, Apache-2.0; 0.2.2 on crates.io) | TUI (`termina` / crossterm) | `floem` (git rev; 0.2.0 on crates.io, 2024-11) |

[GitHub GraphQL repo metadata; manifests listed above]

## Points that matter for later grilling tickets (facts, not choices)

- **Licensing**: `nucleo`, `tree-house` and all of Helix are MPL-2.0, which is file-level copyleft. Zed's `rope` and `text` are GPL-3.0-or-later; only `sum_tree` and `gpui` are Apache-2.0. `notify` is CC0. Everything else listed is MIT and/or Apache-2.0 (ripgrep's crates add Unlicense).
- **Pre-1.0 everywhere** except `notify` (8.x) and `ropey` (1.x). Fast-moving 0.x crates with frequent breaking bumps: `oxc_*` (weekly), `gix` (about monthly), `tree-sitter` (several minors per year).
- **Stale but widely used**: `lsp-types` (2024), `tree-sitter-typescript` (2024 release), `nucleo` (2024 release), `tower-lsp` (2023). Zed works around two of these with forks (`lsp-types`, `tree-sitter-typescript`).
- **Not on crates.io**: `wezterm-term`, and Zed's in-tree `rope`, `text`, `lsp` and `fuzzy`.
- **All three editors fork or pin by git rev** at least one core dependency: Zed (`alacritty_terminal`, `notify`, `lsp-types`, `tree-sitter`, `tree-sitter-typescript`) and Lapce (`alacritty_terminal`, `floem`).
