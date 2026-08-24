import { describe, it, expect } from "vitest";
import { buildDebateBrief, BRIEF_MAX_CHARS } from "./debateBrief";
import { SessionIndex } from "./sessionIndex";
import type { VerdictCard } from "./types";

function verdict(partial: Partial<VerdictCard> & { id: string; claim: string }): VerdictCard {
  return {
    verdict: "false",
    rationale: "",
    createdAt: 0,
    provenance: {
      webSearchUsed: true, provider: "openai", model: "gpt-4o-mini",
      latencyMs: 0, sources: [], prefetched: false,
    },
    ...partial,
  } as VerdictCard;
}

describe("buildDebateBrief", () => {
  it("surfaces the retrieved referent for this claim", () => {
    const idx = new SessionIndex();
    idx.addTurn("t1", "Unemployment is 4.2% according to the ONS", 0);
    const brief = buildDebateBrief(idx, "that 4.2 percent number is out of date", 1000);
    expect(brief).toMatch(/RELEVANT EARLIER/);
    expect(brief).toMatch(/4\.2%/);
  });

  it("lists disputed standing claims before settled ones", () => {
    const idx = new SessionIndex();
    idx.addVerdict(verdict({ id: "v-false", claim: "Crime doubled last year", verdict: "false", createdAt: 10 }));
    idx.addVerdict(verdict({ id: "v-true", claim: "The capital is Paris", verdict: "verified", createdAt: 20 }));
    // A claim that retrieves neither, so both land in 'standing'.
    const brief = buildDebateBrief(idx, "completely unrelated topic here", 100);
    const falseIdx = brief.indexOf("Crime doubled");
    const trueIdx = brief.indexOf("capital is Paris");
    expect(falseIdx).toBeGreaterThanOrEqual(0);
    expect(trueIdx).toBeGreaterThanOrEqual(0);
    expect(falseIdx).toBeLessThan(trueIdx); // disputed first
  });

  it("never exceeds the character cap", () => {
    const idx = new SessionIndex();
    for (let i = 0; i < 100; i++) {
      idx.addVerdict(verdict({
        id: `v${i}`,
        claim: `Claim number ${i} with a lot of padding text to make the brief long ${"x".repeat(40)}`,
        verdict: i % 2 ? "false" : "verified",
        createdAt: i,
      }));
    }
    const brief = buildDebateBrief(idx, "some query", 1000);
    expect(brief.length).toBeLessThanOrEqual(BRIEF_MAX_CHARS);
  });

  it("is empty when there is nothing to say", () => {
    const idx = new SessionIndex();
    expect(buildDebateBrief(idx, "anything", 0)).toBe("");
  });
});
