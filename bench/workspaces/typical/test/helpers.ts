import { createHash } from "node:crypto";
import { readdirSync, readFileSync } from "node:fs";
import { join, relative } from "node:path";

/** A digest over every file's relative path and bytes, independent of where the tree lives. */
export function treeDigest(root: string): { files: number; sha256: string } {
  const hash = createHash("sha256");
  let files = 0;
  const walk = (dir: string): void => {
    const entries = readdirSync(dir, { withFileTypes: true }).sort((x, y) =>
      x.name < y.name ? -1 : x.name > y.name ? 1 : 0,
    );
    for (const entry of entries) {
      const path = join(dir, entry.name);
      if (entry.isDirectory()) {
        walk(path);
      } else {
        files++;
        hash.update(relative(root, path));
        hash.update("\0");
        hash.update(readFileSync(path));
        hash.update("\0");
      }
    }
  };
  walk(root);
  return { files, sha256: hash.digest("hex") };
}
