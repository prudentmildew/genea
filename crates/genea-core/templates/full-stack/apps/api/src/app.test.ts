import { describe, expect, it } from "vitest";
import { app } from "./app.ts";

describe("GET /api/hello", () => {
  it("greets by name", async () => {
    const response = await app.request("/api/hello?name=Ada");
    expect(response.status).toBe(200);
    expect(await response.json()).toEqual({ message: "Hello, Ada!" });
  });

  it("refuses an empty name", async () => {
    const response = await app.request("/api/hello?name=");
    expect(response.status).toBe(400);
  });
});
