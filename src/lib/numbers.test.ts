import { describe, it, expect } from "vitest";
import { canonicalizeNumber, extractNumbers, hasNumber } from "./numbers";

describe("canonicalizeNumber", () => {
  it("normalizes percent forms to the same value", () => {
    expect(canonicalizeNumber("4.2%")).toBe("4.2");
    expect(canonicalizeNumber("4.2 percent")).toBe("4.2");
    expect(canonicalizeNumber("4.2")).toBe("4.2");
  });

  it("strips thousands separators and currency", () => {
    expect(canonicalizeNumber("4,200")).toBe("4200");
    expect(canonicalizeNumber("$3")).toBe("3");
  });

  it("collapses insignificant trailing zeros so 4 and 4.0 collide", () => {
    expect(canonicalizeNumber("4.0")).toBe("4");
    expect(canonicalizeNumber("4")).toBe("4");
  });

  it("returns null for non-numbers", () => {
    expect(canonicalizeNumber("forty")).toBeNull();
    expect(canonicalizeNumber("abc")).toBeNull();
  });
});

describe("extractNumbers — digits and spelled-out numerals", () => {
  it("digit percentages", () => {
    expect(extractNumbers("unemployment is 4.2%")).toContain("4.2");
  });

  it("the transcriber writes 'forty', which must count as 40", () => {
    expect(extractNumbers("they cut it by forty")).toContain("40");
  });

  it("compound spelled numerals", () => {
    expect(extractNumbers("forty two people")).toContain("42");
  });

  it("scaled spelled numerals", () => {
    expect(extractNumbers("three million jobs")).toContain("3000000");
  });

  it("'two hundred and five'", () => {
    expect(extractNumbers("two hundred and five")).toContain("205");
  });

  it("a year is a number", () => {
    expect(extractNumbers("their figure is from 2019")).toContain("2019");
  });

  it("matches 4.2% against a spoken '4.2 percent'", () => {
    const a = new Set(extractNumbers("the rate was 4.2%"));
    const b = new Set(extractNumbers("no, it was 4.2 percent"));
    expect([...a].some((n) => b.has(n))).toBe(true);
  });
});

describe("hasNumber", () => {
  it("true for digit and spelled forms", () => {
    expect(hasNumber("it rose 8%")).toBe(true);
    expect(hasNumber("about forty of them")).toBe(true);
  });
  it("false when there is no figure", () => {
    expect(hasNumber("the weather was pleasant")).toBe(false);
  });
});
