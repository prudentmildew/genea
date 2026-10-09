// Generates the TS sources of every workspace package.
//
// Each package is a set of feature folders (src/<feature>/<noun>.ts[x]) plus a
// barrel src/index.ts. Every module exports a core type (`<Name>Item`), a
// `normalize<Name>Item` function and, in .tsx modules, a `<Name>View`
// component. The rest of a module is filled with "units": small, independently
// type-correct snippets (reducers, stores, zod schemas, Hono routes, React
// components, ...) until the module reaches its target line count. Modules
// import from earlier modules of their package and from the packages they
// depend on, so the import graph is a DAG with cross-package edges.
//
// Names are globally unique, so barrels can `export *` without collisions.

import { posix } from "node:path";
import { PACKAGES, SCOPE, type PackageKind } from "./manifest.ts";
import { createRng, seedFrom, type Rng } from "./rng.ts";

const FEATURES = [
  "account", "audit", "billing", "cart", "catalog", "checkout", "customer", "delivery",
  "discount", "export", "feature", "inventory", "invoice", "ledger", "member", "message",
  "notification", "order", "payment", "pricing", "product", "profile", "refund", "report",
  "schedule", "search", "session", "shipment", "subscription", "tenant", "ticket", "upload",
  "warehouse", "workflow",
] as const;

const NOUNS = [
  "address", "adjustment", "alert", "archive", "attempt", "badge", "batch", "bundle",
  "channel", "comment", "config", "contact", "cursor", "digest", "draft", "entry", "event",
  "filter", "forecast", "group", "history", "import", "label", "limit", "line", "link",
  "lock", "log", "metric", "note", "option", "owner", "plan", "policy", "preference",
  "quota", "rate", "receipt", "record", "region", "reminder", "request", "rule", "segment",
  "setting", "snapshot", "source", "step", "summary", "tag", "target", "task", "template",
  "token", "total", "trend", "usage", "version", "window", "zone",
] as const;

/** Seed for everything; bump it to get a different (but still deterministic) workspace. */
const SEED = "genea-typical-v1";

interface Module {
  readonly pkg: PackageKind;
  /** PascalCase, globally unique, e.g. `BillingReceipt`. */
  readonly name: string;
  /** camelCase of `name`. */
  readonly camel: string;
  /** Human words, e.g. `billing receipt`. */
  readonly words: string;
  readonly kebab: string;
  /** Path relative to the package directory. */
  readonly path: string;
  readonly tsx: boolean;
  /** Target number of non-blank lines. */
  readonly targetLines: number;
}

const pascal = (s: string): string => s.charAt(0).toUpperCase() + s.slice(1);
const article = (words: string): string => (/^[aeiou]/.test(words) ? `an ${words}` : `a ${words}`);

/** Every module of every package, in a fixed order. */
function planModules(): Map<PackageKind, Module[]> {
  const rng = createRng(seedFrom(`${SEED}:names`));
  const names = rng.shuffle(FEATURES.flatMap((feature) => NOUNS.map((noun) => [feature, noun] as const)));
  const needed = PACKAGES.reduce((sum, p) => sum + p.files - 1, 0);
  if (names.length < needed) throw new Error(`only ${names.length} module names for ${needed} modules`);

  const plan = new Map<PackageKind, Module[]>();
  let next = 0;
  for (const pkg of PACKAGES) {
    const sizes = createRng(seedFrom(`${SEED}:${pkg.name}:plan`));
    const modules: Module[] = [];
    for (let i = 0; i < pkg.files - 1; i++) {
      const [feature, noun] = names[next++] as readonly [string, string];
      const tsx = sizes.chance(pkg.tsxShare);
      modules.push({
        pkg: pkg.name,
        name: pascal(feature) + pascal(noun),
        camel: feature + pascal(noun),
        words: `${feature} ${noun}`,
        kebab: `${feature}-${noun}`,
        path: `src/${feature}/${noun}.${tsx ? "tsx" : "ts"}`,
        tsx,
        // Skewed sizes: many small modules, a few long ones; mean ≈ 150 lines.
        targetLines: 16 + Math.floor(Math.pow(sizes.next(), 1.6) * 310),
      });
    }
    modules.sort((a, b) => (a.path < b.path ? -1 : a.path > b.path ? 1 : 0));
    plan.set(pkg.name, modules);
  }
  return plan;
}

