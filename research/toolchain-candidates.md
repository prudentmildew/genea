# Toolchain candidates for each toolchain role

Research for issue #2: "Which tools are the candidates for each toolchain role?" Researched 2026-10-09.

This is **facts only**. It does not pick the blessed tools; a later grilling ticket does that.

## Method

- **Sources.** Every claim cites a primary source:
  - official docs, blogs and release notes
  - GitHub repos, releases and LICENSE files
  - the npm registry (`registry.npmjs.org/<pkg>`: `dist-tags` and `time`)
  - npm's official downloads API (`api.npmjs.org/downloads/point/last-week/<pkg>`, week 2026-10-01 to 2026-10-07)
  - `nodejs.org/dist/index.json`
- **Stars.** Star counts come from the GitHub REST API, or from repo pages where the API rate-limited us. Those are marked "~".
- **Speed figures** are **vendor claims**, not independent benchmarks.
- **Spot-checks.** The most surprising claims were re-fetched directly from their source pages:
  - Deno's runtime end of life
  - Bun's Rust rewrite
  - pnpm 12 in Rust
  - Vite+ 1.0
  - VoidZero joining Cloudflare
  - TypeScript 7 GA

## Landscape shifts since mid-2025 (read first)

These change the shortlist compared with what most write-ups describe.

- **Deno's runtime is being wound down.**
  - The post of 2026-10-09 (today) says: "We will support the Deno runtime for another year with monthly releases containing bug fixes and security updates. After that year we will end our development of the Deno runtime." https://deno.com/blog/cloudflare
  - Deno joins Cloudflare. Deno Deploy shuts down in 6 months. JSR continues on Cloudflare infrastructure. Deno stays open source. https://deno.com/blog/cloudflare , https://blog.cloudflare.com/deno-joins-cloudflare/
- **Bun is now written in Rust.**
  - "Bun v1.3.14 was the last version of Bun written in Zig. Bun v1.4.0 will be the first version of Bun written in Rust." (post dated 2026-07-08). https://bun.com/blog/bun-in-rust
  - Bun was acquired by Anthropic on 2025-12-02 and stays MIT. https://bun.com/blog/bun-joins-anthropic
- **pnpm 12 is a stable Rust rewrite.** It keeps pnpm 11's commands, settings and lockfile format. https://pnpm.io/blog/whats-different-in-pnpm-12
- **TypeScript 7.0 is GA** (2026-07-08). https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/
  - It is a Go port, shipped as the normal `typescript` package (`tsc`).
  - Its editor support is built on LSP.
  - It ships **no programmatic API** yet; a new, different API is expected in 7.1.
  - Vue, Svelte, Astro, MDX and Angular-template tooling still depends on TS 6.0.
- **The Vite ecosystem has consolidated.**
  - Vite 8 (2026-03-12) uses Rolldown as its only bundler. https://vite.dev/blog/announcing-vite8
  - Rolldown 1.0 has been stable since 2026-05-07. https://voidzero.dev/posts/announcing-rolldown-1-0
  - Vite+ 1.0 (2026-09-28, MIT) is one `vp` CLI that covers dev/build, test, lint, format, type-check, task running and Node management. https://voidzero.dev/posts/announcing-vite-plus-1-0
  - VoidZero is joining Cloudflare (2026-06-04). Vite, Vitest, Rolldown, Oxc and Vite+ stay MIT. https://voidzero.dev/posts/voidzero-cloudflare
- **Corepack is no longer shipped with Node.**
  - It has not been included since Node v25.0.0 (TSC vote 2025-03-19), and must now be installed with `npm i -g corepack`. https://github.com/nodejs/corepack/issues/722 , https://github.com/nodejs/node/pull/61207
  - Yarn 4's install docs still route through Corepack. https://yarnpkg.com/getting-started/install

## Tools that cover several toolchain roles

| Tool | Runtime | Package manager | Bundler / dev server | Test runner | Linter | Formatter | Type check |
|---|---|---|---|---|---|---|---|
| **Bun** 1.4 | yes | yes | yes (dev server "work in progress") | yes | – | – | `bun check` (TS 7) |
| **Deno** 2.9 (runtime EOL in about a year) | yes | yes | not researched | yes | yes | yes (dprint-based) | `deno check` |
| **Vite+** 1.0 (`vp`) | manages Node | wraps the chosen PM | Vite 8 / Rolldown | Vitest 5 | Oxlint | Oxfmt | `vp check` |
| **Biome** 2.5 | – | – | – | – | yes | yes | – |
| **Rstack** (ByteDance) | – | – | Rspack / Rsbuild | Rstest | Rslint | – | via plugin |
| **Node** 26 | yes | npm (bundled) | – | `node:test` | – | – | – |
| **pnpm** 12 | can install and pin Node, Deno or Bun | yes | – | – | – | – | – |

Sources:
- Bun: https://bun.com/blog/bun-v1.4 , https://bun.com/docs/runtime/check , https://bun.com/docs/bundler/fullstack
- Deno: https://docs.deno.com/runtime/reference/cli/
- Vite+: https://voidzero.dev/posts/announcing-vite-plus-1-0 , https://viteplus.dev/
- Biome: https://biomejs.dev/
- Rstack: https://rspack.rs/blog/announcing-2-0
- pnpm: https://pnpm.io/blog/releases/12.0

