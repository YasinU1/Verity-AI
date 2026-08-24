// Shared types across the frontend. Kept provider-agnostic and IPC-agnostic so the
// logic modules (claimBuffer, sessionIndex, debateBrief) can be unit-tested in
// isolation with no backend.

export type Verdict =
  | "verified" // the claim is accurate
  | "false" // contradicted by the evidence
  | "misleading" // technically defensible but creates a false impression
  | "context_needed" // depends on a definition, timeframe, or baseline not stated
  | "unverifiable"; // no evidence either way, or it's opinion/prediction/value judgement

// Only these two are published to the HUD — they are the alarms. Both render red.
export const HUD_VERDICTS: readonly Verdict[] = ["false", "misleading"] as const;

export function isAlert(v: Verdict): boolean {
  return HUD_VERDICTS.includes(v);
}

export interface TranscriptUtterance {
  id: string;
  text: string;
  /** Wall-clock ms when finalized. Injectable in tests so timing is deterministic. */
  at: number;
  final: boolean;
}

export interface Source {
  title: string;
  url: string;
  /** Exa publishedDate, if any — recency decides a whole class of claims. */
  publishedDate?: string | null;
  /** Snippet text (Exa contents.text). */
  text?: string;
}

export interface Provenance {
  /** Whether a live web search actually backed this verdict, vs. model memory. */
  webSearchUsed: boolean;
  provider: "openai" | "anthropic";
  model: string;
  latencyMs: number;
  sources: Source[];
  /** Sources that were prefetched and seeded even though the model never called
   * the tool — recorded so a verdict resting on them doesn't appear from nowhere. */
  prefetched: boolean;
}

export interface VerdictCard {
  id: string;
  claim: string;
  verdict: Verdict;
  rationale: string;
  /** The corrected real number/fact, when the claim carried a false impression. */
  correction?: string | null;
  provenance: Provenance;
  createdAt: number;
  /** True while a verification is in-flight; withdrawn if superseded by a repair. */
  withdrawn?: boolean;
}

export interface Claim {
  id: string;
  text: string;
  /** Utterance ids this claim was assembled from. */
  sourceUtteranceIds: string[];
  createdAt: number;
  /** Set when the batch was flagged as a self-correction of an earlier claim. */
  isRepair?: boolean;
}