/** Collects imports and body lines for one module. */
class ModuleWriter {
  readonly #named = new Map<string, Set<string>>();
  readonly #defaults = new Map<string, string>();
  readonly #body: string[] = [];
  #lines = 0;

  /** Import `names` (prefix a name with `type ` for a type-only import) from `from`. */
  use(from: string, ...names: string[]): void {
    let set = this.#named.get(from);
    if (!set) this.#named.set(from, (set = new Set()));
    for (const name of names) set.add(name);
  }

  useDefault(from: string, name: string): void {
    this.#defaults.set(from, name);
  }

  add(block: string): void {
    const lines = block.replace(/^\n+|\s+$/g, "").split("\n");
    this.#body.push(...lines, "");
    this.#lines += lines.filter((l) => l.trim() !== "").length;
  }

  /** Non-blank lines so far, counting one line per import statement. */
  get lines(): number {
    return this.#lines + this.#named.size + this.#defaults.size;
  }

  render(): string {
    const order = (a: string, b: string): number => {
      const ra = a.startsWith(".") ? 1 : 0;
      const rb = b.startsWith(".") ? 1 : 0;
      return ra - rb || (a < b ? -1 : a > b ? 1 : 0);
    };
    const imports: string[] = [];
    const specifiers = [...new Set([...this.#named.keys(), ...this.#defaults.keys()])].sort(order);
    for (const from of specifiers) {
      const def = this.#defaults.get(from);
      if (def) imports.push(`import ${def} from "${from}";`);
      const names = this.#named.get(from);
      if (names && names.size > 0) {
        const sorted = [...names].sort((a, b) => {
          const ka = a.replace(/^type /, "");
          const kb = b.replace(/^type /, "");
          return ka < kb ? -1 : ka > kb ? 1 : 0;
        });
        imports.push(`import { ${sorted.join(", ")} } from "${from}";`);
      }
    }
    const head = imports.length > 0 ? imports.join("\n") + "\n\n" : "";
    return head + this.#body.join("\n").replace(/\n+$/, "") + "\n";
  }
}

interface Ctx {
  readonly m: Module;
  readonly w: ModuleWriter;
  readonly rng: Rng;
  /** Unit index, for unique names. */
  readonly k: number;
}

type Unit = (ctx: Ctx) => void;

// ---------------------------------------------------------------------------
// Units available to every module.

const filterUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
/** Active ${m.words} items ordered by amount, largest first. */
export function active${N}Items${k}(items: readonly ${N}Item[], minimum = 0): ${N}Item[] {
  return items
    .filter((item) => item.state === "active" && item.amount >= minimum)
    .map(normalize${N}Item)
    .sort((a, b) => b.amount - a.amount || a.label.localeCompare(b.label));
}`);
};

const groupUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
/** Groups ${m.words} items by tag; an item with several tags appears in each group. */
export function group${N}ByTag${k}(items: readonly ${N}Item[]): Map<string, ${N}Item[]> {
  const groups = new Map<string, ${N}Item[]>();
  for (const item of items) {
    for (const tag of item.tags) {
      const group = groups.get(tag);
      if (group) {
        group.push(item);
      } else {
        groups.set(tag, [item]);
      }
    }
  }
  return groups;
}`);
};

const reducerUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
export type ${N}Action${k} =
  | { readonly type: "add"; readonly item: ${N}Item }
  | { readonly type: "remove"; readonly id: string }
  | { readonly type: "rename"; readonly id: string; readonly label: string }
  | { readonly type: "archive"; readonly ids: readonly string[] };

/** Applies one ${m.words} action. Never mutates \`state\`. */
export function reduce${N}${k}(state: readonly ${N}Item[], action: ${N}Action${k}): ${N}Item[] {
  switch (action.type) {
    case "add":
      return [...state, normalize${N}Item(action.item)];
    case "remove":
      return state.filter((item) => item.id !== action.id);
    case "rename":
      return state.map((item) => (item.id === action.id ? { ...item, label: action.label } : item));
    case "archive": {
      const ids = new Set(action.ids);
      return state.map((item) => (ids.has(item.id) ? { ...item, state: "archived" as const } : item));
    }
  }
}`);
};

const storeUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
/** In-memory ${m.words} store keyed by id. */
export class ${N}Store${k} {
  readonly #items = new Map<string, ${N}Item>();

  get size(): number {
    return this.#items.size;
  }

  upsert(item: ${N}Item): ${N}Item {
    const normalized = normalize${N}Item(item);
    this.#items.set(normalized.id, normalized);
    return normalized;
  }

  get(id: string): ${N}Item | undefined {
    return this.#items.get(id);
  }

  remove(id: string): boolean {
    return this.#items.delete(id);
  }

  byState(state: ${N}State): ${N}Item[] {
    return [...this.#items.values()].filter((item) => item.state === state);
  }

  total(): number {
    let sum = 0;
    for (const item of this.#items.values()) sum += item.amount;
    return sum;
  }
}`);
};

const resultUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
export type ${N}Result${k}<T> =
  | { readonly ok: true; readonly value: T }
  | { readonly ok: false; readonly error: string };

export function validate${N}${k}(item: ${N}Item): ${N}Result${k}<${N}Item> {
  if (item.label.length === 0) return { ok: false, error: "${m.words} needs a label" };
  if (!Number.isFinite(item.amount)) return { ok: false, error: "${m.words} amount must be finite" };
  if (item.amount < 0) return { ok: false, error: "${m.words} amount must not be negative" };
  return { ok: true, value: normalize${N}Item(item) };
}

export function unwrap${N}${k}<T>(result: ${N}Result${k}<T>, fallback: T): T {
  return result.ok ? result.value : fallback;
}`);
};

const patchUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
export type ${N}Patch${k} = { -readonly [K in keyof ${N}Item]?: ${N}Item[K] };

export function apply${N}Patch${k}(item: ${N}Item, patch: ${N}Patch${k}): ${N}Item {
  const next: ${N}Item = { ...item, ...patch };
  return normalize${N}Item(next);
}

export function changed${N}Keys${k}(before: ${N}Item, after: ${N}Item): (keyof ${N}Item)[] {
  const keys: (keyof ${N}Item)[] = ["id", "createdAt", "label", "amount", "tags", "state"];
  return keys.filter((key) => before[key] !== after[key]);
}`);
};

const describeUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
/** A one-line description of ${article(m.words)}, for logs and tooltips. */
export function describe${N}${k}(item: ${N}Item): string {
  switch (item.state) {
    case "draft":
      return \`\${item.label} (draft)\`;
    case "active":
      return \`\${item.label}: \${item.amount.toFixed(2)}\`;
    case "archived":
      return \`\${item.label} [archived, \${item.tags.length} tags]\`;
  }
}`);
};

const statsUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
export interface ${N}Stats${k} {
  readonly count: number;
  readonly min: number;
  readonly max: number;
  readonly mean: number;
}

