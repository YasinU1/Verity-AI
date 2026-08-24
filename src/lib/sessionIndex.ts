// Session retrieval (spec §7).
//
// Half of what's said in a debate refers to something said earlier: "that number
// ignores people who stopped looking", "their figure is from 2019". Those are
// checkable — but only once the referent is known. We index every finalized turn and
// completed verdict, then retrieve against the claim being checked.
//
// Scoring is LEXICAL, not embeddings — a deliberate trade-off. What identifies the
// relevant earlier moment is almost always a literal (4.2%, ONS, 2019), and dense
// retrieval blurs exactly those: "4.2%" and "2.3%" are neighbours in embedding space,
// which is fatal when which of the two was said is the entire dispute. Lexical is also
// free: no embedding call, no index to warm, no latency. Cost: no synonym/paraphrase
// matching — "unemployment" won't match "joblessness". Acceptable, because the linking
// token in a rebuttal is a shared literal, not a paraphrase.

import type { Verdict, VerdictCard } from "./types";
import { extractNumbers } from "./numbers";

// Weights — numbers dominate on purpose (the only unambiguous link between two
// statements); recency is a tie-breaker, never the ranking.
const W_NUMBER = 3.0;
const W_PROPER = 1.6;
const W_TOKEN = 0.6; // IDF-weighted per shared content token
const W_RECENCY_MAX = 0.6;
// A referential query ("that number", "their figure") cannot be checked without the
// figure it points at, so boost records carrying one.
const W_REFERENTIAL_FIGURE = 2.5;

const STOPWORDS = new Set([
  "the", "a", "an", "and", "or", "but", "of", "to", "in", "on", "at", "for",
  "with", "as", "by", "is", "are", "was", "were", "be", "been", "being", "it",
  "its", "this", "that", "these", "those", "they", "them", "their", "he", "she",
  "we", "you", "i", "him", "her", "us", "our", "your", "my", "me", "so", "if",
  "than", "then", "there", "here", "not", "no", "yes", "do", "does", "did",
  "have", "has", "had", "will", "would", "can", "could", "should", "about",
  "from", "into", "over", "under", "more", "most", "less", "very", "just",
  "also", "which", "who", "what", "when", "where", "how", "why", "because",
]);

export type IndexKind = "turn" | "verdict";

export interface IndexRecord {
  id: string;
  kind: IndexKind;
  text: string;
  at: number;
  numbers: string[];
  properNouns: string[]; // lowercased for matching
  tokens: string[]; // content tokens, lowercased, stopwords removed
  verdict?: Verdict;
  /** false / misleading => disputed; standing brief lists these first. */
  disputed?: boolean;
}

export interface ScoredRecord {
  record: IndexRecord;
  score: number;
}

