import { describe, it, expect } from "vitest";
import {
  SessionIndex,
  extractProperNouns,
  extractTokens,
  isReferential,
} from "./sessionIndex";
import type { VerdictCard } from "./types";

function verdict(partial: Partial<VerdictCard> & { id: string; claim: string }): VerdictCard {
  return {
    verdict: "false",
    rationale: "",
    createdAt: 0,
    provenance: {
      webSearchUsed: true,
      provider: "openai",
      model: "gpt-4o-mini",
      latencyMs: 0,
      sources: [],
      prefetched: false,
    },
    ...partial,
  } as VerdictCard;
}

describe("helpers", () => {
  it("extractProperNouns picks names and acronyms, drops sentence-initial common words", () => {
    const nouns = extractProperNouns("The ONS reported that Britain grew");
    expect(nouns).toContain("ons");
    expect(nouns).toContain("britain");
    expect(nouns).not.toContain("the"); // sentence-initial, capitalized only by position
  });

  it("extractTokens drops stopwords and pure numbers", () => {
    const toks = extractTokens("the unemployment rate is 4.2 percent");
    expect(toks).toContain("unemployment");
    expect(toks).toContain("rate");
    expect(toks).not.toContain("the");
    expect(toks).not.toContain("4.2");
  });

  it("isReferential detects pointers to earlier statements", () => {
    expect(isReferential("that number ignores people who stopped looking")).toBe(true);
    expect(isReferential("their figure is from 2019")).toBe(true);
    expect(isReferential("he said it was higher")).toBe(true);
    expect(isReferential("unemployment is 8 percent")).toBe(false);
  });
});

describe("SessionIndex — finds a rebuttal sharing only a figure", () => {
  it("ranks the earlier statement top by the shared number alone", () => {
    const idx = new SessionIndex();
    idx.addTurn("t1", "Unemployment is 4.2% according to the ONS", 0);
    idx.addTurn("t2", "I had eggs for breakfast this morning", 1000);
    idx.addTurn("t3", "The bus was late again today", 2000);

    // The rebuttal shares the figure 4.2 but almost no content words with t1.
    const out = idx.retrieve("the 4.2 percent claim is from an old survey", 3000);
    expect(out[0].record.id).toBe("t1");
  });
});

describe("SessionIndex — recency is a tie-breaker, not the ranking", () => {
  it("does not bury an old numeric match under fresh chatter", () => {
    const idx = new SessionIndex();
    idx.addTurn("old", "The deficit was 4.2 billion in the last report", 0);
    // Lots of recent chatter sharing generic words but NO figure.
    for (let i = 0; i < 10; i++) {
      idx.addTurn(`chat${i}`, "well the report is interesting and worth a look", 100_000 + i * 1000);
    }
    const now = 200_000;
    const out = idx.retrieve("that 4.2 billion figure is disputed", now);
    expect(out[0].record.id).toBe("old"); // the number match wins despite being old
  });
});

describe("SessionIndex — referential claims boost records carrying a figure", () => {
  it("prefers a numbered record when the query merely points at 'that figure'", () => {
    const idx = new SessionIndex();
    idx.addTurn("num", "The rate came in at 4.2 percent last quarter", 0);
    idx.addTurn("nonum", "The committee discussed the rate at length", 1000);
    const out = idx.retrieve("that figure is misleading", 2000);
    expect(out[0].record.id).toBe("num");
  });
});

describe("SessionIndex — ignores in-progress / withdrawn verdicts", () => {
  it("does not index a withdrawn verdict", () => {
    const idx = new SessionIndex();
    idx.addTurn("t1", "Inflation hit 9 percent in October", 0);
    idx.addVerdict(verdict({ id: "v-withdrawn", claim: "Inflation hit 9 percent", withdrawn: true }));
    expect(idx.size).toBe(1); // the turn only
    const out = idx.retrieve("that 9 percent inflation figure", 1000);
    expect(out.every((r) => r.record.id !== "v-withdrawn")).toBe(true);
  });

  it("indexes a completed verdict and marks disputed ones", () => {
    const idx = new SessionIndex();
    idx.addVerdict(verdict({ id: "v1", claim: "GDP fell 3 percent", verdict: "false" }));
    idx.addVerdict(verdict({ id: "v2", claim: "The sky is blue", verdict: "verified" }));
    expect(idx.size).toBe(2);
    const disputed = idx.all().find((r) => r.id === "v1");
    expect(disputed?.disputed).toBe(true);
    const settled = idx.all().find((r) => r.id === "v2");
    expect(settled?.disputed).toBe(false);
  });
});
