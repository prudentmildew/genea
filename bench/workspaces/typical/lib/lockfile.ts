// Checks, without the network, that the committed lockfile still matches the
// manifests the generator writes, so `pnpm install --frozen-lockfile` will
// accept it. Reads only the `importers:` section of a pnpm v9 lockfile.

const DEP_FIELDS = ["dependencies", "devDependencies", "optionalDependencies"] as const;
type DepField = (typeof DEP_FIELDS)[number];
type Importer = Map<DepField, Map<string, string>>;

const unquote = (s: string): string => s.trim().replace(/^(['"])(.*)\1$/, "$2");

function parseImporters(lockfile: string): Map<string, Importer> {
  const importers = new Map<string, Importer>();
  let inImporters = false;
  let importer: Importer | undefined;
  let field: Map<string, string> | undefined;
  let dep: string | undefined;
  for (const line of lockfile.split("\n")) {
    if (/^\S/.test(line)) {
      inImporters = line.startsWith("importers:");
      continue;
    }
    if (!inImporters || line.trim() === "") continue;
    const indent = line.length - line.trimStart().length;
    const text = line.trim();
    if (indent === 2) {
      const [key = ""] = text.split(/:(?:\s|$)/);
      importer = new Map();
      importers.set(unquote(key), importer);
    } else if (indent === 4 && importer) {
      const name = text.replace(/:$/, "") as DepField;
      field = DEP_FIELDS.includes(name) ? new Map() : undefined;
      if (field) importer.set(name, field);
    } else if (indent === 6 && field) {
      dep = unquote(text.replace(/:$/, ""));
    } else if (indent === 8 && field && dep && text.startsWith("specifier:")) {
      field.set(dep, unquote(text.slice("specifier:".length)));
    }
  }
  return importers;
}

/** Human-readable differences between the lockfile's importers and the manifests (keyed by path). */
export function lockfileDrift(lockfile: string, manifests: Map<string, Record<string, unknown>>): string[] {
  const locked = parseImporters(lockfile);
  const drift: string[] = [];
  const generated = new Set<string>();
  for (const [path, manifest] of manifests) {
    const id = path === "package.json" ? "." : path.replace(/\/package\.json$/, "");
    generated.add(id);
    const importer = locked.get(id);
    if (!importer) {
      drift.push(`${id}: generated but not in the lockfile`);
      continue;
    }
    for (const field of DEP_FIELDS) {
      const wanted = new Map(Object.entries((manifest[field] ?? {}) as Record<string, string>));
      const have = importer.get(field) ?? new Map<string, string>();
      for (const name of [...new Set([...wanted.keys(), ...have.keys()])].sort()) {
        const w = wanted.get(name);
        const h = have.get(name);
        if (w !== h) {
          drift.push(
            `${id} ${field} ${name}: package.json has ${w ?? "nothing"}, lockfile has ${h ?? "nothing"}`,
          );
        }
      }
    }
  }
  for (const id of locked.keys()) {
    if (!generated.has(id)) drift.push(`${id}: in the lockfile but not generated`);
  }
  return drift.sort();
}
