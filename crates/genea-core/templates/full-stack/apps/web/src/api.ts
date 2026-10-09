import type { AppType } from "@{{name}}/api";
import { hc } from "hono/client";

/**
 * Typed calls to the API. In development, Vite proxies `/api` to it
 * (`vite.config.ts`), so the app and the API share one URL.
 */
export const api = hc<AppType>("/").api;
