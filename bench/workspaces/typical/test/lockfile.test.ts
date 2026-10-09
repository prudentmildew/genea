import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { lockfileDrift } from "../lib/lockfile.ts";
import { LOCKFILE_PATH, manifests } from "../lib/workspace.ts";

const LOCKFILE = `lockfileVersion: '9.0'

settings:
  autoInstallPeers: true

importers:

  .:
    devDependencies:
      typescript:
        specifier: 7.0.2
        version: 7.0.2

  packages/a:
    dependencies:
      '@scope/b':
        specifier: workspace:*
        version: link:../b
      zod:
        specifier: 4.6.5
        version: 4.6.5

  packages/b: {}

packages:

  typescript@7.0.2:
    resolution: {integrity: sha512-x}
`;

test("no drift when every manifest's dependencies match its lockfile importer", () => {
  const manifests = new Map<string, Record<string, unknown>>([
    ["package.json", { devDependencies: { typescript: "7.0.2" } }],
    ["packages/a/package.json", { dependencies: { zod: "4.6.5", "@scope/b": "workspace:*" } }],
    ["packages/b/package.json", { name: "b" }],
  ]);
  assert.deepEqual(lockfileDrift(LOCKFILE, manifests), []);
});

test("reports changed, added and removed dependencies and unknown importers", () => {
  const manifests = new Map<string, Record<string, unknown>>([
    ["package.json", { devDependencies: { typescript: "7.0.3" } }],
    ["packages/a/package.json", { dependencies: { "@scope/b": "workspace:*" }, devDependencies: { vite: "8.3.4" } }],
    ["packages/c/package.json", {}],
  ]);
  assert.deepEqual(lockfileDrift(LOCKFILE, manifests), [
    ". devDependencies typescript: package.json has 7.0.3, lockfile has 7.0.2",
    "packages/a dependencies zod: package.json has nothing, lockfile has 4.6.5",
    "packages/a devDependencies vite: package.json has 8.3.4, lockfile has nothing",
    "packages/b: in the lockfile but not generated",
    "packages/c: generated but not in the lockfile",
  ]);
});

test("the committed lockfile matches the generated manifests", () => {
  assert.deepEqual(lockfileDrift(readFileSync(LOCKFILE_PATH, "utf8"), manifests()), []);
});
