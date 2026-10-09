// Measures a workspace's figures and compares them with the Typical targets.
//
// - packages: the root package plus each directory matched by the
//   `packages` globs of pnpm-workspace.yaml that has a package.json (the
//   workspace model of #19). Only literal paths and a trailing `/*` are supported.
// - tsFiles: .ts/.tsx/.mts/.cts files outside node_modules.
// - loc: non-blank lines in those files.
// - nodeModulesFiles: every non-directory entry (files and symlinks, which are
//   not followed) inside any node_modules directory.

import { existsSync, lstatSync, readdirSync, readFileSync } from "node:fs";
import { join } from "node:path";
import { TARGETS, TOLERANCE } from "./manifest.ts";

export interface Figures {
  packages: number;
  tsFiles: number;
  loc: number;
  nodeModulesFiles: number;
}

export interface FigureRow {
  figure: keyof Figures;
  target: number;
  actual: number;
  /** (actual - target) / target */
  deviation: number;
  ok: boolean;
}

const TS_FILE = /\.(c|m)?tsx?$/;

export function measureWorkspace(root: string): Figures {
  const figures: Figures = { packages: countPackages(root), tsFiles: 0, loc: 0, nodeModulesFiles: 0 };
  const walk = (dir: string, inNodeModules: boolean): void => {
    for (const entry of readdirSync(dir, { withFileTypes: true })) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(path, inNodeModules || entry.name === "node_modules");
      } else if (inNodeModules) {
        figures.nodeModulesFiles++;
      } else if (entry.isFile() && TS_FILE.test(entry.name)) {
        figures.tsFiles++;
        figures.loc += countNonBlankLines(readFileSync(path, "utf8"));
      }
    }
  };
  walk(root, false);
  return figures;
}

function countNonBlankLines(text: string): number {
  let count = 0;
  for (const line of text.split("\n")) if (line.trim() !== "") count++;
  return count;
}

function countPackages(root: string): number {
  let count = existsSync(join(root, "package.json")) ? 1 : 0;
  const workspaceFile = join(root, "pnpm-workspace.yaml");
  if (!existsSync(workspaceFile)) return count;
  for (const pattern of workspacePatterns(readFileSync(workspaceFile, "utf8"))) {
    const dirs = pattern.endsWith("/*") ? childDirs(join(root, pattern.slice(0, -2))) : [join(root, pattern)];
    for (const dir of dirs) if (existsSync(join(dir, "package.json"))) count++;
  }
  return count;
}

/** The entries of the top-level `packages:` list. */
function workspacePatterns(yaml: string): string[] {
  const patterns: string[] = [];
  let inPackages = false;
  for (const line of yaml.split("\n")) {
    if (/^packages:\s*$/.test(line)) {
      inPackages = true;
    } else if (inPackages && /^\s+-\s+/.test(line)) {
      patterns.push(line.replace(/^\s+-\s+/, "").trim().replace(/^(['"])(.*)\1$/, "$2"));
    } else if (/^\S/.test(line)) {
      inPackages = false;
    }
  }
  return patterns;
}

function childDirs(dir: string): string[] {
  if (!existsSync(dir)) return [];
  return readdirSync(dir)
    .map((name) => join(dir, name))
    .filter((path) => lstatSync(path).isDirectory());
}

export function checkFigures(figures: Figures): FigureRow[] {
  return (Object.keys(TARGETS) as (keyof Figures)[]).map((figure) => {
    const target = TARGETS[figure];
    const actual = figures[figure];
    const deviation = (actual - target) / target;
    return { figure, target, actual, deviation, ok: Math.abs(deviation) <= TOLERANCE + 1e-12 };
  });
}
