import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { checkFigures, measureWorkspace } from "../lib/figures.ts";
import { generateTypical } from "../lib/workspace.ts";

function tempDir(t: { after: (fn: () => void) => void }): string {
  const dir = mkdtempSync(join(tmpdir(), "typical-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

function write(root: string, path: string, text: string): void {
  mkdirSync(dirname(join(root, path)), { recursive: true });
  writeFileSync(join(root, path), text);
}

test("measures packages, TS files, non-blank lines and node_modules entries", (t) => {
  const root = tempDir(t);
  write(root, "package.json", "{}");
  write(root, "pnpm-workspace.yaml", "packages:\n  - packages/*\n  - 'tools/cli'\n");
  write(root, "packages/a/package.json", "{}");
  write(root, "packages/b/package.json", "{}");
  mkdirSync(join(root, "packages/not-a-package"), { recursive: true });
  write(root, "tools/cli/package.json", "{}");
  write(root, "src/x.ts", "const a = 1;\n\n   \nexport { a };\n");
  write(root, "packages/a/src/y.tsx", "export const Y = () => <div />;\n// end\n");
  write(root, "packages/a/src/z.js", "not counted\n");
  write(root, "node_modules/foo/index.js", "");
  write(root, "node_modules/foo/package.json", "{}");
  mkdirSync(join(root, "node_modules/.bin"));
  symlinkSync("../foo/index.js", join(root, "node_modules/.bin/foo"));
  mkdirSync(join(root, "packages/a/node_modules"));
  symlinkSync("../../../node_modules/foo", join(root, "packages/a/node_modules/foo"));
  write(root, "packages/a/node_modules/types.d.ts", "declare const x: 1;\n");

  assert.deepEqual(measureWorkspace(root), {
    packages: 4,
    tsFiles: 2,
    loc: 4,
    nodeModulesFiles: 5,
  });
});

test("a figure passes within ±10 % of its target and fails outside", () => {
  const rows = checkFigures({ packages: 5, tsFiles: 2200, loc: 269_999, nodeModulesFiles: 150_000 });
  assert.deepEqual(
    rows.map((r) => [r.figure, r.target, r.actual, r.ok]),
    [
      ["packages", 5, 5, true],
      ["tsFiles", 2000, 2200, true],
      ["loc", 300_000, 269_999, false],
      ["nodeModulesFiles", 150_000, 150_000, true],
    ],
  );
});

test("the generated workspace's packages, TS files and LOC are within ±10 % of the targets", (t) => {
  const root = tempDir(t);
  generateTypical(root);
  const rows = checkFigures(measureWorkspace(root)).filter((r) => r.figure !== "nodeModulesFiles");
  for (const row of rows) {
    assert.ok(row.ok, `${row.figure}: ${row.actual} is not within ±10 % of ${row.target}`);
  }
});
