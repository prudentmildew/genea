// Writes the Typical reference workspace: manifests, configs, the committed
// lockfile and the generated TS sources. Output depends only on this code and
// the committed lockfile, never on the clock, the environment or the output path.

import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import {
  DISALLOWED_BUILDS,
  NODE_VERSION,
  PACKAGES,
  PNPM_VERSION,
  ROOT_DEV_DEPENDENCIES,
  SCOPE,
  WORKSPACE_NAME,
  type PackageSpec,
} from "./manifest.ts";
import { generateSources } from "./sources.ts";

/** The frozen lockfile, committed next to the generator. */
export const LOCKFILE_PATH = fileURLToPath(new URL("../pnpm-lock.yaml", import.meta.url));

export type FileMap = Map<string, string>;

const json = (value: unknown): string => JSON.stringify(value, null, 2) + "\n";

/** package.json contents for the root and every workspace package, by relative path. */
export function manifests(): Map<string, Record<string, unknown>> {
  const out = new Map<string, Record<string, unknown>>();
  out.set("package.json", {
    name: WORKSPACE_NAME,
    private: true,
    type: "module",
    packageManager: `pnpm@${PNPM_VERSION}`,
    devEngines: { runtime: { name: "node", version: NODE_VERSION } },
    scripts: {
      typecheck: "tsc -b --noEmit",
      lint: "oxlint",
      format: "oxfmt",
      test: "vitest run",
    },
    devDependencies: ROOT_DEV_DEPENDENCIES,
  });
  for (const pkg of PACKAGES) {
    out.set(`packages/${pkg.name}/package.json`, packageManifest(pkg));
  }
  return out;
}

function packageManifest(pkg: PackageSpec): Record<string, unknown> {
  const workspace = Object.fromEntries(pkg.workspaceDeps.map((d) => [`${SCOPE}/${d}`, "workspace:*"]));
  return {
    name: `${SCOPE}/${pkg.name}`,
    version: "0.0.0",
    private: true,
    description: pkg.description,
    type: "module",
    exports: { ".": "./src/index.ts" },
    scripts: pkg.scripts,
    dependencies: { ...workspace, ...pkg.dependencies },
    devDependencies: pkg.devDependencies,
  };
}

/** Every file of the workspace except the lockfile, by relative path. */
export function workspaceFiles(): FileMap {
  const files: FileMap = new Map();
  for (const [path, manifest] of manifests()) files.set(path, json(manifest));

  files.set(
    "pnpm-workspace.yaml",
    [
      "packages:",
      "  - packages/*",
      "",
      "# Keep installs reproducible: no release-age gate rewriting this file,",
      "# and no dependency build scripts.",
      "minimumReleaseAge: 0",
      "allowBuilds:",
      ...DISALLOWED_BUILDS.map((name) => `  '${name}': false`),
      "",
    ].join("\n"),
  );
  files.set(".gitignore", ["node_modules/", "dist/", "*.tsbuildinfo", ""].join("\n"));
  files.set(
    "tsconfig.base.json",
    json({
      compilerOptions: {
        target: "ES2023",
        module: "NodeNext",
        moduleResolution: "NodeNext",
        moduleDetection: "force",
        strict: true,
        noUncheckedIndexedAccess: true,
        noImplicitOverride: true,
        verbatimModuleSyntax: true,
        allowImportingTsExtensions: true,
        isolatedModules: true,
        skipLibCheck: true,
        noEmit: true,
      },
    }),
  );
  // One program over every package, so `tsc -b --noEmit` at the root checks
  // the whole workspace. TS 7 refuses `-b --noEmit` across project references.
  files.set(
    "tsconfig.json",
    json({
      extends: "./tsconfig.base.json",
      compilerOptions: { lib: ["ES2023", "DOM", "DOM.Iterable"], jsx: "react-jsx", types: ["node"] },
      include: PACKAGES.map((p) => `packages/${p.name}/src`),
    }),
  );
  files.set(
    ".oxlintrc.json",
    json({ categories: { correctness: "error" }, ignorePatterns: ["dist"] }),
  );
  files.set(".oxfmtrc.json", json({ printWidth: 100 }));

  for (const pkg of PACKAGES) {
    const dir = `packages/${pkg.name}`;
    files.set(
      `${dir}/tsconfig.json`,
      json({ extends: "../../tsconfig.base.json", compilerOptions: pkg.compilerOptions, include: ["src"] }),
    );
  }
  for (const [path, text] of generateSources()) files.set(path, text);
  return files;
}

/** Writes the Typical workspace into `outDir` (created if missing). */
export function generateTypical(outDir: string): void {
  const files = workspaceFiles();
  files.set("pnpm-lock.yaml", readFileSync(LOCKFILE_PATH, "utf8"));
  for (const [path, text] of [...files].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0))) {
    const target = join(outDir, path);
    mkdirSync(dirname(target), { recursive: true });
    writeFileSync(target, text);
  }
}
