# TypeScript's native compiler (tsgo / TypeScript 7) as a language server, as of 2026-10-09

Research for wayfinder ticket #3. Facts only. The architecture decision belongs to a later grilling ticket.

Terms follow `GLOSSARY.md`: **language intelligence** means completions, diagnostics, go-to-definition, find references, rename and refactors. The **first-class languages** are TS, TSX, JS and JSX.

## TL;DR

- **Shipped and stable.** TypeScript 7.0 is GA (blog post 2026-07-08). The current release is `typescript@7.0.2`. 7.1 is in progress: its Beta was planned for 2026-10-06 but had not been published by 2026-10-09. 7.1 Stable is planned for 2026-11-24. The native compiler now lives in `microsoft/TypeScript`; the `microsoft/typescript-go` staging repo is closed. [S1][S3][S4][S6]
- **Real LSP, unlike tsserver.** `tsc --lsp --stdio` is a standard Language Server Protocol server. The classic `tsserver` speaks a custom pre-LSP protocol and needs a wrapper such as vtsls or typescript-language-server to work in non-VS Code editors. [S1][S5][S9]
- **Covers almost all language intelligence, but very few refactors.** Completions with auto-imports, diagnostics (pull and push), definition, type definition, implementation, references, rename, hover, signature help, inlay hints, semantic tokens, code lens, call hierarchy, formatting, folding, selection ranges, linked editing and workspace symbols are all implemented. Code actions are limited to three quick-fix families (missing imports, isolated-declaration type annotations, implement interface), "fix all", and organize, sort and remove-unused imports. There are no refactorings (extract function or constant, move to file, and so on). TS 6 had 73 code-fix modules and 16 refactor modules. [S5][S10][S11]
- **Runs only as a separate process.** It is a Go binary (about 23.6 MB on darwin-arm64). It is distributed through npm platform packages and talks over stdio. No in-process or C-ABI embedding is offered. The programmatic API (`tsc --api`, which uses msgpack or JSON-RPC over stdio or a pipe) is also out-of-process, and it is unstable until 7.1. [S5][S7][S8][S12]
- **Fast.** On the VS Code codebase, time to the first error in the editor dropped from about 17.5 s to under 1.3 s. Full builds are 8–12× faster. Build memory is 6–26% lower. Failing language-server commands fell by more than 80% and server crashes by more than 60% compared with TS 6.0. [S1]
- **Zed** still defaults to vtsls (tsserver). tsgo is available through a first-party Zed Industries extension (`tsgo`, server id `typescript-ls`). That extension npm-installs `typescript` and launches the platform binary with `--lsp --stdio`. Neovim's nvim-lspconfig ships a `tsc` config for it. WebStorm 2026.2 supports TS 7 natively. Helix still defaults to typescript-language-server. [S13][S14][S15][S16][S17]

## 1. Release status

| Date | Event | Source |
|---|---|---|
| 2025-03-11 | Native port announced ("A 10x Faster TypeScript") | [S2] |
| 2025-05-22 | `@typescript/native-preview` nightlies plus a VS Code extension. Auto-imports, find-all-references and rename still pending. | [S2b] |
| 2025-12-02 | Progress update: auto-imports, references, rename, call hierarchy and more work, including with project references. TS 6.0 announced as the last JS-based release. | [S9] |
| 2026-03-23 | TypeScript 6.0 (the bridge release; afterwards patch releases only) | [S3] |
| 2026-04-21 | TypeScript 7.0 Beta | [S3] |
| 2026-06-18 | TypeScript 7.0 RC (the binary is renamed from `tsgo` to `tsc`) | [S3][S6] |
| 2026-07-08 | **TypeScript 7.0 GA** | [S1] |
| 2026-08-20 | `v7.0.2` tagged in `microsoft/TypeScript`. npm `latest` is `7.0.2`. | [S4][S7] |
| 2026-10-09 | npm `next` is `7.1.0-dev.20261009.1`. No 7.1 Beta on npm or the blog yet. | [S7][S3] |
| 2026-11-24 (plan) | 7.1 Stable. 7.1 Beta was planned for 2026-10-06 and RC for 2026-11-10. | [S4b] |