---

## 1. Runtime

| | Node.js | Bun | Deno |
|---|---|---|---|
| Latest | v26.11.1 Current (2026-10-07). LTS: v24.21.0 "Krypton", v22.23.3 "Jod" [1] | v1.4.2 (2026-09-05) [2] | v2.9.7 (2026-09-17) [3] |
| Adoption | 122k stars; Node 26 goes LTS on 2026-10-28 [4] | 96k stars; `bun` npm package ~3.75M/wk | 108.7k stars; **runtime development ends after one more year** [5] |
| Runs `.ts` directly | Type stripping only (erasable syntax); stable since v25.2 / v24.12 [6] | Transpiles on the fly, including `.tsx` [7] | Strips and runs; enums, namespaces and parameter properties work [8] |
| Type checking | None (use `tsc`) | `bun check` / `--check`, aiming to match TS 7 `tsc` output [9] | `deno check`; `deno test` type-checks by default [8] |
| Debug protocol | V8 Inspector / Chrome DevTools Protocol [10] | **WebKit** Inspector Protocol, not CDP [11] | V8 Inspector / CDP [12] |
| Install | Official binary or installer | Single dependency-free executable; macOS 13+ [13] | Single binary, no dependencies [14] |
| Licence | MIT | MIT, but statically links LGPL-2 JavaScriptCore [15] | MIT |
| Written in | C++ / JS | Rust (since 1.4) | Rust |

**Node.js**
- Release model:
  - New model: one major per year, every release becomes LTS, and version numbers align with the calendar year (27.0.0 in April 2027). https://nodejs.org/en/blog/announcements/evolving-the-nodejs-release-schedule
  - v20 reached end of life on 2026-04-30. https://github.com/nodejs/release
