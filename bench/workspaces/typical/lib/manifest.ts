// The shape of the Typical reference workspace: its packages, their exact
// dependency pins and the targets its figures must hit (#19, Benchmark harness).
//
// Changing anything in `ROOT` or `PACKAGES` that ends up in a package.json
// changes what the committed lockfile must contain: run
// `typical/update-lockfile.sh` afterwards and commit the new lockfile.

export const WORKSPACE_NAME = "typical";
export const SCOPE = "@typical";

/** Toolchain pins (ADR 0005): exact versions in package.json. */
export const PNPM_VERSION = "11.13.0";
export const NODE_VERSION = "24.18.0";

/** Targets from #19: ~5 packages, ~2k TS files, ~300k LOC, ~150k node_modules files. */
export const TARGETS = {
  packages: 5,
  tsFiles: 2000,
  loc: 300_000,
  nodeModulesFiles: 150_000,
} as const;
export const TOLERANCE = 0.1;

/**
 * Dependencies with install scripts. None of them is needed for editing or
 * type-checking, and skipping them keeps node_modules identical across runs.
 * pnpm 11 refuses to leave them unlisted.
 */
export const DISALLOWED_BUILDS = ["@firebase/util", "core-js", "esbuild", "protobufjs"] as const;

export type Deps = Readonly<Record<string, string>>;

export type PackageKind = "shared" | "ui" | "web" | "api";

export interface PackageSpec {
  /** Directory under packages/ and the unscoped package name. */
  readonly name: PackageKind;
  readonly description: string;
  /** Number of TS source files under src/ (including index.ts). */
  readonly files: number;
  /** Share of source files that are .tsx. */
  readonly tsxShare: number;
  /** Workspace packages this one imports from. */
  readonly workspaceDeps: readonly PackageKind[];
  readonly dependencies: Deps;
  readonly devDependencies: Deps;
  /** TS compiler options specific to this package. */
  readonly compilerOptions: Readonly<Record<string, unknown>>;
  readonly scripts: Readonly<Record<string, string>>;
}

export const ROOT_DEV_DEPENDENCIES: Deps = {
  "@faker-js/faker": "10.6.0",
  "@playwright/test": "1.64.0",
  "@types/node": "26.6.5",
  "aws-cdk-lib": "2.273.0",
  constructs: "10.8.1",
  oxfmt: "0.72.0",
  oxlint: "1.87.0",
  typescript: "7.0.2",
  vitest: "5.0.3",
};

const domLib = { lib: ["ES2023", "DOM", "DOM.Iterable"], jsx: "react-jsx", types: [] };

export const PACKAGES: readonly PackageSpec[] = [
  {
    name: "shared",
    description: "Domain types, schemas and utilities shared by the web app and the API.",
    files: 400,
    tsxShare: 0,
    workspaceDeps: [],
    dependencies: {
      "date-fns": "4.4.0",
      effect: "4.0.2",
      "libphonenumber-js": "1.13.15",
      "lodash-es": "4.18.1",
      zod: "4.6.5",
    },
    devDependencies: {
      "@types/lodash-es": "4.17.12",
    },
    compilerOptions: { lib: ["ES2023"], types: [] },
    scripts: { typecheck: "tsc --noEmit", test: "vitest run" },
  },
  {
    name: "ui",
    description: "The design system: React components on top of MUI.",
    files: 450,
    tsxShare: 0.8,
    workspaceDeps: ["shared"],
    dependencies: {
      "@emotion/react": "11.14.0",
      "@emotion/styled": "11.14.1",
      "@mui/icons-material": "9.5.0",
      "@mui/material": "9.5.0",
      "@tabler/icons-react": "3.49.0",
      "lucide-react": "1.54.0",
      react: "19.3.0",
      "react-dom": "19.3.0",
    },
    devDependencies: {
      "@storybook/react-vite": "10.6.1",
      "@types/react": "19.3.0",
      "@types/react-dom": "19.3.0",
      storybook: "10.6.1",
    },
    compilerOptions: domLib,
    scripts: { typecheck: "tsc --noEmit", test: "vitest run", storybook: "storybook dev" },
  },
  {
    name: "web",
    description: "The single-page web app.",
    files: 650,
    tsxShare: 0.6,
    workspaceDeps: ["shared", "ui"],
    dependencies: {
      "@emotion/react": "11.14.0",
      "@emotion/styled": "11.14.1",
      "@mui/material": "9.5.0",
      "@mui/x-data-grid": "9.15.0",
      "@mui/x-date-pickers": "9.15.0",
      "@sentry/react": "11.6.0",
      "@tanstack/react-query": "5.104.1",
      "@tanstack/react-router": "1.170.41",
      "core-js": "3.50.0",
      "date-fns": "4.4.0",
      firebase: "13.0.0",
      "highlight.js": "11.12.0",
      i18next: "26.4.2",
      react: "19.3.0",
      "react-dom": "19.3.0",
      "react-i18next": "17.0.16",
    },
    devDependencies: {
      "@types/react": "19.3.0",
      "@types/react-dom": "19.3.0",
      "@vitejs/plugin-react": "6.1.2",
      vite: "8.3.4",
    },
    compilerOptions: domLib,
    scripts: { dev: "vite", build: "vite build", typecheck: "tsc --noEmit", test: "vitest run" },
  },
  {
    name: "api",
    description: "The HTTP API and background jobs.",
    files: 500,
    tsxShare: 0,
    workspaceDeps: ["shared"],
    dependencies: {
      "@aws-sdk/client-dynamodb": "3.1148.0",
      "@aws-sdk/client-s3": "3.1148.0",
      "@aws-sdk/client-sqs": "3.1148.0",
      "@hono/node-server": "2.1.4",
      "@opentelemetry/auto-instrumentations-node": "0.81.0",
      "@opentelemetry/sdk-node": "0.223.0",
      "@sentry/node": "11.6.0",
      "drizzle-orm": "0.45.4",
      googleapis: "185.0.0",
      hono: "4.13.13",
      pg: "8.23.1",
      pino: "10.4.0",
      rxjs: "7.8.2",
      zod: "4.6.5",
    },
    devDependencies: {
      "@types/pg": "8.23.1",
    },
    compilerOptions: { lib: ["ES2023"], types: ["node"] },
    scripts: {
      dev: "node --watch src/index.ts",
      start: "node src/index.ts",
      typecheck: "tsc --noEmit",
      test: "vitest run",
    },
  },
];