- The README of `microsoft/typescript-go` says: "This Repo Is Closed … continue development and discussion in the original repo … will be permanently archived in September 2026." The Go source now lives under `tsc/` in `microsoft/TypeScript`. [S6][S4]
- That README's last feature table still lists "Language service (LSP): in progress, nearly all features implemented" and "API: not ready". [S6]
- The team plans a "fairly similar timeline to releases prior to TypeScript 7.0, with new featureful versions published every 3-4 months." [S1]
- The 7.1 iteration plan has these Editor Productivity items: "Replace Existing API Integrations in VS Code", plus "Investigate" items for an expandable-hover LSP API, a multi-document highlighting API, and region diagnostics. There are no refactoring items. [S4b]

## 2. LSP feature coverage

### Methods handled by the server (`tsc/internal/lsp/server.go`, identical method set at `v7.0.2` and `main`) [S5]

| Area | LSP methods / capabilities |
|---|---|
| Completions | `textDocument/completion` (resolve supported). Auto-imports are included; the auto-import index lives in `tsc/internal/ls/autoimport/`. [S10] |
| Diagnostics | Pull: `textDocument/diagnostic` (`identifier: "typescript"`, `interFileDependencies: true`). Push: `textDocument/publishDiagnostics`. Push can be turned off with the `disablePushDiagnostics` init option. `workspace/diagnostic/refresh` is sent when the client supports it. There is no `workspace/diagnostic` handler. |
| Navigation | `definition`, `typeDefinition`, `implementation`, `references`, custom `sourceDefinition` (go to source definition), `documentHighlight` plus a custom multi-document highlight, `documentSymbol`, `workspace/symbol`, `prepareCallHierarchy` with incoming and outgoing calls |
| Rename | `textDocument/rename` with `prepareRename`. `workspace/willRenameFiles` updates imports when files are renamed. |
| Code actions | `textDocument/codeAction` with kinds `quickfix`, `source.organizeImports.ts`, `source.removeUnusedImports.ts`, `source.sortImports.ts` and `source.fixAll.ts`. **No `refactor.*` kinds.** |
| Hover / signature | `hover` (expandable hovers per the blog [S1]), `signatureHelp` |
| Inlay hints | `textDocument/inlayHint` |
| Semantic tokens | `semanticTokens/full` and `semanticTokens/range`. Added between 7.0 Beta and RC. [S1] |
| Code lens | `textDocument/codeLens` plus resolve (reference and implementation counts) |
| Editing | `formatting`, `rangeFormatting`, `onTypeFormatting`, `foldingRange`, `selectionRange`, `linkedEditingRange` (JSX tags), VS-specific `onAutoInsert` (JSX closing tags, trigger `>`) |
| Encoding | Negotiates UTF-8 positions when the client offers them; otherwise uses UTF-16. |
| Custom | `custom/projectInfo`, `custom/initializeAPISession`, profiling and GC hooks. On `main` only (so 7.1): `custom/setContentMapperContributions`. |

### Code fixes and refactors: the main gap compared with tsserver

- The registered code-fix providers at `v7.0.2` and on `main` are `ImportFixProvider`, `IsolatedDeclarationsFixProvider` and `FixClassIncorrectlyImplementsInterfaceProvider`. The source comments: "Add more code fix providers here as they are implemented". [S11]
- TS 6.0.3 (`src/services/`) has **73 codefix modules and 16 refactor modules**. The refactors include extract function or constant, extract type, move to file, and convert import/export forms. [S18]
- Neither the 7.0 posts nor the 7.1 iteration plan name refactors as upcoming work. [S1][S4b]

### Timeline of editor-feature parity (from the blog)

- 7.0 GA: "we've added in missing functionality like auto-imports, expandable hovers, inlay hints, code lenses, go-to-source-definition, JSX linked editing and tag completions, and more. Missing features from TypeScript 7.0 beta, such as semantic highlighting, 'sort imports', 'remove unused imports', and more are now in." [S1]
- Language-service issues filed against TS 6 were bulk-closed with the label "7.0 LS Migration", because the language service was "heavily rewritten" for LSP. [S9]

### Not supported

- **tsserver / language-service plugins.** Vue, MDX, Astro and Svelte (all Volar-based) and Angular template type-checking "will likely not yet be able to leverage TypeScript 7". Teams should "use TypeScript 7 in scenarios where language server plugins are not required." [S1]
- 7.1 adds **content mappers**: a `contentMappers` section in `tsconfig.json` names an external process that turns a foreign file type (for example `.vue`) into TS, with source maps. It runs only with `--runExternalCode`. It was merged on 2026-08-19 and is not in 7.0.x. [S19][S5]

## 3. Project references and workspace support

