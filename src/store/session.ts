// Session store (orchestration). Wires the pipeline described in spec §3:
//   transcript → ClaimBuffer (batching) → SessionIndex (retrieval) → debate brief →
//   verify_claim (Rust) → verdict → dashboard + HUD.
//
// Two behaviours here are load-bearing and each maps to a spec failure mode:
//   * Publish ONLY false + misleading to the HUD, and scan EVERY check when deciding
//     what to publish — up to three verifications run concurrently and they do not
//     complete in order, so "look at the newest result" misses alarms.
//   * On a self-correction, WITHDRAW in-flight verifications before running the
//     corrected claim, but never delete a verdict that already landed (it records
//     something genuinely said; removing it makes the HUD disagree with the transcript).

import { create } from "zustand";
import { ClaimBuffer, type ClaimEvent } from "../lib/claimBuffer";
import { SessionIndex } from "../lib/sessionIndex";
import { buildDebateBrief } from "../lib/debateBrief";
import { isAlert, type TranscriptUtterance, type VerdictCard } from "../lib/types";
import { ipc } from "../lib/ipc";
import { useSettings } from "./settings";

// Non-reactive pipeline state kept out of the store so feeding it doesn't re-render.
let buffer = new ClaimBuffer({ verifyEveryWords: useSettings.getState().verifyEveryWords });
const index = new SessionIndex();

interface InFlight {
  claim: string;
  withdrawn: boolean;
}
const inFlight = new Map<string, InFlight>();
let uttSeq = 0;

interface SessionState {
  live: boolean;
  transcript: TranscriptUtterance[];
  partial: string;
  cards: VerdictCard[];
  alerts: VerdictCard[]; // derived: false/misleading, not withdrawn — the HUD reads this
  inFlightCount: number;

  start: () => void;
  stop: () => void;
  ingestPartial: (text: string) => void;
  ingestFinal: (text: string) => void;
  pollIdle: () => void;
  clear: () => void;
}

export const useSession = create<SessionState>((set, get) => ({
  live: false,
  transcript: [],
  partial: "",
  cards: [],
  alerts: [],
  inFlightCount: 0,

  start: () => {
    buffer = new ClaimBuffer({ verifyEveryWords: useSettings.getState().verifyEveryWords });
    index.reset();
    inFlight.clear();
    set({ live: true, transcript: [], partial: "", cards: [], alerts: [], inFlightCount: 0 });
  },

  stop: () => {
    const events = buffer.end();
    handleEvents(events, set);
    set({ live: false, partial: "" });
  },

  ingestPartial: (text) => set({ partial: text }),

  ingestFinal: (text) => {
    const trimmed = text.trim();
    if (!trimmed) return;
    const utt: TranscriptUtterance = {
      id: `utt-${Date.now()}-${uttSeq++}`,
      text: trimmed,
      at: Date.now(),
      final: true,
    };
    // Index every finalized turn for retrieval (a rebuttal may refer back to it).
    index.addTurn(utt.id, utt.text, utt.at);
    set((s) => ({ transcript: [...s.transcript, utt], partial: "" }));
    const events = buffer.push({ id: utt.id, text: utt.text, at: utt.at });
    handleEvents(events, set);
  },

  pollIdle: () => {
    if (!get().live) return;
    const events = buffer.poll();
    handleEvents(events, set);
  },

  clear: () => {
    index.reset();
    inFlight.clear();
    set({ transcript: [], cards: [], alerts: [], partial: "", inFlightCount: 0 });
  },
}));

type SetFn = (partial: Partial<SessionState> | ((s: SessionState) => Partial<SessionState>)) => void;

function handleEvents(events: ClaimEvent[], set: SetFn) {
  for (const ev of events) {
    if (ev.type === "repair") {
      // A correction supersedes anything still in flight — withdraw those first.
      for (const [, f] of inFlight) f.withdrawn = true;
      // Landed verdicts are NOT touched here.
      runVerification(ev.claim.id, ev.claim.text, set);
    } else if (ev.type === "flush") {
      runVerification(ev.claim.id, ev.claim.text, set);
    }
  }
}

async function runVerification(claimId: string, claim: string, set: SetFn) {
  const s = useSettings.getState();
  const keys = s.effectiveKeys();
  const brief = buildDebateBrief(index, claim, Date.now());

  inFlight.set(claimId, { claim, withdrawn: false });
  set({ inFlightCount: inFlight.size });

  let card: VerdictCard | null = null;
  try {
    card = await ipc.verifyClaim({
      claim,
      brief,
      openai_key: keys.openai,
      anthropic_key: keys.anthropic,
      exa_key: keys.exa,
      web_search_enabled: s.webSearchEnabled,
      provider: s.verificationProvider,
      openai_model: s.verificationModel,
      anthropic_model: s.anthropicModel,
    });
  } catch (e) {
    const msg = String(e);
    // SATURATED: all 3 slots busy — drop rather than queue (a late verdict is useless).
    // NO_PROVIDER: no key configured — nothing to do.
    if (!msg.includes("SATURATED") && !msg.includes("NO_PROVIDER")) {
      console.error("verify_claim failed:", msg);
    }
    inFlight.delete(claimId);
    set({ inFlightCount: inFlight.size });
    return;
  }

  const wasWithdrawn = inFlight.get(claimId)?.withdrawn ?? false;
  inFlight.delete(claimId);

  const finalCard: VerdictCard = { ...card, id: claimId, withdrawn: wasWithdrawn };
  // Only index (for retrieval) verdicts that actually stand.
  if (!wasWithdrawn) index.addVerdict(finalCard);

  set((state) => {
    const cards = [...state.cards, finalCard];
    // Scan EVERY standing check — not just this one — to decide the alert set.
    const alerts = cards.filter((c) => !c.withdrawn && isAlert(c.verdict));
    return { cards, alerts, inFlightCount: inFlight.size };
  });
}

// Test hook: reset module-level pipeline state between tests.
export function __resetPipeline() {
  buffer = new ClaimBuffer({ verifyEveryWords: 50 });
  index.reset();
  inFlight.clear();
}
