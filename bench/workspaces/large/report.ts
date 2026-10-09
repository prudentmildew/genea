// Usage: node large/report.ts [dir] [--json]
// Prints the Large workspace's figures (default bench/workspaces/out/large).
// Large has no targets: it is whatever vscode is at the pinned commit.

import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { measureWorkspace } from "../typical/lib/figures.ts";

const args = process.argv.slice(2);
const positional = args.filter((a) => !a.startsWith("--"));
const dir = resolve(positional[0] ?? fileURLToPath(new URL("../out/large", import.meta.url)));
const { tsFiles, loc, nodeModulesFiles } = measureWorkspace(dir);

if (args.includes("--json")) {
  console.log(JSON.stringify({ workspace: dir, figures: { tsFiles, loc, nodeModulesFiles } }, null, 2));
} else {
  console.log(`Large workspace figures for ${dir}`);
  console.log(`tsFiles           ${String(tsFiles).padStart(10)}`);
  console.log(`loc               ${String(loc).padStart(10)}`);
  console.log(`nodeModulesFiles  ${String(nodeModulesFiles).padStart(10)}`);
}