- Build mode, project references and incremental builds are all "done". [S6] As of December 2025, find-all-references, rename and related features "work in any TypeScript or JavaScript codebase – including those with project references." [S9]
- The server keeps a project collection of configured projects plus inferred projects (`tsc/internal/project/`). It discovers the right `tsconfig.json` per open file, so one server serves the whole workspace. The nvim-lspconfig docs state this as: "supports monorepos by default … without the need of spawning multiple instances." [S5][S15]
- **Workspace folders.** The server uses a workspace folder as its working directory only when **exactly one** is sent. Otherwise it falls back to `rootUri`, `rootPath` or the process cwd. [S5]
- **File watching.** The server uses LSP client-side watching (`workspace/didChangeWatchedFiles` with dynamic registration) when the client offers it. Otherwise it uses its own in-process watcher if a fast recursive backend exists (FSEvents on macOS, or Windows). Failing both, watching is disabled. The CLI `--watch` uses a Go port of `@parcel/watcher`. [S5][S1]
- **Automatic type acquisition.** The server is given an `npm install` callback and a global typings cache location (`tsc/cmd/tsc/lsp.go`). [S12]

## 4. Memory and latency figures

All figures come from Microsoft's own posts.

| Metric | TS 6 / Strada | TS 7 | Source |
|---|---|---|---|
| VS Code repo: open editor to first error shown | ~17.5 s | <1.3 s (13×) | [S1] |
| VS Code repo: full project load in editor (Mar 2025 preview) | ~9.6 s | ~1.2 s (8×) | [S2] |
| Canva: time to first error in editor | ~58 s | ~4.8 s | [S1] |
| Language server failing commands | baseline | −80%+ | [S1] |
| Language server crashes | baseline | −60%+ | [S1] |
| Full build, vscode / sentry / bluesky / playwright / tldraw (`--checkers 4`) | 125.7 / 139.8 / 24.3 / 12.8 / 11.2 s | 10.6 / 15.7 / 2.8 / 1.47 / 1.46 s | [S1] |
| Build memory, same five repos | 5.2 / 4.9 / 1.8 / 1.0 / 0.6 GB | 4.2 / 4.6 / 1.3 / 0.9 / 0.5 GB (−6% to −26%) | [S1] |
| Editor memory (Mar 2025 preview, unoptimised) | baseline | "roughly half" | [S2] |
| Kibana project load in WebStorm | ~12 s | ~3 s | [S17] |

- Type-checking uses a fixed pool of checker workers (4 by default). `--checkers`, `--builders` and `--singleThreaded` tune it. More checkers means more memory. The LSP serves "simultaneous requests" on multiple threads. [S1]
- No official steady-state editor memory figure for a large repo was found for 7.0 GA. For comparison, Zed raises tsserver's heap cap to 8 GiB by default for vtsls because "vtsls may run out of memory on very large projects". [S13]

## 5. Distribution