// "that number", "their figure", "he said", "that's not true" — a statement that
// points at an earlier one. The pointer, not the content, is what makes it checkable.
const REFERENTIAL =
  /\b(that (number|figure|stat|statistic|claim|point|study)|their (number|figure|stat|claim|data)|his figure|her figure|that's not true|that is not true|he said|she said|they said|as (he|she|they) said|the same (number|figure)|earlier|previously|you said|you claimed)\b/i;

export function isReferential(text: string): boolean {
  return REFERENTIAL.test(text);
}

export function extractProperNouns(text: string): string[] {
  const out: string[] = [];
  const words = text.split(/\s+/);
  words.forEach((w, i) => {
    const clean = w.replace(/[^A-Za-z]/g, "");
    if (!clean) return;
    const isAcronym = /^[A-Z]{2,}$/.test(clean); // ONS, GDP, NHS
    const isCapWord = /^[A-Z][a-z]+$/.test(clean);
    if (!isAcronym && !isCapWord) return;
    // Drop a sentence-initial capitalized common word (it's capitalized only because
    // it starts the sentence, not because it's a name).
    if (i === 0 && !isAcronym && STOPWORDS.has(clean.toLowerCase())) return;
    out.push(clean.toLowerCase());
  });
  return Array.from(new Set(out));
}

export function extractTokens(text: string): string[] {
  const toks = text
    .toLowerCase()
    .split(/[^a-z0-9]+/)
    .filter(Boolean)
    .filter((t) => t.length >= 2)
    .filter((t) => !STOPWORDS.has(t))
    .filter((t) => !/^\d[\d,.]*$/.test(t)); // numbers scored separately
  return Array.from(new Set(toks));
}

function buildRecord(
  id: string,
  kind: IndexKind,
  text: string,
  at: number,
  extra: Partial<IndexRecord> = {},
): IndexRecord {
  return {
    id,
    kind,
    text,
    at,
    numbers: Array.from(new Set(extractNumbers(text))),
    properNouns: extractProperNouns(text),
    tokens: extractTokens(text),
    ...extra,
  };
}

export class SessionIndex {
  private records: IndexRecord[] = [];
  // Document frequency per token, for IDF weighting.
  private df = new Map<string, number>();

  get size(): number {
    return this.records.length;
  }

  all(): readonly IndexRecord[] {
    return this.records;
  }

  addTurn(id: string, text: string, at: number): void {
    if (!text.trim()) return;
    this.insert(buildRecord(id, "turn", text, at));
  }

  /**
   * Index a COMPLETED verdict. In-flight / withdrawn verdicts are ignored — indexing
   * an in-progress verdict would let a claim retrieve its own not-yet-finished check
   * and reason in a circle.
   */
  addVerdict(card: VerdictCard): void {
    if (card.withdrawn) return;
    const disputed = card.verdict === "false" || card.verdict === "misleading";
    this.insert(
      buildRecord(card.id, "verdict", `${card.claim} ${card.rationale}`, card.createdAt, {
        verdict: card.verdict,
        disputed,
      }),
    );
  }

  private insert(rec: IndexRecord): void {
    this.records.push(rec);
    for (const tok of rec.tokens) {
      this.df.set(tok, (this.df.get(tok) ?? 0) + 1);
    }
  }

  private idf(token: string): number {
    const n = this.records.length || 1;
    const df = this.df.get(token) ?? 0;
    // Smoothed IDF normalized to ~0..1 so a common token contributes near zero and a
    // rare one near the full W_TOKEN, keeping token overlap subordinate to numbers.
    const raw = Math.log((n + 1) / (df + 0.5));
    const norm = raw / Math.log(n + 1);
    return Math.max(0, Math.min(1, norm));
  }

  /** Retrieve the records most relevant to `claimText`. */
  retrieve(claimText: string, now: number, limit = 5): ScoredRecord[] {
    const qNumbers = new Set(extractNumbers(claimText));
    const qProper = new Set(extractProperNouns(claimText));
    const qTokens = new Set(extractTokens(claimText));
    const referential = isReferential(claimText);

    const scored: ScoredRecord[] = [];
    for (const rec of this.records) {
      let score = 0;

      for (const num of rec.numbers) if (qNumbers.has(num)) score += W_NUMBER;
      for (const p of rec.properNouns) if (qProper.has(p)) score += W_PROPER;
      for (const t of rec.tokens) if (qTokens.has(t)) score += W_TOKEN * this.idf(t);

      // A referential claim needs the earlier figure — surface records that have one.
      if (referential && rec.numbers.length > 0) score += W_REFERENTIAL_FIGURE;

      if (score <= 0) continue;

      // Recency: a small, slowly-decaying tie-breaker. Deliberately capped so a
      // strong numeric match from minutes ago still outranks fresh chatter.
      const ageMin = Math.max(0, (now - rec.at) / 60_000);
      score += W_RECENCY_MAX / (1 + ageMin / 5);

      scored.push({ record: rec, score });
    }

    scored.sort((a, b) => b.score - a.score || b.record.at - a.record.at);
    return scored.slice(0, limit);
  }

  reset(): void {
    this.records = [];
    this.df.clear();
  }
}
