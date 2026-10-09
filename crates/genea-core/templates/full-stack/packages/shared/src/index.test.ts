import { describe, expect, it } from "vitest";
import { MAX_NAME_LENGTH, parseName } from "./index.ts";

describe("parseName", () => {
  it("trims the name", () => {
    expect(parseName("  Ada ")).toBe("Ada");
  });

  it("refuses an empty or overlong name", () => {
    expect(parseName("   ")).toBeNull();
    expect(parseName("a".repeat(MAX_NAME_LENGTH + 1))).toBeNull();
  });
});
