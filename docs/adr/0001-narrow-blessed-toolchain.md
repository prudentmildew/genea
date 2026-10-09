# Narrow blessed toolchain

Genea blesses one tool per toolchain role and allows almost no alternatives. The runtime is Node, and Bun is the only alternative the config can pick. The package manager is detected from a pnpm or Bun lockfile, with pnpm as the default. The linter/formatter is Oxlint + Oxfmt, with no alternative. Templates use Vite 8 and Vitest 5. Projects that use any other tool for a role Genea drives get that role's features turned off rather than supported. We chose this so that each role has exactly one integration to build, test and keep fast, which matters more for a personal daily driver than reach. We accept two costs: Oxfmt is still beta, and npm, Yarn, ESLint and Prettier projects open in a reduced mode.

## Considered Options

- **Biome, or ESLint + Prettier, as linter/formatter alternatives**: rejected. Each would be a second LSP integration to build, and typescript-eslint doesn't run on TS 7.
- **npm and Yarn lockfiles**: rejected. Yarn PnP can't work with the Go-based TS 7, and npm would add a third package manager to support for little gain.
- **Blessing Vite+ (`vp`) as one tool covering several roles**: rejected. It is newer than its parts and breaks "one tool per toolchain role". A project that uses it still works, because its parts are the blessed tools.

## Consequences

- Widening the allowlist later is cheap. Narrowing it after projects depend on an alternative is not, which is why it starts narrow.
- Oxlint's type-aware linting needs TS 7, so it is tied to how Genea gets its language intelligence.