export function ${m.camel}Stats${k}(items: readonly ${N}Item[]): ${N}Stats${k} {
  if (items.length === 0) return { count: 0, min: 0, max: 0, mean: 0 };
  let min = Number.POSITIVE_INFINITY;
  let max = Number.NEGATIVE_INFINITY;
  let sum = 0;
  for (const { amount } of items) {
    min = Math.min(min, amount);
    max = Math.max(max, amount);
    sum += amount;
  }
  return { count: items.length, min, max, mean: sum / items.length };
}`);
};

const batchUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.add(`
/** Runs \`handle\` over ${m.words} items in batches and reports the ids that failed. */
export async function process${N}Batch${k}(
  items: readonly ${N}Item[],
  handle: (item: ${N}Item) => Promise<boolean>,
  batchSize = 10,
): Promise<{ processed: number; failed: string[] }> {
  const failed: string[] = [];
  let processed = 0;
  for (let start = 0; start < items.length; start += batchSize) {
    const batch = items.slice(start, start + batchSize);
    const results = await Promise.all(batch.map(handle));
    results.forEach((ok, index) => {
      const item = batch[index];
      if (!ok && item) failed.push(item.id);
    });
    processed += batch.length;
  }
  return { processed, failed };
}`);
};

const weightUnit: Unit = ({ m, w, k, rng }) => {
  const N = m.name;
  w.add(`
const ${m.camel}Weights${k}: Record<${N}State, number> = { draft: ${rng.int(1, 9) / 10}, active: 1, archived: 0 };

export function weighted${N}Total${k}(items: readonly ${N}Item[]): number {
  return items.reduce((sum, item) => sum + item.amount * ${m.camel}Weights${k}[item.state], 0);
}`);
};

const COMMON_UNITS: readonly Unit[] = [
  filterUnit, groupUnit, reducerUnit, storeUnit, resultUnit, patchUnit, describeUnit, statsUnit,
  batchUnit, weightUnit,
];

// ---------------------------------------------------------------------------
// Units that use a package's dependencies.

const zodUnit: Unit = ({ m, w, k, rng }) => {
  const N = m.name;
  w.use("zod", "z");
  w.add(`
export const ${m.camel}Schema${k} = z.object({
  id: z.string().min(1),
  createdAt: z.coerce.date(),
  label: z.string().min(1).max(${rng.int(40, 400)}),
  amount: z.number().nonnegative(),
  tags: z.array(z.string()).default([]),
  state: z.enum(["draft", "active", "archived"]),
});

export type ${N}Input${k} = z.infer<typeof ${m.camel}Schema${k}>;

export function parse${N}${k}(value: unknown): ${N}Item {
  return normalize${N}Item(${m.camel}Schema${k}.parse(value));
}`);
};

const dateUnit: Unit = ({ m, w, k, rng }) => {
  const N = m.name;
  w.use("date-fns", "addDays", "format", "isBefore");
  w.add(`
export function ${m.camel}DueDate${k}(item: ${N}Item, days = ${rng.int(7, 90)}): string {
  return format(addDays(item.createdAt, days), "yyyy-MM-dd");
}

export function is${N}Overdue${k}(item: ${N}Item, now: Date, days = ${rng.int(7, 90)}): boolean {
  return isBefore(addDays(item.createdAt, days), now);
}`);
};

const lodashUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.use("lodash-es", "chunk", "sortBy");
  w.add(`
export function ${m.camel}Pages${k}(items: readonly ${N}Item[], size: number): ${N}Item[][] {
  return chunk(sortBy([...items], (item) => item.label), size);
}`);
};

const queryUnit: Unit = ({ m, w, k, rng }) => {
  const N = m.name;
  w.use("@tanstack/react-query", "useQuery");
  w.add(`
