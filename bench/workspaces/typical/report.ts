// Usage: node typical/report.ts [dir] [--json] [--skip-node-modules]
// Measures a Typical workspace (default bench/workspaces/out/typical) and
// prints its figures against the targets. Exits 1 when a figure is outside
// ±10 % of its target. --skip-node-modules leaves that figure out of the
// verdict, for a workspace that hasn't been installed yet.

import { resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { checkFigures, measureWorkspace } from "./lib/figures.ts";

const args = process.argv.slice(2);
const flags = new Set(args.filter((a) => a.startsWith("--")));
const positional = args.filter((a) => !a.startsWith("--"));
const dir = resolve(positional[0] ?? fileURLToPath(new URL("../out/typical", import.meta.url)));

const rows = checkFigures(measureWorkspace(dir)).filter(
  (r) => !(flags.has("--skip-node-modules") && r.figure === "nodeModulesFiles"),
);
const ok = rows.every((r) => r.ok);

if (flags.has("--json")) {
  console.log(JSON.stringify({ workspace: dir, ok, figures: rows }, null, 2));
} else {
  console.log(`Typical workspace figures for ${dir}`);
  console.log("figure             target     actual   deviation");
  for (const r of rows) {
    const deviation = `${r.deviation >= 0 ? "+" : ""}${(r.deviation * 100).toFixed(1)} %`;
    console.log(
      `${r.figure.padEnd(16)} ${String(r.target).padStart(8)} ${String(r.actual).padStart(10)}   ${deviation.padStart(8)}  ${r.ok ? "ok" : "OUT OF RANGE"}`,
    );
  }
  console.log(ok ? "All figures within ±10 % of target." : "Some figures are outside ±10 % of target.");
}
process.exit(ok ? 0 : 1);
