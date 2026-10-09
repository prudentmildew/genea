import { test } from "node:test";
import assert from "node:assert/strict";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { generateTypical } from "../lib/workspace.ts";
import { treeDigest } from "./helpers.ts";

function tempDir(t: { after: (fn: () => void) => void }): string {
  const dir = mkdtempSync(join(tmpdir(), "typical-"));
  t.after(() => rmSync(dir, { recursive: true, force: true }));
  return dir;
}

test("generating twice gives byte-identical output, wherever it is written", (t) => {
  const a = join(tempDir(t), "one");
  const b = join(tempDir(t), "nested", "two");
  generateTypical(a);
  generateTypical(b);
  const digestA = treeDigest(a);
  assert.ok(digestA.files > 0, "the generator wrote nothing");
  assert.deepEqual(treeDigest(b), digestA);
});