export function use${N}Items${k}(filter: string) {
  return useQuery({
    queryKey: ["${m.kebab}", ${k}, filter],
    queryFn: async (): Promise<${N}Item[]> => {
      const response = await fetch(\`/api/${m.kebab}?filter=\${encodeURIComponent(filter)}\`);
      if (!response.ok) throw new Error(\`${m.words} request failed: \${response.status}\`);
      const body = (await response.json()) as ${N}Item[];
      return body.map(normalize${N}Item);
    },
    staleTime: ${rng.int(1, 60)} * 1000,
  });
}`);
};

const honoUnit: Unit = ({ m, w, k }) => {
  w.use("hono", "Hono");
  w.add(`
export const ${m.camel}Routes${k} = new Hono()
  .get("/${m.kebab}/${k}", (c) => c.json({ items: [] as string[], page: Number(c.req.query("page") ?? "1") }))
  .post("/${m.kebab}/${k}", async (c) => {
    const body = await c.req.json<{ label?: string; amount?: number }>();
    return c.json({ label: body.label ?? "", amount: body.amount ?? 0 }, 201);
  })
  .delete("/${m.kebab}/${k}/:id", (c) => c.json({ deleted: c.req.param("id") }));`);
};

const rxjsUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.use("rxjs", "filter", "from", "map", "type Observable");
  w.add(`
export function ${m.camel}Stream${k}(items: readonly ${N}Item[]): Observable<string> {
  return from(items).pipe(
    filter((item) => item.state !== "archived"),
    map((item) => \`\${item.id}:\${item.amount}\`),
  );
}`);
};

// ---------------------------------------------------------------------------
// React units (.tsx modules).

function viewComponent(m: Module, w: ModuleWriter): void {
  const N = m.name;
  w.use("react", "useMemo", "useState");
  w.use("@mui/material", "Button", "Stack", "Typography");
  w.add(`
export interface ${N}ViewProps {
  readonly items: readonly ${N}Item[];
  readonly title: string;
  readonly onSelect?: (item: ${N}Item) => void;
}

export function ${N}View({ items, title, onSelect }: ${N}ViewProps) {
  const [query, setQuery] = useState("");
  const visible = useMemo(
    () => items.filter((item) => item.label.toLowerCase().includes(query.toLowerCase())),
    [items, query],
  );
  return (
    <Stack spacing={2}>
      <Typography variant="h6">{title}</Typography>
      <input value={query} onChange={(event) => setQuery(event.target.value)} />
      <ul>
        {visible.map((item) => (
          <li key={item.id}>
            <Button onClick={() => onSelect?.(item)}>{item.label}</Button>
            <span>{item.amount.toFixed(2)}</span>
          </li>
        ))}
      </ul>
    </Stack>
  );
}`);
}

const rowUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.use("@mui/material", "Box", "Chip", "Typography");
  w.add(`
export function ${N}Row${k}({ item, selected }: { readonly item: ${N}Item; readonly selected: boolean }) {
  return (
    <Box sx={{ display: "flex", gap: 1, fontWeight: selected ? 600 : 400 }}>
      <Typography component="span">{item.label}</Typography>
      <Chip size="small" label={item.state} />
      <Typography component="span" color="text.secondary">
        {item.amount.toFixed(2)}
      </Typography>
    </Box>
  );
}`);
};

const formUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.use("react", "useState");
  w.use("@mui/material", "Button", "TextField");
  w.add(`
export function ${N}Form${k}({
  initial,
  onSubmit,
}: {
  readonly initial: ${N}Item;
  readonly onSubmit: (item: ${N}Item) => void;
}) {
  const [label, setLabel] = useState(initial.label);
  const [amount, setAmount] = useState(String(initial.amount));
  const valid = label.trim().length > 0 && Number.isFinite(Number(amount));
  return (
    <form
      onSubmit={(event) => {
        event.preventDefault();
        if (valid) onSubmit(normalize${N}Item({ ...initial, label, amount: Number(amount) }));
      }}
    >
      <TextField label="Label" value={label} onChange={(event) => setLabel(event.target.value)} />
      <TextField label="Amount" value={amount} onChange={(event) => setAmount(event.target.value)} />
      <Button type="submit" disabled={!valid}>
        Save
      </Button>
    </form>
  );
}`);
};

const summaryUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.use("react", "useMemo");
  w.use("@mui/material", "Stack", "Typography");
  w.add(`
export function ${N}Summary${k}({ items }: { readonly items: readonly ${N}Item[] }) {
  const totals = useMemo(() => {
    const byState: Record<${N}State, number> = { draft: 0, active: 0, archived: 0 };
    for (const item of items) byState[item.state] += item.amount;
    return byState;
  }, [items]);
  return (
    <Stack spacing={0.5}>
      {(Object.keys(totals) as ${N}State[]).map((state) => (
        <Typography key={state} variant="body2">
          {state}: {totals[state].toFixed(2)}
        </Typography>
      ))}
    </Stack>
  );
}`);
};

