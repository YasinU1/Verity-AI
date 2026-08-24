// Debate brief (spec §7).
//
// Assembled MECHANICALLY from the session index, not summarized by an LLM. The
// trade-off: a mechanical brief can read choppily and won't paraphrase, but it costs
// no round trip (latency is a correctness constraint here) and it cannot hallucinate
// a "settled" fact that was never actually established — an LLM summary can.
//
// Two parts:
//   retrieved — records lexically relevant to THIS claim (the referent it points at).
//   standing  — what's already been settled this session, DISPUTED claims first: a
//               speaker repeating a debunked figure is the single most useful thing to
//               surface, so it leads.
// Capped (~1400 chars) so the brief can never crowd out the actual evidence.

import type { SessionIndex, IndexRecord } from "./sessionIndex";

export const BRIEF_MAX_CHARS = 1400;

function verdictTag(rec: IndexRecord): string {
  if (rec.kind !== "verdict" || !rec.verdict) return "";
  return `[${rec.verdict.toUpperCase()}] `;
}

function oneLine(text: string, max = 180): string {
  const clean = text.replace(/\s+/g, " ").trim();
  return clean.length > max ? clean.slice(0, max - 1) + "…" : clean;
}

export interface BriefOptions {
  maxChars?: number;
  retrievedLimit?: number;
  standingLimit?: number;
}

export function buildDebateBrief(
  index: SessionIndex,
  claimText: string,
  now: number,
  opts: BriefOptions = {},
): string {
  const maxChars = opts.maxChars ?? BRIEF_MAX_CHARS;
  const retrievedLimit = opts.retrievedLimit ?? 4;
  const standingLimit = opts.standingLimit ?? 6;

  const retrieved = index.retrieve(claimText, now, retrievedLimit);
  const retrievedIds = new Set(retrieved.map((r) => r.record.id));

  // Standing: completed verdicts, disputed first, excluding anything already shown
  // under "retrieved" so we don't repeat ourselves inside the cap.
  const standing = index
    .all()
    .filter((r): r is IndexRecord => r.kind === "verdict" && !retrievedIds.has(r.id))
    .sort((a, b) => {
      if (!!a.disputed !== !!b.disputed) return a.disputed ? -1 : 1; // disputed first
      return b.at - a.at;
    })
    .slice(0, standingLimit);

  const lines: string[] = [];

  if (retrieved.length > 0) {
    lines.push("RELEVANT EARLIER:");
    for (const { record } of retrieved) {
      lines.push(`- ${verdictTag(record)}${oneLine(record.text)}`);
    }
  }

  if (standing.length > 0) {
    lines.push("ALREADY ESTABLISHED (disputed first):");
    for (const record of standing) {
      lines.push(`- ${verdictTag(record)}${oneLine(record.text)}`);
    }
  }

  // Enforce the cap by dropping WHOLE lines from the end (keeps the brief coherent —
  // retrieved-for-this-claim leads, so the tail we drop is the least relevant).
  let brief = "";
  for (const line of lines) {
    const next = brief ? brief + "\n" + line : line;
    if (next.length > maxChars) break;
    brief = next;
  }
  return brief;
}
