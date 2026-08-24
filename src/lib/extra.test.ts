import { describe, it, expect } from "vitest";
import { extractNumbers } from "./numbers";
import { ClaimBuffer, hasAssertionMarker, endsIncomplete } from "./claimBuffer";
import { SessionIndex } from "./sessionIndex";
import { buildDebateBrief } from "./debateBrief";
import type { ClaimEvent } from "./claimBuffer";

function flushes(events: ClaimEvent[]): string[] {
  return events.filter((e) => e.type === "flush").map((e) => (e as { claim: { text: string } }).claim.text);
}
function reasons(events: ClaimEvent[]): string[] {
  return events.filter((e) => e.type === "flush").map((e) => (e as { reason: string }).reason);
}

describe("numbers — more forms", () => {
  it("handles ninety and compound ninety-nine", () => {
    expect(extractNumbers("ninety")).toContain("90");
    expect(extractNumbers("ninety nine")).toContain("99");
  });
  it("handles currency and thousands together", () => {
    const n = extractNumbers("$1,200 was spent");
    expect(n).toContain("1200");
  });
  it("a decimal percent and a spoken one collide", () => {
    const a = new Set(extractNumbers("2.3%"));
    expect(a.has("2.3")).toBe(true);
  });
});

describe("claim buffer — flush reasons", () => {
  it("a long declarative run flushes with reason 'words'", () => {
    let t = 0;
    const buf = new ClaimBuffer({ now: () => t, verifyEveryWords: 20 });
    // >20 plain declarative words, no assertion marker → words-trigger flush.
    const text =
      "the committee met in the old hall down near the river bank to look over the plans they had drawn up for the coming spring season here";
    expect(text.split(/\s+/).length).toBeGreaterThan(20);
    const ev = buf.push({ id: "u1", text });
    expect(reasons(ev)).toContain("words");
  });

  it("session end flushes with reason 'session_end'", () => {
    let t = 0;
    const buf = new ClaimBuffer({ now: () => t });
    buf.push({ id: "u1", text: "the hall was full of people today" });
    expect(reasons(buf.end())).toEqual(["session_end"]);
  });

  it("an assertion marker mid-utterance still waits for the sentence to land", () => {
    let t = 0;
    const buf = new ClaimBuffer({ now: () => t });
    // 'rose' arms; ends on 'than' → incomplete → held.
    expect(flushes(buf.push({ id: "u1", text: "unemployment rose higher than" }))).toEqual([]);
    t += 100;
    expect(flushes(buf.push({ id: "u2", text: "it was last year." })).length).toBe(1);
  });
});

describe("marker / completeness edge cases", () => {
  it("superlatives and comparisons arm", () => {
    expect(hasAssertionMarker("it was the largest ever recorded")).toBe(true);
    expect(hasAssertionMarker("more than half agreed")).toBe(true);
  });
  it("a trailing article reads incomplete", () => {
    expect(endsIncomplete("the winner was the")).toBe(true);
  });
});

describe("retrieval + brief integration", () => {
  it("a disputed verdict repeated later is retrievable and leads the standing brief", () => {
    const idx = new SessionIndex();
    idx.addVerdict({
      id: "v1",
      claim: "Crime doubled since 2010",
      verdict: "false",
      rationale: "Crime fell over the period.",
      correction: "It fell ~20%",
      createdAt: 0,
      provenance: { webSearchUsed: true, provider: "openai", model: "m", latencyMs: 1, sources: [], prefetched: false },
    });
    // Speaker repeats the debunked framing.
    const brief = buildDebateBrief(idx, "as I said, crime doubled since 2010", 1000);
    expect(brief).toMatch(/FALSE/);
    expect(brief).toMatch(/doubled/);
  });

  it("retrieval prefers the record sharing a year literal", () => {
    const idx = new SessionIndex();
    idx.addTurn("a", "their figure is from 2019", 0);
    idx.addTurn("b", "we talked about the weather", 10);
    const out = idx.retrieve("the 2019 number is stale", 100);
    expect(out[0].record.id).toBe("a");
  });

  it("a shared number outranks a record sharing only common tokens", () => {
    const idx = new SessionIndex();
    idx.addTurn("num", "the total was 512 people", 0);
    idx.addTurn("common", "the people in the room were people people", 10);
    const out = idx.retrieve("that 512 people figure is wrong", 100);
    expect(out[0].record.id).toBe("num");
  });

  it("brief is empty with no verdicts and no lexical match", () => {
    const idx = new SessionIndex();
    idx.addTurn("t", "the sky is a nice colour", 0);
    expect(buildDebateBrief(idx, "unrelated economics claim", 1000)).toBe("");
  });
});

describe("youtube — additional hosts and shapes", () => {
  it("gaming and music subdomains resolve", async () => {
    const { parseYouTube } = await import("./youtube");
    expect(parseYouTube("https://gaming.youtube.com/watch?v=dQw4w9WgXcQ")?.videoId).toBe("dQw4w9WgXcQ");
    expect(parseYouTube("https://music.youtube.com/watch?v=dQw4w9WgXcQ")?.videoId).toBe("dQw4w9WgXcQ");
  });
});

describe("claim buffer — filler and disfluency handling", () => {
  it("a filler word inside a real claim does not suppress the whole claim", async () => {
    const { ClaimBuffer } = await import("./claimBuffer");
    let t = 0;
    const buf = new ClaimBuffer({ now: () => t });
    const ev = buf.push({ id: "u1", text: "yeah unemployment is 8 percent." });
    // 'yeah' leads but the sentence carries a real figure → still flushes.
    expect(flushes(ev).length).toBe(1);
  });

  it("a repair with no corrected value emits nothing to run", async () => {
    const { ClaimBuffer } = await import("./claimBuffer");
    let t = 0;
    const buf = new ClaimBuffer({ now: () => t });
    // A repair opener with nothing after it strips to empty → nothing to run/withdraw.
    const ev = buf.push({ id: "u1", text: "actually," });
    expect(ev.length).toBe(0);
  });
});
