import { describe, it, expect } from "vitest";
import {
  ClaimBuffer,
  ClaimEvent,
  endsIncomplete,
  hasAssertionMarker,
  hasDeclarativeSubstance,
  isFillerOnly,
  isOpinionOpener,
  isQuestion,
  startsWithRepair,
  stripDisfluencies,
  stripRepairOpener,
  INCOMPLETE_HOLD_MS,
  IDLE_FLUSH_MS,
  ARMED_HARD_WORD_CAP,
} from "./claimBuffer";

// A deterministic clock so idle-timeout behaviour is testable without real time.
function clockBuffer(verifyEveryWords?: number) {
  let t = 0;
  const now = () => t;
  const buf = new ClaimBuffer({ now, verifyEveryWords });
  let n = 0;
  const push = (text: string) => buf.push({ id: `u${n++}`, text, at: t });
  const advance = (ms: number) => (t += ms);
  return { buf, push, advance, at: () => t };
}

function flushes(events: ClaimEvent[]): string[] {
  return events.filter((e) => e.type === "flush").map((e) => (e as any).claim.text);
}

describe("assertion markers (spelled numerals count)", () => {
  it("fires on a digit figure", () => {
    expect(hasAssertionMarker("unemployment is 8%")).toBe(true);
  });
  it("fires on a spelled-out numeral", () => {
    expect(hasAssertionMarker("they hired forty people")).toBe(true);
  });
  it("fires on quantifiers, absolutes, attribution, direction", () => {
    expect(hasAssertionMarker("the vast majority agree")).toBe(true);
    expect(hasAssertionMarker("nobody voted for it")).toBe(true);
    expect(hasAssertionMarker("the ONS said so")).toBe(true);
    expect(hasAssertionMarker("prices rose sharply")).toBe(true);
  });
  it("does not fire on a plain declarative", () => {
    expect(hasAssertionMarker("the meeting is on Tuesday")).toBe(false);
  });
});

describe("sentence completeness", () => {
  it("terminal punctuation reads complete", () => {
    expect(endsIncomplete("it was 3.2 percent.")).toBe(false);
  });
  it("a dangling connective reads incomplete", () => {
    expect(endsIncomplete("they cut it by about")).toBe(true);
    expect(endsIncomplete("it is higher than")).toBe(true);
  });
  it("a bare trailing numeral reads incomplete (waiting for its unit)", () => {
    expect(endsIncomplete("they cut it by about forty")).toBe(true);
    expect(endsIncomplete("it dropped to 40")).toBe(true);
  });
  it("a trailing hesitation reads incomplete", () => {
    expect(endsIncomplete("the figure is, um")).toBe(true);
  });
  it("a normal ending reads complete (bias toward flushing)", () => {
    expect(endsIncomplete("unemployment is eight percent")).toBe(false);
  });
});

describe("ClaimBuffer — waits for the rest of a figure", () => {
  it("holds an armed but incomplete claim, then flushes when it lands", () => {
    const { push } = clockBuffer();
    // "forty" is checkable-looking but half a claim — must NOT flush yet.
    const held = push("they cut it by about forty");
    expect(flushes(held)).toEqual([]);
    // The rest arrives and the sentence lands.
    const done = push("thousand jobs.");
    expect(flushes(done).length).toBe(1);
    expect(flushes(done)[0]).toMatch(/forty thousand jobs/);
  });
});

describe("ClaimBuffer — a completed sentence flushes immediately on a marker", () => {
  it("flushes 'It was 3.2 percent.'", () => {
    const { push } = clockBuffer();
    const ev = push("It was 3.2 percent.");
    expect(flushes(ev).length).toBe(1);
  });
});

describe("ClaimBuffer — doesn't stall forever", () => {
  it("flushes at the hard word cap even if the sentence never lands", () => {
    const { push } = clockBuffer();
    const filler = new Array(ARMED_HARD_WORD_CAP + 5).fill("data").join(" ");
    // Arm with a marker, end on a dangling connective so it stays 'incomplete'.
    const ev = push(`prices rose ${filler} and`);
    expect(flushes(ev).length).toBe(1);
  });
});