const listUnit: Unit = ({ m, w, k }) => {
  const N = m.name;
  w.use("react", "type ReactNode");
  w.use("@mui/material", "Box", "Typography");
  w.add(`
export interface ${N}ListProps${k}<T> {
  readonly rows: readonly T[];
  readonly getKey: (row: T) => string;
  readonly render: (row: T) => ReactNode;
  readonly empty?: ReactNode;
}

export function ${N}List${k}<T>({ rows, getKey, render, empty = "Nothing here" }: ${N}ListProps${k}<T>) {
  if (rows.length === 0) return <Typography color="text.secondary">{empty}</Typography>;
  return (
    <Box component="ul" sx={{ listStyle: "none", p: 0, m: 0 }}>
      {rows.map((row) => (
        <li key={getKey(row)}>{render(row)}</li>
      ))}
    </Box>
  );
}`);
};

const toolbarUnit: Unit = ({ m, w, k, rng }) => {
  const N = m.name;
  w.use("@mui/material", "Button", "IconButton", "Stack");
  let icon: string;
  switch (rng.int(0, 2)) {
    case 0:
      w.useDefault("@mui/icons-material/Add", "AddIcon");
      icon = "<AddIcon />";
      break;
    case 1:
      w.use("lucide-react", "Plus");
      icon = "<Plus size={16} />";
      break;
    default:
      w.use("@tabler/icons-react", "IconPlus");
      icon = "<IconPlus size={16} />";
  }
  w.add(`
export function ${N}Toolbar${k}({ onAdd, onRefresh }: { readonly onAdd: () => void; readonly onRefresh: () => void }) {
  return (
    <Stack direction="row" spacing={1}>
      <IconButton aria-label="add ${m.words}" onClick={onAdd}>
        ${icon}
      </IconButton>
      <Button variant="outlined" onClick={onRefresh}>
        Refresh
      </Button>
    </Stack>
  );
}`);
};

/** A page that loads items and renders this module's view and the views it depends on. */
function pageUnit(views: readonly Module[]): Unit {
  return ({ m, w, k }) => {
    const N = m.name;
    w.use("react", "useState");
    w.use("@tanstack/react-query", "useQuery");
    w.use("@mui/material", "Alert", "CircularProgress", "Stack", "TextField");
    const others = views
      .map((v) => `\n      <${v.name}View title="${pascal(v.words)}" items={[]} />`)
      .join("");
    w.add(`
export function ${N}Page${k}({ title }: { readonly title: string }) {
  const [filter, setFilter] = useState("");
  const query = useQuery({
    queryKey: ["${m.kebab}-page", ${k}, filter],
    queryFn: async (): Promise<${N}Item[]> => {
      const response = await fetch(\`/api/${m.kebab}?q=\${encodeURIComponent(filter)}\`);
      return ((await response.json()) as ${N}Item[]).map(normalize${N}Item);
    },
  });
  if (query.isPending) return <CircularProgress />;
  if (query.isError) return <Alert severity="error">{query.error.message}</Alert>;
  return (
    <Stack spacing={2}>
      <TextField label="Filter" value={filter} onChange={(event) => setFilter(event.target.value)} />
      <${N}View title={title} items={query.data} />${others}
    </Stack>
  );
}`);
  };
}

function unitsFor(m: Module): readonly Unit[] {
  const extra: Unit[] = [];
  switch (m.pkg) {
    case "shared":
      extra.push(zodUnit, zodUnit, dateUnit, lodashUnit);
      break;
    case "ui":
      if (m.tsx) extra.push(rowUnit, rowUnit, formUnit, summaryUnit, listUnit, toolbarUnit, toolbarUnit);
      break;
    case "web":
      extra.push(queryUnit, dateUnit);
      if (m.tsx) extra.push(rowUnit, formUnit, summaryUnit, listUnit);
      break;
    case "api":
      extra.push(zodUnit, honoUnit, honoUnit, rxjsUnit);
      break;
  }
  return [...COMMON_UNITS, ...extra];
}

