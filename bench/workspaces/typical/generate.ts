// Usage: node typical/generate.ts [outDir]
// Writes the Typical reference workspace (without node_modules) into outDir,
// default bench/workspaces/out/typical. Existing files are overwritten; files
// the generator no longer writes are left alone, so use a fresh directory
// (setup.sh does) when the generator has changed.

import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { generateTypical } from "./lib/workspace.ts";

const defaultOut = fileURLToPath(new URL("../out/typical", import.meta.url));
const outDir = resolve(process.argv[2] ?? defaultOut);
generateTypical(outDir);
console.log(`Typical workspace written to ${outDir}`);
