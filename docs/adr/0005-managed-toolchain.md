# Genea manages runtimes and package managers per project

Genea expects nothing on the user's machine. It downloads the runtime (Node, or Bun) and the package manager (pnpm or Bun) itself, at the versions the project pins, and puts them first on PATH for every process it starts, including the integrated terminal's shell. The pins live in `package.json`: `devEngines.runtime` for the runtime and `packageManager` for the package manager. Genea writes them through in-app pickers, always as exact versions. Tools that are tied to a project's config (TypeScript 7, Oxlint, Oxfmt) come from the project's own `node_modules`, and Genea never brings its own copy. We chose this because Genea is a personal daily driver: every project should carry its whole toolchain, and nothing should depend on Homebrew, version managers or the Xcode Command Line Tools.

## Considered Options

- **Expect tools on PATH**, resolved from the user's login shell, with version managers (mise, nvm, fnm, Volta) in charge: rejected. That makes the project depend on whatever the machine has installed, and it's an external dependency.
- **Pins in Genea's config**: rejected. pnpm, Bun and CI already read the `package.json` fields, so a second copy in the config would drift from them.
- **Let pnpm download Node** (`devEngines.runtime` with `onFail: "download"`): rejected. pnpm can't do it for Bun projects, and Genea needs a runtime before any install, to run the Oxlint and Oxfmt servers.
- **A Genea-owned TypeScript 7, Oxlint or Oxfmt as a fallback**: rejected. Removed options like `baseUrl` are hard errors in TS 7, and `.oxlintrc.json` rejects keys a binary doesn't know, so a version the project didn't choose reports problems that aren't real. The standalone Oxc binaries also drop features without saying so: Oxfmt has no `--lsp` and skips HTML, and Oxlint ignores `jsPlugins`.

## Consequences

- **Downloads** are checksum-verified and go into one shared store in `~/Library/Application Support/Genea/`. They start automatically on project open, because downloading runs no project code. Node comes from nodejs.org, Bun from its GitHub releases, and pnpm from npm's `@pnpm/exe` package, since pnpm's GitHub releases publish no checksums. A failed download turns that role's features off and shows a notice with Retry.
- **Unpinned projects** use Genea's built-in default versions, which move forward with Genea releases. A notice offers to pin them. Genea never writes to `package.json` without a click, and never moves a pin by itself: "Update toolchain…" does that, when the user picks a version.
- **Range pins** resolve to the newest matching version in the store, or to the newest matching download. They are valid but not reproducible. Genea ignores pnpm's lockfile entry for the runtime.
- **Missing project tools turn features off.** Without TypeScript 7, language intelligence is off and a notice offers to add it. Without Oxlint and Oxfmt in `package.json`, lint and format are off and a notice offers to add them.
- **Installing dependencies** only happens when the user clicks "Install dependencies", and it runs visibly in the terminal, because install scripts run project code. The one exception is creating a project from a template: the install starts by itself, in the terminal, because creating the project was the click.
- **The terminal sees the pinned versions.** Pinned tools come first on PATH. The user's login shell, run in the project root, supplies only environment variables. A shell rc file that activates a version manager can push its own versions back in front, and on this machine the fix is to remove that from the rc file.
- **A Bun runtime pin** means Bun runs everything, including the Oxlint and Oxfmt servers and any script that calls `node`. Where Bun can't stand in for Node, that is a limit of choosing Bun.
- **Git** is done in-process through a crate, because the `git` CLI on macOS needs the Xcode Command Line Tools.