// ---------------------------------------------------------------------------

interface Dep {
  readonly module: Module;
  /** Import specifier as seen from the importing module. */
  readonly from: string;
}

function relativeSpecifier(fromPath: string, toPath: string): string {
  const rel = posix.relative(posix.dirname(fromPath), toPath);
  return rel.startsWith(".") ? rel : `./${rel}`;
}

function renderModule(m: Module, deps: readonly Dep[], rng: Rng): string {
  const w = new ModuleWriter();
  const N = m.name;
  w.add(`
export type ${N}State = "draft" | "active" | "archived";

/** ${pascal(article(m.words))} as the ${m.pkg} package sees it. */
export interface ${N}Item {
  readonly id: string;
  readonly createdAt: Date;
  label: string;
  amount: number;
  tags: readonly string[];
  state: ${N}State;
}

export function normalize${N}Item(item: ${N}Item): ${N}Item {
  return {
    ...item,
    label: item.label.trim(),
    amount: Math.round(item.amount * 100) / 100,
    tags: [...new Set(item.tags)].sort(),
  };
}`);
  if (m.tsx) viewComponent(m, w);

  for (const { module: d, from } of deps) {
    w.use(from, `normalize${d.name}Item`, `type ${d.name}Item`);
    w.add(`
/** Converts ${article(d.words)} into ${article(m.words)}. */
export function ${m.camel}From${d.name}(source: ${d.name}Item): ${N}Item {
  const base = normalize${d.name}Item(source);
  return normalize${N}Item({ ...base, id: \`${m.kebab}:\${base.id}\`, label: \`${pascal(m.words)} \${base.label}\` });
}`);
  }

  const units = unitsFor(m);
  const views = deps.filter((d) => d.module.tsx);
  let k = 0;
  if (m.tsx && m.pkg === "web") {
    for (const { module: v, from } of views) w.use(from, `${v.name}View`);
    pageUnit(views.map((d) => d.module))({ m, w, rng, k: k++ });
  }
  while (w.lines < m.targetLines) rng.pick(units)({ m, w, rng, k: k++ });
  return w.render();
}

/** Every generated source file, by path relative to the workspace root. */
export function generateSources(): Map<string, string> {
  const plan = planModules();
  const files = new Map<string, string>();
  for (const pkg of PACKAGES) {
    const rng = createRng(seedFrom(`${SEED}:${pkg.name}:sources`));
    const modules = plan.get(pkg.name) ?? [];
    modules.forEach((m, index) => {
      const deps: Dep[] = [];
      // Up to three earlier modules of the same package, mostly nearby ones.
      const local = new Set<number>();
      for (let n = rng.int(0, 3); n > 0 && index > 0; n--) {
        local.add(Math.max(0, index - 1 - Math.floor(Math.pow(rng.next(), 2) * index)));
      }
      for (const i of [...local].sort((a, b) => a - b)) {
        const d = modules[i] as Module;
        deps.push({ module: d, from: relativeSpecifier(m.path, d.path) });
      }
      // Up to two modules from each package this one depends on.
      for (const other of pkg.workspaceDeps) {
        const candidates = plan.get(other) ?? [];
        const picked = new Set<Module>();
        for (let n = rng.int(0, 2); n > 0; n--) picked.add(rng.pick(candidates));
        for (const d of [...picked].sort((a, b) => (a.path < b.path ? -1 : 1))) {
          deps.push({ module: d, from: `${SCOPE}/${other}` });
        }
      }
      files.set(`packages/${pkg.name}/${m.path}`, renderModule(m, deps, rng));
    });
    const barrel = modules.map((m) => `export * from "./${m.path.slice("src/".length)}";`).join("\n");
    files.set(`packages/${pkg.name}/src/index.ts`, barrel + "\n");
  }
  return files;
}

