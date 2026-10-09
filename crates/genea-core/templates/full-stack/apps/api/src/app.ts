import { parseName } from "@{{name}}/shared";
import { Hono } from "hono";
import { validator } from "hono/validator";

/**
 * The API, mounted under `/api`. The web app imports its type to call it
 * through `hc`, so this file must not use Node globals: `src/index.ts`
 * serves it.
 */
export const app = new Hono().basePath("/api").get(
  "/hello",
  validator("query", (query, c) => {
    const input = query["name"];
    const name = typeof input === "string" ? parseName(input) : null;
    if (name === null) {
      return c.json({ error: "Enter a name." }, 400);
    }
    return { name };
  }),
  (c) => c.json({ message: `Hello, ${c.req.valid("query").name}!` }),
);

export type AppType = typeof app;
