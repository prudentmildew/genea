import { describe, expect, it } from "vitest";
import { clickLabel } from "./clicks";

describe("clickLabel", () => {
  it("asks for the first click", () => {
    expect(clickLabel(0)).toBe("Click me");
  });

  it("counts the clicks", () => {
    expect(clickLabel(1)).toBe("Clicked once");
    expect(clickLabel(3)).toBe("Clicked 3 times");
  });
});
