import { describe, expect, it } from "vitest";
import { app } from "./app.ts";

describe("app", () => {
  it("answers on /", async () => {
    const response = await app.request("/");
    expect(response.status).toBe(200);
    expect(await response.text()).toBe("Hello from {{name}}!");
  });

  it("greets by name", async () => {
    const response = await app.request("/hello/Ada");
    expect(await response.json()).toEqual({ message: "Hello, Ada!" });
  });
});
