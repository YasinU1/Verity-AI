import { describe, it, expect } from "vitest";
import { verdictStyle, VERDICT_STYLE } from "./verdictStyle";
import { isAlert, HUD_VERDICTS } from "./types";

describe("verdict styling / alert gate", () => {
  it("only false and misleading are HUD alarms", () => {
    expect(HUD_VERDICTS.slice().sort()).toEqual(["false", "misleading"]);
    expect(isAlert("false")).toBe(true);
    expect(isAlert("misleading")).toBe(true);
    expect(isAlert("verified")).toBe(false);
    expect(isAlert("context_needed")).toBe(false);
    expect(isAlert("unverifiable")).toBe(false);
  });

  it("false and misleading are BOTH red (amber would read as 'minor')", () => {
    expect(verdictStyle("false").color).toBe("#ef4444");
    expect(verdictStyle("misleading").color).toBe("#ef4444");
    expect(verdictStyle("misleading").color).toBe(verdictStyle("false").color);
  });

  it("keeps distinct icons despite the shared colour", () => {
    expect(verdictStyle("false").icon).not.toBe(verdictStyle("misleading").icon);
  });

  it("marks exactly the two alarm verdicts as hudAlarm", () => {
    const alarms = Object.entries(VERDICT_STYLE)
      .filter(([, s]) => s.hudAlarm)
      .map(([k]) => k)
      .sort();
    expect(alarms).toEqual(["false", "misleading"]);
  });
});
