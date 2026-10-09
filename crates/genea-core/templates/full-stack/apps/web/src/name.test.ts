import { describe, expect, it } from "vitest";
import { nameHint } from "./name";

describe("nameHint", () => {
  it("is empty for a valid name", () => {
    expect(nameHint("Ada")).toBe("");
  });

  it("explains what is wrong", () => {
    expect(nameHint(" ")).toBe("Enter a name.");
    expect(nameHint("a".repeat(100))).toBe("Use at most 40 characters.");
  });
});