describe("ClaimBuffer — two idle timeouts", () => {
  it("an armed, incomplete batch flushes after the SHORT hold, not 12s", () => {
    const { push, advance, buf } = clockBuffer();
    push("unemployment rose to about"); // armed (rose), incomplete (dangling 'about')
    expect(buf.isArmed).toBe(true);
    advance(INCOMPLETE_HOLD_MS - 1);
    expect(flushes(buf.poll())).toEqual([]); // still held — armed batch kept
    advance(2);
    expect(flushes(buf.poll()).length).toBe(1); // flushed via the incomplete hold
  });

  it("an unarmed batch waits the LONG idle timeout", () => {
    const { push, advance, buf } = clockBuffer();
    push("the meeting ran quite late today"); // no marker → not armed
    advance(IDLE_FLUSH_MS - 1);
    expect(flushes(buf.poll())).toEqual([]);
    advance(2);
    expect(flushes(buf.poll()).length).toBe(1);
  });
});

describe("ClaimBuffer — disfluency stripping", () => {
  it("removes 'um' from the emitted claim", () => {
    const { push } = clockBuffer();
    const ev = push("unemployment is, um, eight percent.");
    const text = flushes(ev)[0];
    expect(text).toBeDefined();
    expect(text).not.toMatch(/\bum\b/);
    expect(text).toMatch(/unemployment is/);
  });
});

describe("stripDisfluencies", () => {
  it("cleans hesitations and the punctuation they leave", () => {
    expect(stripDisfluencies("it is, um, high")).toBe("it is, high");
    expect(stripDisfluencies("you know it rose")).toBe("it rose");
  });
});

describe("self-correction / repair openers", () => {
  it("detects a repair opener ONLY at the start", () => {
    expect(startsWithRepair("actually, it was 2.3")).toBe(true);
    expect(startsWithRepair("sorry, I misspoke, 2.3")).toBe(true);
    expect(startsWithRepair("no wait, 2.3")).toBe(true);
    // Mid-sentence 'actually' is an intensifier, not a retraction.
    expect(startsWithRepair("I was actually quite surprised by the number")).toBe(false);
  });

  it("strips the opener from the corrected claim", () => {
    expect(stripRepairOpener("actually, 2.3")).toBe("2.3");
    expect(stripRepairOpener("I mean, 2.3 percent")).toBe("2.3 percent");
  });

  it("emits a repair event that withdraws in-flight verifications", () => {
    const { push } = clockBuffer();
    const first = push("It was 3.2 percent."); // flushes → a verification starts
    expect(flushes(first).length).toBe(1);
    const repair = push("actually, 2.3.");
    const rep = repair.find((e) => e.type === "repair");
    expect(rep).toBeDefined();
    expect((rep as any).withdrawInFlight).toBe(true);
    expect((rep as any).claim.isRepair).toBe(true);
    expect((rep as any).claim.text).not.toMatch(/^actually/i);
  });
});

describe("suppression", () => {
  it("filler-only utterances never form a claim", () => {
    const { push, buf } = clockBuffer();
    expect(isFillerOnly("yeah")).toBe(true);
    expect(flushes(push("yeah"))).toEqual([]);
    expect(buf.pendingText).toBe("");
  });

  it("questions never arm or flush", () => {
    const { push, advance, buf } = clockBuffer();
    expect(isQuestion("is it really 8 percent?")).toBe(true);
    push("is it really 8 percent?");
    expect(buf.isArmed).toBe(false);
    advance(IDLE_FLUSH_MS + 1);
    expect(flushes(buf.poll())).toEqual([]);
  });

  it("opinion openers without a figure lack declarative substance", () => {
    expect(isOpinionOpener("I think we should raise taxes")).toBe(true);
    expect(hasDeclarativeSubstance("I think we should raise taxes")).toBe(false);
    // ...but an opinion carrying a real figure is still checkable.
    expect(hasDeclarativeSubstance("I think unemployment is 8 percent")).toBe(true);
  });
});

describe("front-drop keeps the newest words when a batch runs long", () => {
  it("caps accumulated words for a never-flushing run", () => {
    const { push, buf } = clockBuffer(200);
    // A long run of questions: never arms, never flushes — the front-drop is the
    // only thing bounding it.
    for (let i = 0; i < 60; i++) push("why did they choose that particular option?");
    const words = buf.pendingText.split(/\s+/).filter(Boolean).length;
    expect(words).toBeLessThanOrEqual(200);
  });
});

describe("session end", () => {
  it("flushes remaining substantive content", () => {
    const { push, buf } = clockBuffer();
    push("the weather was quite pleasant today"); // no marker, below threshold
    const ev = buf.end();
    expect(flushes(ev).length).toBe(1);
  });
  it("drops trailing filler with no substance", () => {
    const { push, buf } = clockBuffer();
    push("yeah");
    expect(buf.end()).toEqual([]);
  });
});
