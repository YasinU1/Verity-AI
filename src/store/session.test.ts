import { describe, it, expect, beforeEach, vi } from "vitest";
import type { Verdict, VerdictCard } from "../lib/types";

// Control ipc.verifyClaim with manually-resolved deferreds so we can drive verifications
// out of order and mid-flight, exactly like the concurrent, unordered real pipeline.
const h = vi.hoisted(() => {
  const queue: { resolve: (c: VerdictCard) => void; claim: string }[] = [];
  return {
    queue,
    verifyClaim: (req: { claim: string }) =>
      new Promise<VerdictCard>((resolve) => h.queue.push({ resolve, claim: req.claim })),
  };
});

vi.mock("../lib/ipc", () => ({
  inTauri: () => false,
  onTranscript: () => Promise.resolve(() => {}),
  ipc: { verifyClaim: h.verifyClaim },
}));

import { useSession } from "./session";

function card(claim: string, verdict: Verdict): VerdictCard {
  return {
    id: "x",
    claim,
    verdict,
    rationale: "",
    correction: null,
    createdAt: Date.now(),
    provenance: {
      webSearchUsed: true,
      provider: "openai",
      model: "gpt-4o-mini",
      latencyMs: 10,
      sources: [],
      prefetched: false,
    },
  };
}

const tick = () => new Promise((r) => setTimeout(r, 0));

function resolveFor(match: string, verdict: Verdict) {
  const idx = h.queue.findIndex((d) => d.claim.includes(match));
  if (idx < 0) throw new Error(`no in-flight verification matching "${match}"`);
  const d = h.queue.splice(idx, 1)[0];
  d.resolve(card(d.claim, verdict));
}

describe("session pipeline", () => {
  beforeEach(() => {
    h.queue.length = 0;
    useSession.getState().start();
  });

  it("publishes only false/misleading to the alert set", async () => {
    const s = useSession.getState();
    s.ingestFinal("Crime rose 30 percent."); // complete → flushes → verify
    await tick();
    resolveFor("30 percent", "false");
    await tick();

    s.ingestFinal("GDP grew 2 percent.");
    await tick();
    resolveFor("2 percent", "verified");
    await tick();

    const st = useSession.getState();
    expect(st.cards.length).toBe(2);
    expect(st.alerts.length).toBe(1); // only the false one
    expect(st.alerts[0].verdict).toBe("false");
  });

  it("scans EVERY check, not just the newest, when deciding alerts", async () => {
    const s = useSession.getState();
    s.ingestFinal("Crime rose 30 percent."); // A
    await tick();
    s.ingestFinal("GDP grew 2 percent."); // B
    await tick();

    // Resolve the misleading FIRST, the benign one LAST — a "check newest" gate would
    // miss the alarm because the last-resolved verdict is verified.
    resolveFor("30 percent", "misleading");
    await tick();
    resolveFor("2 percent", "verified");
    await tick();

    const st = useSession.getState();
    expect(st.alerts.length).toBe(1);
    expect(st.alerts[0].claim).toContain("30 percent");
  });

  it("withdraws an in-flight verification on a self-correction, keeping the corrected one", async () => {
    const s = useSession.getState();
    s.ingestFinal("It was 3.2 percent."); // flushes → verification A in flight
    await tick();
    expect(h.queue.some((d) => d.claim.includes("3.2"))).toBe(true);

    // A breath later, the speaker corrects themselves.
    s.ingestFinal("actually, 2.3 percent."); // repair → withdraw A, run corrected B
    await tick();

    // The withdrawn original lands but must NOT become an alert.
    resolveFor("3.2", "false");
    await tick();
    // The corrected claim verifies and IS published.
    resolveFor("2.3", "false");
    await tick();

    const st = useSession.getState();
    const withdrawn = st.cards.find((c) => c.claim.includes("3.2"));
    expect(withdrawn?.withdrawn).toBe(true);
    expect(st.alerts.length).toBe(1);
    expect(st.alerts[0].claim).toContain("2.3");
  });

  it("does not publish anything before a verdict lands", async () => {
    const s = useSession.getState();
    s.ingestFinal("Unemployment is 8 percent.");
    await tick();
    // In flight, nothing resolved yet.
    expect(useSession.getState().alerts.length).toBe(0);
    expect(useSession.getState().inFlightCount).toBe(1);
  });
});
