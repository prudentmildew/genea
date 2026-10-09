import { Hono } from "hono";

/**
 * The app's routes. `src/index.ts` serves them, and the tests call them
 * through `app.request()`.
 */
export const app = new Hono()
  .get("/", (c) => c.text("Hello from {{name}}!"))
  .get("/hello/:name", (c) => c.json({ message: `Hello, ${c.req.param("name")}!` }));