- Node 26.0.0 (2026-05-05): Temporal is on by default, and **`--experimental-transform-types` was removed**. https://nodejs.org/en/blog/release/v26.0.0
- Type-stripping limits (all from https://nodejs.org/api/typescript.html):
  - `enum`, runtime namespaces, parameter properties and import aliases fail with `ERR_UNSUPPORTED_TYPESCRIPT_SYNTAX`.
  - Decorators give a parse error, and **`.tsx` is unsupported**.
  - tsconfig is ignored, so `paths` does not work; use `#` subpath imports instead.
  - Imports need explicit `.ts` extensions.
  - TS files under `node_modules` are refused.
  - No source maps are generated.
  - Recommended tsconfig settings: `erasableSyntaxOnly`, `verbatimModuleSyntax`, `rewriteRelativeImportExtensions`.
- Inspector: `--inspect`, plus the `node:inspector` API, which supports all CDP domains declared by V8. https://nodejs.org/api/inspector.html

**Bun**
- Vendor claims for 1.4 vs 1.3: about 50% faster startup on Linux (5.1 ms vs 10.9 ms) and up to 35% lower memory. https://bun.com/blog/bun-v1.4
- `bun check` has no JSON mode. It prints `tsc`-style lines when output is not a TTY. https://bun.com/docs/runtime/check
- Debugging in VS Code is "experimental". The protocol definitions are in the `bun-inspector-protocol` package. https://bun.com/docs/runtime/debugger
- Install: curl script, Homebrew, or `npm i -g bun`. https://bun.com/docs/installation

**Deno**
- Vendor claims for 2.9 vs 2.8: cold start 1.98x faster, peak RSS 2.2x lower. 2.9 targets Node 26 compatibility. https://deno.com/blog/v2.9
- TS 7 (tsgo) is available behind `--unstable-tsgo`, described as "not yet feature-complete". https://docs.deno.com/runtime/fundamentals/typescript/
- Machine-readable output: https://docs.deno.com/runtime/reference/cli/info/
  - `deno info --json` (unstable), `deno doc --json`, `deno lint --json`, `deno bench --json`.
  - `deno test` has no JSON reporter, only JUnit or TAP.

**Other runtimes:** we found no other credible general-purpose local TS runtime. Cloudflare's workerd is a hosting runtime.

Runtime references:
- [1] https://nodejs.org/dist/index.json
- [2] https://github.com/oven-sh/bun/releases
- [3] https://github.com/denoland/deno/releases
- [4] https://github.com/nodejs/release
- [5] https://deno.com/blog/cloudflare
- [6] https://nodejs.org/api/typescript.html
- [7] https://bun.com/docs/runtime
- [8] https://docs.deno.com/runtime/fundamentals/typescript/
- [9] https://bun.com/docs/runtime/check
- [10] https://nodejs.org/api/inspector.html
- [11] https://bun.com/docs/runtime/debugger
- [12] https://docs.deno.com/runtime/fundamentals/debugging/
- [13] https://bun.com/docs/installation
- [14] https://docs.deno.com/runtime/getting_started/installation/
- [15] https://raw.githubusercontent.com/oven-sh/bun/main/LICENSE.md

---

## 2. Package manager

Per the glossary, Genea detects the package manager from the project's lockfile rather than blessing one.

| | npm | pnpm | Yarn Berry (4.x) | Bun | Deno | vlt |
|---|---|---|---|---|---|---|
| Latest | 12.2.0 (2026-09-30); Node 26.11.1 still bundles 11.20.0 | 12.11.1 (2026-10-09); 11.x maintained | 4.18.1 (2026-09-24); Yarn 6 (Rust, "zpm") still a canary | 1.4.2 | 2.9.7 | 1.3.8 (2026-10-09) |
| Weekly downloads | 16.3M | 210.6M (not explained; possibly inflated by its binary-fetcher packaging) | `yarn` (Classic 1.22.22) 9.0M; `@yarnpkg/cli-dist` 0.72M | n/a | n/a | 0.1M |
| Lockfile | `package-lock.json` (v3, JSON); `npm-shrinkwrap.json` **ignored by npm 12** | `pnpm-lock.yaml` | `yarn.lock` (+ `.pnp.cjs` under PnP) | `bun.lock` (text, default since 1.2); `bun.lockb` (legacy, binary) | `deno.lock` | `vlt-lock.json` |
| Workspaces | `workspaces`, `-w` / `--workspaces` | `pnpm-workspace.yaml`; rich `--filter` (deps/dependents, git-changed) | `workspaces`, `workspaces foreach -pt --since` | `workspaces`, `--filter` | `deno.json` `workspace` or package.json `workspaces` | `vlt.json` |
| Catalogs | none found | yes | yes (4.10+) | yes | yes (2.8+, npm deps only) | not verified |
| Needs Node | yes | **no** (standalone installer) | yes (JS CLI; Corepack route) | no | no | yes (22.22+) |
| JSON output | `npm ls --json` ("not supported by all commands") | `pnpm list --json` | `yarn workspaces list --json` (NDJSON), `npm info --json` | `bun pm licenses/diff --json`, `bun audit --json`; **no `--json` on `bun pm ls`** | – | `vlt query` (selector syntax) |
| Licence | Artistic-2.0 | MIT | BSD-2-Clause (Yarn 6: **GPL-3.0**) | MIT | MIT | BSD-2-Clause-Patent |

Sources:
- npm: https://registry.npmjs.org/npm , https://github.com/npm/cli/releases/tag/v12.0.0 , https://docs.npmjs.com/cli/v12/commands/npm-ls , https://docs.npmjs.com/cli/v12/configuring-npm/package-lock-json , https://raw.githubusercontent.com/npm/cli/latest/LICENSE
- pnpm: https://pnpm.io/installation , https://pnpm.io/filtering , https://pnpm.io/catalogs , https://pnpm.io/cli/list , https://pnpm.io/blog/releases/12.0
- Yarn: https://yarnpkg.com/features/workspaces , https://yarnpkg.com/features/catalogs , https://yarnpkg.com/cli/workspaces/list , https://v6.yarnpkg.com/ , https://raw.githubusercontent.com/yarnpkg/zpm/main/LICENSE.md
- Bun: https://bun.com/docs/pm/lockfile , https://bun.com/docs/pm/workspaces , https://bun.com/docs/pm/cli/pm
- Deno: https://docs.deno.com/runtime/reference/cli/install/ , https://docs.deno.com/runtime/fundamentals/workspaces/
- vlt: https://docs.vlt.sh/ , https://raw.githubusercontent.com/vltpkg/vltpkg/main/LICENSE
- Download counts: `api.npmjs.org/downloads/point/last-week/<pkg>`

Notes:
- **npm 12.0.0** (2026-07-08) https://github.com/npm/cli/releases/tag/v12.0.0
  - Requires Node `^22.22.2 || ^24.15.0 || >=26`.
  - Dependency lifecycle scripts are **blocked by default** (approve them with `npm install-scripts approve`).
  - Unknown configs and flags now throw.
- **pnpm vendor benchmark** ("alotta-files", clean install): npm 12.2.0 44.0 s, pnpm 12 4.29 s. Yarn and Bun are not in the current table. https://pnpm.io/benchmarks
- **pnpm 12 extras:** it can also install and pin Node, Deno and Bun runtimes. Homebrew does not offer it yet. https://pnpm.io/blog/releases/12.0
- **Bun PM 1.4:** https://bun.com/blog/bun-v1.4
  - Opt-in isolated linker with a global store.
  - `--filter` on add, remove and update.
  - Vendor claim: a T3-stack first install in 1.41 s vs 18.1 s for npm.
- **Lockfile migration:** both tools import other managers' lockfiles.
  - Bun auto-migrates from `yarn.lock`, `package-lock.json` and `pnpm-lock.yaml`. https://bun.com/docs/pm/lockfile
  - Deno 2.9 seeds `deno.lock` from any of the four. https://deno.com/blog/v2.9

**Lockfile detection map:**

| File | Package manager |
|---|---|
| `package-lock.json` | npm |
| `pnpm-lock.yaml` | pnpm |
| `yarn.lock` | Yarn (`.pnp.cjs` alongside means Berry with PnP) |
| `bun.lock` / `bun.lockb` | Bun |
| `deno.lock` | Deno |
| `vlt-lock.json` | vlt |

---

## 3. Bundler / dev server

| Tool | Latest (date) | Written in / install | Dev server + HMR | TypeScript | Licence | Stars |
|---|---|---|---|---|---|---|
| **Vite** | 8.3.4 (2026-10-08) | TS package; Rust core (Rolldown/Oxc); needs Node 20.19+/22.12+ | yes (`import.meta.hot`) | Oxc transpile, no type check | MIT | 83k |
| **Rolldown** | 1.2.13 (2026-10-07) | Rust; npm native binaries with a Wasm fallback | **no** ("use it through Vite") | Oxc | MIT | 14k |
| **Vite+** (`vp`) | 1.1.0 (2026-10-07) | Global install by script or Homebrew; prebuilt binaries; manages Node itself | yes (Vite 8) | Vite + `vp check` type-check | MIT | ~6k |
| **Rspack** | 2.2.8 (2026-09-28) | Rust core + TS; ESM-only, Node 20+ | via `@rspack/dev-server` / Rsbuild | `builtin:swc-loader`, transpile | MIT | 12.9k |
| **Rsbuild** | 2.2.12 (2026-10-06) | TS on Rspack; API runs on Node, Deno or Bun | yes | SWC; type check via plugin | MIT | 3.4k |
| **esbuild** | 0.28.2 (2026-08-08), pre-1.0 | Go; native binary on npm, or a Go library | serve + live reload; **no JS HMR** | strips types | MIT | 40k |
| **Bun bundler** | Bun 1.4.2 | inside the Bun executable | `Bun.serve` HTML imports + HMR ("work in progress") | transpile; optional `--check` | MIT (LGPL parts) | 96k |
| **Turbopack** | inside Next 16.4.0 | Rust, ships in `next` | Next.js only | SWC | MIT | – |
| **Parcel** | 2.16.4 (2026-02-02) | JS + Rust parts | yes | SWC, zero-config | MIT | 44k |
| **Farm** | 1.7.11 stable (2025-08); 2.0 in beta | Rust | yes | – | MIT | 5.6k |

Notes:
- **Vite**
  - Vendor claims: builds "10-30x faster"; 65M weekly downloads; Linear's build went from 46 s to 6 s. https://vite.dev/blog/announcing-vite8
  - 8.1 added an experimental Bundled Dev Mode, claimed about 15x faster startup on a 10k-component app. https://vite.dev/blog/announcing-vite8-1
  - "Vite only performs transpilation on `.ts` files and does NOT perform type checking". https://vite.dev/guide/features
- **Vite's machine interface** (Genea could drive it):
  - JS API `createServer` returns a `ViteDevServer` with `ws`, `moduleGraph`, `watcher`, `transformRequest`, `reloadModule` and `waitForRequestsIdle`. https://vite.dev/guide/api-javascript
  - Client HMR events include `vite:beforeUpdate`, `vite:error` and `vite:ws:connect`. https://vite.dev/guide/api-hmr
  - `searchForWorkspaceRoot` handles monorepos.
- **Rolldown**
  - Vendor claim: "10x-30x faster than Rollup", on par with esbuild.
  - The public API is locked under semver since 1.0.
  - https://voidzero.dev/posts/announcing-rolldown-1-0 , https://rolldown.rs/guide/getting-started
- **Vite+** https://voidzero.dev/posts/announcing-vite-plus-1-0 , https://viteplus.dev/guide/ , https://viteplus.dev/guide/ide-integration
  - Commands: `create`, `install`, `dev`, `check`, `test`, `build`, `pack`, `run`, `env`.
  - Vendor claim: about 2M weekly downloads.
  - "The tools underneath are written in Rust". Whether `vp` itself is a Rust binary is **not stated** (the repo contains Rust crates).
  - No JSON output modes are documented. IDE support goes through the Oxc and Vitest extensions and the oxlint/oxfmt language servers.
- **Rspack and Rsbuild**
  - Rspack 2.0 (2026-04-22) is a drop-in webpack replacement with 5M weekly downloads. It is used by Next.js, Nuxt, Nx, Storybook and Angular Rspack. https://rspack.rs/blog/announcing-2-0
  - Rsbuild JS API: `createRsbuild()`, then `startDevServer()` / `build()`. https://rsbuild.rs/api/start/
  - Vendor table: Rsbuild 1.36 s dev start / 160 ms HMR, against Vite 6.50 s / 130 ms. https://rsbuild.rs/
- **esbuild:** "esbuild does not do any type checking"; "Hot-reloading for JavaScript is not currently implemented". https://esbuild.github.io/content-types/ , https://esbuild.github.io/api/
- **Bun bundler** https://bun.com/docs/bundler , https://bun.com/docs/bundler/fullstack , https://bun.com/docs/bundler/hot-reloading
  - Produces a metafile JSON and supports `--compile` to an executable.
  - The full-stack dev server is "work in progress".
  - `import.meta.hot` is modelled on Vite's, but `invalidate` and `send` are not implemented.
- **Turbopack** is the default bundler in Next 16 and "built into Next.js". Standalone use is mentioned only as "in the future", so it is not a general-purpose candidate. https://nextjs.org/docs/app/api-reference/turbopack
- **Parcel:** no release since 2026-02. **Farm:** no stable release for over a year. Both are marginal.

---

## 4. Test runner

| Tool | Latest (date) | Install | TypeScript | Monorepo | Machine-readable interface | Licence |
|---|---|---|---|---|---|---|
| **Vitest** | 5.0.3 (2026-09-30) | npm; needs Node 22.12+ and Vite 6.4+ | out of the box via Vite (transpile) | `test.projects` (nested in 5.0) | reporters: json, junit, tap, blob, html, `agent`; **Node API** (`vitest/node`); official VS Code extension | MIT |
| **Jest** | 30.5.2 (2026-09-18) | npm, Node | Babel preset (no type check) or ts-jest; TS config needs ts-node or esbuild-register | `projects` | `--json --outputFile`, `--listTests`, `--testLocationInResults`, custom reporters | MIT |
| **Bun test** | Bun 1.4.2 | in the Bun binary | native; optional `--check` | no per-workspace test config documented | junit, dots, GitHub annotations; **no JSON reporter**; custom reporters via WebKit Inspector `TestReporter` domain | MIT |
| **node:test** | Node 26.11.1 | built into Node | type stripping (erasable syntax only, no `.tsx`) | – | spec, tap, dot, junit, lcov; `run()` returns an event stream (`test:pass`, `test:fail` and so on) | MIT |
| **Deno test** | Deno 2.9.7 | in the Deno binary | **type-checks by default** | workspace root runs all members | pretty, dot, junit, tap | MIT |
| **Rstest** | 0.12.3 (2026-09-30), pre-1.0 | npm, on Rspack | out of the box | `projects` | json, junit, blob, md; `rstest list --json`; VS Code extension | MIT |
| **Playwright** (E2E) | 1.64.0 (2026-10-07) | npm, Node | yes | Playwright projects | json, junit, blob, html; Reporter API; MCP | Apache-2.0 |

Notes:
- **Vitest 5.0** (2026-09-03) https://vitest.dev/blog/vitest-5
  - Vendor claim: up to −53% time against 4.1 on a dependency-heavy suite.
  - New `vitest.createReport` reporter API, Trace View, nested projects.
  - Browser Mode has been stable since 4.0. https://vitest.dev/blog/vitest-4
- **Vitest Node API and editor integration**
  - Node API: `createVitest`, then `start`, `runTestSpecifications`, `rerunTestSpecifications`, `watcher`, `close`. https://vitest.dev/api/advanced/vitest
  - The VS Code extension spawns a Vitest child process, or uses a WebSocket in terminal mode. https://github.com/vitest-dev/vscode
  - Reporter list: https://vitest.dev/guide/reporters
- **Jest:** the last major is Jest 30 (2025-06-04). "Jest will not type-check your tests". https://jestjs.io/blog , https://jestjs.io/docs/getting-started , https://jestjs.io/docs/cli
- **Bun test**
  - Jest-compatible API, but "not everything is implemented".
  - Produces quieter output when it detects an agent environment (`CLAUDECODE`, `AGENT`).
  - Vendor claim: runs "266 React SSR tests faster than Jest can print its version number".
  - https://bun.com/docs/test , https://bun.com/docs/test/reporters
- **node:test:** stable since v20, but watch mode and coverage are still experimental, and each file runs in its own process. https://nodejs.org/api/test.html
- **Deno test:** `--no-check` skips type checking; it also runs `node:test` tests and supports `--changed` / `--related`. https://docs.deno.com/runtime/reference/cli/test/ , https://docs.deno.com/runtime/fundamentals/testing/
- **Rstest:** Jest-compatible and still in "active development". https://rstest.rs/ , https://rstest.rs/config/test/reporters , https://rstest.rs/guide/basic/cli
- **Playwright:** the experimental component-testing packages were removed in favour of a built-in `mount` fixture. That change is worth re-checking. https://playwright.dev/docs/test-components , https://playwright.dev/docs/test-reporters , https://github.com/microsoft/playwright/releases

---

## 5. Linter / formatter

| Tool | Latest (date) | Covers | Written in / install | Needs Node? | Type-aware lint | Monorepo | Machine interface | Beyond TS/JS | Licence |
|---|---|---|---|---|---|---|---|---|---|
| **ESLint 10 + typescript-eslint 8** | 10.12.0 (2026-10-02); tseslint 8.71.1 (2026-10-05) | lint | JS, npm | yes (`^20.19 \|\| ^22.13 \|\| >=24`) | yes, via the TS compiler API; **TS `<6.1` only, no TS 7 yet** | config looked up per file's directory (v10) | json formatters; Node API (`ESLint` class); LSP via the vscode-eslint server; SARIF via a 2024 Microsoft formatter | JSON, CSS, Markdown (official plugins); HTML (community) | MIT |
| **Prettier** | 3.9.9 (2026-09-23) | format | JS, npm | yes | n/a | nearest config + `overrides` | async Node API; **no first-party LSP** | CSS/SCSS/Less, HTML, JSON, GraphQL, Markdown, YAML, Vue, Angular | MIT |
| **Biome** | 2.5.15 (2026-09-30) | lint + format | Rust, **single binary** | no | yes, its **own type inference, no TS compiler** (`noFloatingPromises` catches ~75% of typescript-eslint's cases) | nested `biome.json` (`"root": false`, `"extends": "//"`) | `biome lsp-proxy` + daemon; reporters json (experimental), sarif, github, gitlab, junit, checkstyle, rdjson; `@biomejs/js-api` | JSON/JSONC, CSS, GraphQL; HTML experimental; YAML and Markdown in progress | MIT OR Apache-2.0 |
| **Oxlint** (+ tsgolint) | 1.87.0 (2026-10-05), weekly releases; stable since 1.0 (2025-06) | lint | Rust, standalone binaries; tsgolint is Go | core no; JS plugins unclear | yes, **stable since 2026-07-22**, via tsgolint on TS 7; 59/61 typescript-eslint typed rules; **requires TS 7** | nearest config, not merged | `oxlint --lsp`; `--format` json, sarif, github, gitlab, junit, checkstyle, agent | only `<script>` blocks of Vue, Svelte, Astro | MIT |
| **Oxfmt** | 0.72.0 (2026-10-05), **beta** | format | Rust; npm or standalone binary | only for formats it hands to Prettier | n/a | nearest config + `overrides` | `oxfmt --lsp`, `--stdin-filepath`, Node `format()` | native: JSON, CSS/SCSS/Less, GraphQL, TOML, YAML, Markdown; HTML/Vue/Svelte/MDX through bundled Prettier (needs Node) | MIT |
| **dprint** | 0.61.1 (2026-10-07) | format (plugin platform) | Rust + Wasm plugins, single binary | no (except the Prettier process plugin) | n/a | descendant configs, `"inherit": true` | `dprint lsp`, `check --json` (NDJSON), `--stdin` | via plugins: JSON, Markdown, TOML, CSS, HTML, YAML, GraphQL | MIT |
| **deno lint / fmt** | Deno 2.9.7 | lint + format | Rust, single binary | no | none documented | Deno workspaces | `deno lint --json`, `deno fmt -`, `deno lsp` | fmt: JSON, Markdown, CSS, HTML, YAML; lint covers JS/TS only | MIT |

No tool documents `.env` support.

Notes:
- **ESLint**
  - v10 (2026-02-06) removed eslintrc; v9 reached end of life on 2026-08-06. https://eslint.org/blog/2026/02/eslint-v10.0.0-released/ , https://eslint.org/blog/
  - Node API: `lintFiles`, `lintText` and `outputFixes`, with messages that carry `fix` and `suggestions`, plus a `concurrency` option. https://eslint.org/docs/latest/integrate/nodejs-api
  - By default it lints only `.js/.cjs/.mjs`. https://eslint.org/docs/latest/use/configure/configuration-files
- **typescript-eslint**
  - 136 rules, about 63 of them type-checked. https://typescript-eslint.io/rules/
  - Supported TS range: `>=4.8.4 <6.1.0`. https://typescript-eslint.io/users/dependency-versions
  - TS 7 support is tracked in issue #10940. As of 2026-09-29 it was work in progress, with "not expecting performance to be significantly better with TS7" for the first version. https://github.com/typescript-eslint/typescript-eslint/issues/10940
- **Prettier**
  - 4.0 is still alpha (last published 2025-11). https://registry.npmjs.org/prettier
  - The fast CLI is still experimental. https://prettier.io/blog/2026/06/27/3.9.0
  - API reference: https://prettier.io/docs/api
- **Biome**
  - Vendor claims: 568 rules, "97% compatibility with Prettier", and "~35x" faster than Prettier. https://biomejs.dev/
  - Plugins are GritQL only. https://biomejs.dev/linter/plugins/
  - 2026 roadmap: SCSS, then stabilising YAML and HTML; Markdown has no champion. https://biomejs.dev/blog/roadmap-2026/
  - Further references: language support https://biomejs.dev/internals/language-support/ , reporters https://biomejs.dev/reference/reporters/ , building an editor extension https://biomejs.dev/guides/editors/create-an-extension/
- **Oxlint**
  - 871 rules; vendor claim "50 to 100 times faster than ESLint". https://oxc.rs/docs/guide/usage/linter.html
  - Type-aware vendor claim: 12–18x faster than ESLint + typescript-eslint. https://oxc.rs/blog/2026-07-22-type-aware-linting-stable
  - JS plugins (ESLint-v9-compatible API) are **alpha**. https://oxc.rs/docs/guide/usage/linter/js-plugins.html
  - Output formats: https://oxc.rs/docs/guide/usage/linter/output-formats.html
- **Oxfmt**
  - Vendor claims: passes 100% of Prettier's JS/TS conformance tests; >30x faster than Prettier and 3x faster than Biome.
  - Adopters include vuejs/core and turborepo.
  - Import sorting and Tailwind class sorting are built in.
  - https://oxc.rs/blog/2026-02-24-oxfmt-beta , https://oxc.rs/docs/guide/usage/formatter/language-support.html
- **dprint:** https://dprint.dev/plugins/ , https://dprint.dev/cli/ , https://dprint.dev/config/
- **deno lint / fmt:** https://docs.deno.com/runtime/reference/cli/lint/ , https://docs.deno.com/runtime/reference/cli/fmt/

### Related: TypeScript 7 and language intelligence

This affects both type-aware linting and Genea's own language intelligence.

- **Release and install**
  - TS 7.0 shipped on 2026-07-08 as the `typescript` package, giving a `tsc` binary.
  - Vendor claim: about 10x faster builds.
  - `@typescript/typescript6` (binary `tsc6`) lets TS 6 run alongside it.
  - Source: https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/
- **Editor support and API**
  - Editor support is "built on the Language Server Protocol".
  - There is no API until 7.1.
  - The post advises TS 7 for CLI checks and TS 6.0 for editor support where Vue, Svelte, MDX or Astro are used.
  - Source: https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/
- **LSP entry point**
  - The source dispatches `--lsp` (stdio only). https://raw.githubusercontent.com/microsoft/typescript-go/main/cmd/tsgo/main.go
  - The typescript-go repo was archived on 2026-09-01, and development moved to microsoft/TypeScript. https://github.com/microsoft/typescript-go
  - That classic `tsserver.js` is gone in TS 7 is **unverified** on a primary source.
- **Effect on linters**
  - Oxlint's type-aware mode needs TS 7.
  - typescript-eslint needs TS below 6.1.
  - Biome needs neither.

---

## 6. Template frameworks

### Backend HTTP framework

| Framework | Latest stable (date) | Next | Stars | npm/wk | Types | Runtimes | Basis | Licence |
|---|---|---|---|---|---|---|---|---|
| **Hono** | 4.13.13 (2026-10-04) | 5.0.0-rc.0 (2026-10-08) | 32.5k | 73.1M | written in TS | Workers, Deno, Bun, Lambda, Fastly, Node (via `@hono/node-server`) | Web Standards | MIT |
| **Fastify** | 5.12.5 (2026-09-16) | 6.0.0-alpha.4 | 37.2k | 15.2M | JS + bundled `.d.ts`; type providers (TypeBox, Zod) | Node only (docs) | Node http | MIT |
| **Express** | 5.3.0 (2026-10-09) | – | 69.6k | 141.0M | `@types/express` only | Node 18+ | Node http | MIT |
| **Elysia** | 1.4.30 (2026-08-26); 1.4.x gets security fixes only | 2.0.0-beta.27 | 19.2k | 1.54M | bundled; peer dependency on `@types/bun` | Bun first; Node via adapter; Deno, Workers | Web Standards | MIT |
| **NestJS** | 12.1.2 (2026-09-30) | – | 76.8k | 16.0M | written in TS (decorators) | Node 20.19+ / 22.12+ | on Express or Fastify | MIT |
| **Koa** | 3.2.1 (2026-05-21) | – | 35.7k | 7.54M | `@types/koa` only | Node 18+ | Node http | MIT |

Sources: npm registry, the downloads API, GitHub repo pages, plus:
- Hono: https://hono.dev/docs/ , https://github.com/honojs/hono/releases
- Fastify: https://fastify.dev/docs/latest/Reference/TypeScript/
- Express: https://expressjs.com/en/guide/migrating-5.html
- Elysia: https://elysiajs.com/at-glance.html , https://github.com/elysiajs/elysia/releases
- NestJS: https://github.com/nestjs/nest/releases , https://docs.nestjs.com/first-steps
- Koa: https://koajs.com/

**Sharing types between frontend and backend** (relevant to the full-stack workspace template):
- **Hono RPC:** the server exports `AppType`, and the client is `hc<AppType>()`. https://hono.dev/docs/guides/rpc
  - The docs advise identical Hono versions across packages, `strict: true`, and TS project references.
  - Very many routes can slow the IDE.
- **Elysia Eden Treaty:** `treaty<App>()`, with no codegen. https://elysiajs.com/eden/overview
- **tRPC 11.19.0** (5.43M/wk): adapters for Node, Express, Fastify and Fetch/edge. https://trpc.io/docs , https://trpc.io/docs/server/adapters
- **oRPC 1.15.5** (1.49M/wk; 2.0 in beta): supports Standard Schema, can serve the same router as RPC or OpenAPI, and integrates with NestJS. https://orpc.dev/docs/getting-started
- Fastify, Express, Koa and NestJS have no built-in client type sharing.

**Speed (vendor claims)**
- Fastify's own benchmark, Node single instance, req/s (updated 2026-10-01): https://fastify.dev/benchmarks/
  - Fastify 50,373
  - Hono 45,102
  - Koa 38,917
  - Express 27,474
- Hono claims the "fastest router in the JavaScript world". https://hono.dev/docs/concepts/benchmarks
- Elysia's Bun figures date from 2023. https://elysiajs.com/at-glance.html

**Other notes**
- NestJS 12 (2026-08-27) is ESM-only, and its CLI uses Rspack and oxlint in generated projects. https://github.com/nestjs/nest/releases

### Frontend UI framework

| Framework | Latest stable (date) | Next | Stars | npm/wk | Build need | Own language server? | Licence |
|---|---|---|---|---|---|---|---|
| **React** | 19.3.0 (2026-09-09) | – | 250.8k | 186.3M | standard JSX transform; Compiler optional | **no** (TSX) | MIT |
| **Vue** | 3.5.43 (2026-09-17) | 3.6.0-rc.10 (Vapor Mode) | 54.6k | 16.9M | SFC compiler (`@vitejs/plugin-vue`) | **yes** for `.vue` (Volar / `@vue/language-server`) | MIT |
| **Svelte** | 5.57.2 (2026-10-06) | – | 88.4k | 6.45M | Svelte compiler (`@sveltejs/vite-plugin-svelte`) | **yes** for `.svelte` (`svelte-language-server`) | MIT |
| **SolidJS** | 1.9.17 (2026-10-07) | 2.0.0-rc.14 (2026-10-08) | 36.1k | 6.78M | own JSX compiler (`vite-plugin-solid`) | no (TSX, `jsxImportSource`) | MIT |
| **Preact** | 11.0.1 (2026-10-08) | – | 38.9k | 37.1M | standard JSX transform; "no special compiler" | no (TSX) | MIT |
| **Angular** | 22.2.2 (2026-10-08) | 22.3.0-next | 101.0k | 6.03M | Angular CLI (esbuild + Vite dev server, not configurable) | **yes** for templates (Angular Language Service) | MIT |

**Why this matters for Genea:** Genea's first-class languages are TS, TSX, JS and JSX. Vue, Svelte and Angular put code in files or templates that need their own language server. TS 7 does not support them yet; they stay on TS 6.0. https://devblogs.microsoft.com/typescript/announcing-typescript-7-0/

React, Preact and Solid stay within TSX.

Notes:
- **React**
  - Hosted by the React Foundation under the Linux Foundation since 2026-02-24. https://react.dev/blog
  - React Compiler 1.0 has been stable since 2025-10-07. Vendor claim: up to 12% faster loads. https://react.dev/blog/2025/10/07/react-compiler-1
  - With Vite it runs via `@vitejs/plugin-react` 6+ and `@rolldown/plugin-babel`. https://react.dev/learn/react-compiler/installation
  - Types come from `@types/react`. https://react.dev/learn/typescript
- **Vue**
  - "Written in TypeScript". https://vuejs.org/guide/typescript/overview
  - Type-checking uses `vue-tsc`.
  - TSX is possible via `jsxImportSource: vue`.
  - 3.6's Vapor Mode is opt-in and feature-complete. https://raw.githubusercontent.com/vuejs/core/minor/CHANGELOG.md
- **Svelte:** `<script lang="ts">` strips types only, and type-checking uses `svelte-check`. https://svelte.dev/docs/svelte/typescript , https://github.com/sveltejs/language-tools
- **Solid:** "Solid is written in TypeScript". Its JSX transform is incompatible with TypeScript's, so builds need Solid's compiler. https://docs.solidjs.com/configuration/typescript
- **Preact 11** (2026-09-30) is ESM-only and needs TS 5.1+. https://preactjs.com/guide/v11/upgrade-guide , https://preactjs.com/guide/v11/typescript
- **Angular 22** (2026-06-03) https://angular.dev/reference/versions , https://angular.dev/tools/cli/build-system-migration , https://angular.dev/tools/language-service
  - Needs TS ≥6.0 <6.1.
  - Components default to OnPush.
  - Zoneless has been the default since v21.

**Meta-frameworks** (only relevant if templates use one)

| Meta-framework | Version | npm/wk | Notes |
|---|---|---|---|
| Next.js | 16.4.0 | 65.5M | Turbopack by default. https://nextjs.org/blog |
| React Router | 8.4.0 | 56.5M | v8 is the former "v7 framework mode": Node 22.22+, Vite 7+, ESM-only. https://raw.githubusercontent.com/remix-run/react-router/main/CHANGELOG.md |
| TanStack Start | 1.168.60 | 11.2M | Docs still say "Release Candidate"; runs on Vite or Rsbuild. https://tanstack.com/start/latest/docs/framework/react/overview |
| Nuxt | 4.6.0 | 2.35M | https://nuxt.com/blog |
| SvelteKit | 3.0.1 | 3.15M | 3.0 (2026-10-01) moves config into `vite.config.ts`. https://svelte.dev/blog/sveltekit-3-is-here |

---

## Gaps and unverified items

**Package managers**
- Whether Yarn 4 needs Node to run. It is a JS CLI, so assumed yes.
- Whether pnpm 12's standalone install is a single file.
- Why pnpm's download count is so high.
- The `pnpm-lock.yaml` `lockfileVersion` value.
- Whether `deno.lock` is JSON.

**Bundlers and test runners**
- Whether `vp` (Vite+) is itself a Rust binary.
- Whether Turbopack has any standalone release.

**Linters, formatters and TypeScript**
- Whether Oxlint has a programmatic API.
- Whether Oxlint JS plugins need Node.
- dprint's dedicated LSP docs page returned 404.
- That `tsserver` is absent from TS 7.
- Where the TS 7 LSP source lives after the move to microsoft/TypeScript.
- Release dates for TS 7.1, Oxfmt 1.0, Vue 3.6, Solid 2.0 and Elysia 2.0.

**Frameworks**
- The Hono v5 breaking-change list (the release page failed to load).
- NestJS's decorator tsconfig flags.

**Data quality**
- Some GitHub star counts come from page scrapes because of API rate limits.