- **npm**: `npm install -D typescript` (≥ 7.0). The `typescript` package contains a Node shim `bin/tsc` and `lib/getExePath.js`. These resolve an optional platform package, `@typescript/typescript-<platform>-<arch>`. Twenty platform packages are published, including `darwin-arm64` and `darwin-x64`. [S7][S8]
- **The platform package is the standalone binary.** `@typescript/typescript-darwin-arm64@7.0.2` contains `lib/tsc` (a 23.6 MB Mach-O arm64 executable) next to the `lib.*.d.ts` files. Editors that do not want to involve Node, such as the Zed extension, run that binary directly. [S8][S14]
- **Nightlies** are published as `typescript@next`. The old `@typescript/native-preview` package (binary `tsgo`) has been superseded. [S1][S7]
- **Side by side with TS 6**: `@typescript/typescript6` provides `tsc6` and re-exports the 6.0 API. This exists for tools such as typescript-eslint that still need the JS API. [S1]
- **No official standalone download** outside npm was found. The GitHub releases for `v7.0.2` and `typescript/v7.0.2` are tags with notes. [S4] A wasm (`wasip1`) npm target was proposed for 7.1 (PR microsoft/typescript-go#4733), but that PR was closed and nothing has shipped. [S4b][S20]
- License: Apache-2.0. [S8]

## 6. Embedding versus a separate process

- **LSP is out-of-process only.** `tsc --lsp` accepts `--stdio`. `--pipe` and `--socket` are parsed but rejected with "only stdio is supported". `--clientProcessId` (or the initialize `processId`) starts a watchdog that shuts the server down when the parent dies. [S12]
- **The API is also out-of-process.** `tsc --api` serves a msgpack protocol (or JSON-RPC with `--async`) over stdio, a named pipe or a Unix socket. The npm package's JS clients (`typescript/unstable/sync`, `typescript/unstable/async` in 7.0.2) spawn the binary with `--api`. [S12][S8] The 7.0 GA post says "TypeScript 7.0 … does not ship with an API" and "We expect TypeScript 7.1 to ship with a new (and different) API". Stabilising it is the first 7.1 item ("Content Mapper API", "Emit API", "Language Service API"). [S1][S4b]
- **No in-process embedding is offered.** All compiler packages are Go `internal/` packages (`tsc/internal/...`), so they cannot be imported from outside the module. No C ABI, shared library or library distribution was found. [S5] An embedder could theoretically vendor the Go module and build a c-archive itself, but that is not a supported or documented path. This is an inference from the module layout, not a Microsoft statement.
- **Comparison with tsserver.** tsserver is a Node.js program. It can run as a process speaking the custom tsserver protocol, or in-process inside a JS host through `typescript.js`'s `LanguageService` API. A Rust host cannot use either without bundling Node. [S9][S1]

## 7. tsgo LSP compared with classic tsserver

| | tsserver (TS ≤ 6.0) | TS 7 `tsc --lsp` |
|---|---|---|
| Runtime | Node.js (JS) | Native Go binary, multi-threaded |
| Protocol | Custom tsserver JSON protocol (pre-LSP). Non-VS Code editors use wrappers (vtsls, typescript-language-server). | Standard LSP over stdio |
| Future | 6.0 is the last JS release; patch releases only for security, severe regressions and 6/7 compatibility | Active line; feature releases every 3–4 months |
| Language intelligence | Full | Nearly full: same nav, completions, rename and diagnostics; **only 3 quick-fix families and no refactors** |
| Plugins (Vue/Svelte/Astro/MDX/Angular via Volar or LS plugins) | Yes | No. 7.1 adds content mappers instead. |
| Programmatic API | Stable JS API, in-process in Node | None stable in 7.0; new out-of-process API in 7.1 |
| Perf | baseline | ~8–13× faster load and first error; 8–12× faster builds |

Sources: [S1][S5][S9][S11][S18]

## 8. How non-VS Code editors integrate it today

- **Zed**: the built-in default is still **vtsls**, with typescript-language-server as the alternate. Zed's TypeScript docs do not mention tsgo. [S13] TS 7 comes through the **`tsgo` extension** (`zed-extensions/tsgo`, authored by Zed Industries, v1.1.0 in the registry). [S14][S16]
  - It npm-installs the `typescript` package (latest, or a `package_version` that must be ≥ 7.0.0).
  - It locates `node_modules/@typescript/typescript-<os>-<arch>/lib/tsc` and runs it with `--lsp --stdio`. Node is used only for installation.
  - Language server id `typescript-ls` serves TypeScript, TSX, JavaScript and JSX, mapped to LSP language ids `typescript`, `typescriptreact`, `javascript` and `javascriptreact`.
  - It sends inlay-hint and code-lens defaults through `workspace/configuration` under `typescript.*` and `javascript.*`.
  - The README suggests running it alongside vtsls: "Zed will use `typescript-ls` for features it supports and fallback to the next language server in the list for unsupported features". That is how a user gets tsserver's refactors next to tsgo.
  - Zed's own file-watcher tests mention tsgo's lowercased path events on macOS. [S16]
- **Neovim** (nvim-lspconfig): ships `lsp/tsc.lua`, which runs `tsc --lsp`. It probes whether a given binary supports `--lsp` and skips a pre-7 `node_modules/.bin/tsc`. The older `tsgo` config is deprecated in favour of `tsc`. [S15]
- **Helix**: `languages.toml` still defaults to `typescript-language-server`. [S21]
- **WebStorm 2026.2**: TS 7 support out of the box, native integration with the Go engine. For Angular, WebStorm ships its own Kotlin port of the Angular template transpiler and maps positions itself. The Vue approach is still being evaluated. [S17]
- **Visual Studio** enables TS 7 automatically per workspace. **VS Code** uses a dedicated TS 7 extension, with built-in support "in the coming weeks" as of 2026-07-08. [S1]

## Sources

- [S1] TypeScript blog, "Announcing TypeScript 7.0", 2026-07-08. https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/
- [S2] TypeScript blog, "A 10x Faster TypeScript", 2025-03-11. https://devblogs.microsoft.com/typescript/typescript-native-port/
- [S2b] TypeScript blog, "Announcing TypeScript Native Previews", 2025-05-22. https://devblogs.microsoft.com/typescript/announcing-typescript-native-previews/
- [S3] TypeScript blog post index (WordPress API): 7.0 Beta 2026-04-21, RC 2026-06-18, GA 2026-07-08; no later posts as of 2026-10-09. https://devblogs.microsoft.com/typescript/ (also https://devblogs.microsoft.com/typescript/announcing-typescript-7-0-beta/ and https://devblogs.microsoft.com/typescript/announcing-typescript-7-0-rc/)
- [S4] microsoft/TypeScript releases and repo layout (Go code under `tsc/`; latest commits 2026-10-08). https://github.com/microsoft/TypeScript/releases
- [S4b] microsoft/TypeScript#63703, "TypeScript 7.1 Iteration Plan" (updated 2026-10-05). https://github.com/microsoft/TypeScript/issues/63703
- [S5] `tsc/internal/lsp/server.go` (main and `v7.0.2`) and the `tsc/internal/project/` tree. https://github.com/microsoft/TypeScript/blob/main/tsc/internal/lsp/server.go
- [S6] microsoft/typescript-go README ("This Repo Is Closed", feature table). https://github.com/microsoft/typescript-go
- [S7] npm registry metadata for `typescript` (dist-tags `latest: 7.0.2`, `next: 7.1.0-dev.20261009.1`; optionalDependencies). https://registry.npmjs.org/typescript
- [S8] Contents of the `typescript-7.0.2.tgz` and `@typescript/typescript-darwin-arm64-7.0.2.tgz` tarballs (`bin/tsc`, `lib/getExePath.js`, `lib/tsc` Mach-O binary, `dist/api/*/client.js` spawning `--api`). https://www.npmjs.com/package/typescript
- [S9] TypeScript blog, "Progress on TypeScript 7 – December 2025", 2025-12-02. https://devblogs.microsoft.com/typescript/progress-on-typescript-7-december-2025/
- [S10] `tsc/internal/ls/` source tree (auto-import, inlay hints, semantic tokens, and so on). https://github.com/microsoft/TypeScript/tree/main/tsc/internal/ls
- [S11] `tsc/internal/ls/codeactions.go` (`codeFixProviders`), main and `v7.0.2`. https://github.com/microsoft/TypeScript/blob/main/tsc/internal/ls/codeactions.go
- [S12] `tsc/cmd/tsc/lsp.go`, `api.go` and `main.go` (`--lsp`, `--api` flags). https://github.com/microsoft/TypeScript/tree/main/tsc/cmd/tsc
- [S13] Zed docs, TypeScript language page (`docs/src/languages/typescript.md`). https://zed.dev/docs/languages/typescript
- [S14] zed-extensions/tsgo (README, `extension.toml`, `src/tsgo.rs`). https://github.com/zed-extensions/tsgo
- [S15] nvim-lspconfig `lsp/tsc.lua` and `lsp/tsgo.lua`. https://github.com/neovim/nvim-lspconfig/blob/master/lsp/tsc.lua
- [S16] zed-industries/extensions `extensions.toml` (`[tsgo] version = "1.1.0"`) and zed `crates/fs/src/fs_watcher.rs`. https://github.com/zed-industries/extensions
- [S17] JetBrains blog, "TypeScript 7 in WebStorm: Faster Coding Assistance for Angular and React, No Migration Required" (2026-09). https://blog.jetbrains.com/webstorm/2026/09/typescript-7-in-webstorm-faster-coding-assistance-for-angular-and-react-no-migration-required/
- [S18] microsoft/TypeScript tree at `v6.0.3`: `src/services/codefixes/` (73 files), `src/services/refactors/` (16 files). https://github.com/microsoft/TypeScript/tree/v6.0.3/src/services
- [S19] microsoft/typescript-go#4712, "Content mappers" (merged 2026-08-19). https://github.com/microsoft/typescript-go/pull/4712
- [S20] microsoft/typescript-go#4733, "Add wasip1 npm build target" (closed, not merged). https://github.com/microsoft/typescript-go/pull/4733
- [S21] helix-editor/helix `languages.toml`. https://github.com/helix-editor/helix/blob/master/languages.toml
