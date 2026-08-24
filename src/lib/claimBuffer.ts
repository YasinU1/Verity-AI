// Claim batching (spec §6).
//
// Utterances arrive from the transcriber chunked on BREATHING, not on meaning. This
// layer reassembles them into checkable claims and decides when a claim is worth an
// API call. The three subtleties that make it work — and each of which shipped as a
// real bug when missing — are:
//
//   1. Sentence completeness. A marker firing ("forty") does NOT mean flush; the
//      transcriber finalizes on a pause, so "they cut it by about forty" looks
//      checkable but is half a claim. We ARM on a marker and wait for an utterance
//      that reads finished, bounded by a hard word cap so it can't stall forever.
//      Bias toward "complete": a stalled buffer is worse than a slightly clipped claim.
//   2. Two idle timeouts, not one. A held-but-incomplete claim waits only
//      INCOMPLETE_HOLD_MS; a batch with nothing checkable yet waits IDLE_FLUSH_MS.
//      A single flat 12s makes a finished-but-unflushed claim look like a hung app.
//   3. Self-correction. "It was 3.2 percent." flushes and verifies; a breath later
//      "actually, 2.3." must WITHDRAW the in-flight check, not run a second one that
//      lands a red card against a figure the speaker already retracted.

import type { Claim } from "./types";
import { extractNumbers, hasNumber, QUANTIFIER_WORDS } from "./numbers";

// --- Tunable constants (each with the reason it holds the value it does) ---

/** Held claim only missing the end of its sentence — flush after a short silence. */
export const INCOMPLETE_HOLD_MS = 1_200;
/** Nothing checkable said yet — no cost to waiting, so wait long before flushing. */
export const IDLE_FLUSH_MS = 12_000;
/** A sentence that hasn't landed in this many words is clipped and flushed anyway,
 *  so an armed batch waiting for completion can never stall the pipeline. */
export const ARMED_HARD_WORD_CAP = 45;
/** Above this the batch drops from the FRONT — the tail is what the claim depends on. */
export const MAX_BATCH_WORDS = 200;

export const DEFAULT_VERIFY_EVERY_WORDS = 50;
export const MIN_VERIFY_EVERY_WORDS = 20;
export const MAX_VERIFY_EVERY_WORDS = 200;

export type FlushReason = "marker" | "words" | "idle" | "session_end";

export type ClaimEvent =
  | { type: "flush"; claim: Claim; reason: FlushReason }
  // A repair supersedes in-flight verifications; the caller must withdraw them
  // BEFORE running the corrected claim. Landed verdicts are NOT deleted (they record
  // something genuinely said) — that is the caller's rule, enforced elsewhere.
  | { type: "repair"; claim: Claim; withdrawInFlight: true };

export interface ClaimBufferOptions {
  verifyEveryWords?: number;
  /** Injectable clock so tests are deterministic; defaults to Date.now. */
  now?: () => number;
}

interface Entry {
  id: string;
  text: string;
  at: number;
}

// --- Lexical predicates ---

const FILLER_ONLY = new Set([
  "yeah", "yep", "yes", "yup", "nope", "no", "ok", "okay", "sure", "right",
  "exactly", "totally", "mhm", "mmhm", "uhhuh", "cool", "nice", "wow", "hmm",
  "well", "so", "anyway", "true", "correct", "indeed", "absolutely",
]);

const HESITATION = new Set(["um", "uh", "uhh", "er", "erm", "mmm", "hmm"]);

// Trailing tokens that mean the sentence is unfinished — flushing here clips it.
const DANGLING = new Set([
  "and", "or", "but", "because", "than", "about", "of", "to", "for", "with",
  "as", "at", "in", "on", "from", "by", "so", "if", "when", "while", "since",
  "though", "although", "nor", "yet", "the", "a", "an", "that", "which", "into",
  "onto", "per", "versus", "vs", "over", "under", "up", "down", "around",
]);

// Repair openers, anchored to the START of an utterance. Mid-sentence "actually" is
// an intensifier, not a retraction — so we only match at position zero.
const REPAIR_OPENERS = [
  "actually", "sorry", "i mean", "no wait", "wait no", "i misspoke",
  "correction", "let me correct", "scratch that", "i meant",
];

const OPINION_OPENERS =
  /^\s*(i think|i believe|i feel|i guess|i reckon|we should|you should|they should|in my opinion|imo|i'd say|i would say|my view)\b/i;

const NUMBER_WORD =
  /\b(zero|one|two|three|four|five|six|seven|eight|nine|ten|eleven|twelve|thirteen|fourteen|fifteen|sixteen|seventeen|eighteen|nineteen|twenty|thirty|forty|fifty|sixty|seventy|eighty|ninety|hundred|thousand|million|billion|trillion)\b/i;

// Assertion markers — any one of these makes a completed batch worth checking.
const MARKERS: RegExp[] = [
  // quantifiers / vagueness that still asserts a magnitude
  /\b(most|majority|minority|vast majority|nearly all|almost all|few|many|half|double|triple)\b/i,
  // absolutes
  /\b(never|always|nobody|no one|everyone|everybody|none|all of|every single)\b/i,
  // superlatives
  /\b(\w+est|best|worst|largest|smallest|highest|lowest|greatest|most|least)\b/i,
  // comparisons
  /\b(more than|less than|fewer than|greater than|higher than|lower than|than)\b/i,
  // directional change
  /\b(rose|fell|increased|decreased|grew|shrank|dropped|climbed|plunged|surged|declined|doubled|halved|up from|down from)\b/i,
  // attribution
  /\b(said|claimed|told|stated|announced|argued|insisted|according to)\b/i,
  // factual framing
  /\b(studies show|research shows|data shows|reports? (say|show|found)|the figures? (show|say)|statistics show)\b/i,
];

function tokenize(text: string): string[] {
  return text.toLowerCase().split(/[^a-z0-9.%$-]+/).filter(Boolean);
}

function wordCount(text: string): number {
  return tokenize(text).length;
}

/** Strip disfluencies ("um", "uh", "you know") and tidy the punctuation they leave. */
export function stripDisfluencies(text: string): string {
  return text
    .replace(/\b(um+|uh+|er+|erm|mmm|hmm)\b/gi, "")
    .replace(/\byou know\b/gi, "")
    .replace(/\bi mean\b/gi, "")
    .replace(/\s*,\s*,+/g, ", ") // collapse doubled commas the removals leave
    .replace(/\s+,/g, ",")
    .replace(/,\s*(?=[.!?])/g, "")
    .replace(/\s{2,}/g, " ")
    .replace(/^[\s,]+/, "")
    .replace(/\s+([.!?])/g, "$1")
    .trim();
}

export function isQuestion(text: string): boolean {
  return /\?\s*$/.test(text.trim());
}

export function isOpinionOpener(text: string): boolean {
  return OPINION_OPENERS.test(text);
}

/** Whole utterance is nothing but filler ("yeah", "exactly", "right"). */
export function isFillerOnly(text: string): boolean {
  const toks = tokenize(text);
  if (toks.length === 0) return true;
  return toks.every((t) => FILLER_ONLY.has(t) || HESITATION.has(t));
}

/** Does an assertion marker fire anywhere in the text? Spelled numerals count. */
export function hasAssertionMarker(text: string): boolean {
  if (hasNumber(text)) return true; // covers digits AND spelled-out numerals
  if (NUMBER_WORD.test(text)) return true;
  for (const w of QUANTIFIER_WORDS) {
    if (new RegExp(`\\b${w}\\b`, "i").test(text)) return true;
  }
  return MARKERS.some((re) => re.test(text));
}

/** Does the utterance START with a repair opener (retraction), not contain one? */
export function startsWithRepair(text: string): boolean {
  const t = stripDisfluencies(text).toLowerCase().replace(/^[\s,.-]+/, "");
  return REPAIR_OPENERS.some(
    (op) => t === op || t.startsWith(op + " ") || t.startsWith(op + ","),
  );
}

/** Remove a leading repair opener so the corrected claim is just the corrected value. */
export function stripRepairOpener(text: string): string {
  let t = text.replace(/^[\s,.-]+/, "");
  for (const op of REPAIR_OPENERS) {
    const re = new RegExp(`^${op}[\\s,]+`, "i");
    if (re.test(t)) {
      t = t.replace(re, "");
      break;
    }
  }
  return t.trim();
}

/**
 * Does this text read as an UNFINISHED sentence? True means "hold, don't flush yet".
 * We bias toward complete (return false when unsure): a stalled buffer is worse than
 * a slightly clipped claim.
 */
export function endsIncomplete(text: string): boolean {
  const trimmed = text.trim();
  if (!trimmed) return true;
  // Terminal punctuation is a hard signal the sentence landed.
  if (/[.!?]["')]?\s*$/.test(trimmed)) return false;

  const toks = tokenize(trimmed);
  const last = toks[toks.length - 1];
  const lastTwo = toks.slice(-2).join(" ");
  if (!last) return true;

  if (HESITATION.has(last)) return true; // trailing "um"
  if (lastTwo === "you know" || last === "like") return true; // trailing hesitation phrase
  if (DANGLING.has(last)) return true; // dangling connective / article / preposition

  // A bare numeral with nothing after it ("about forty", "cut it by 40") is a claim
  // still waiting for its unit — hold for the rest.
  if (/^\d[\d,.]*$/.test(last)) return true;
  if (NUMBER_WORD.test(last) && QUANTIFIER_WORDS.has(last) === false) {
    // "forty" alone is bare; but "forty percent" ends on "percent" and is complete.
    if (extractNumbers(last).length > 0) return true;
  }

  return false;
}

/** Enough declarative content to be worth an API call (not a question/opinion/filler). */
export function hasDeclarativeSubstance(text: string): boolean {
  const t = text.trim();
  if (!t) return false;
  if (isFillerOnly(t)) return false;
  if (isQuestion(t)) return false;
  // Opinion openers are suppressed UNLESS the sentence also carries a hard figure —
  // "I think unemployment is at 8%" contains a genuinely checkable number. Cost of
  // this choice: a purely rhetorical "I think" with a stray number slips through.
  if (isOpinionOpener(t) && !hasNumber(t)) return false;
  return wordCount(t) >= 3 || hasNumber(t);
}

export class ClaimBuffer {
  private entries: Entry[] = [];
  private armed = false;
  private lastActivityAt = 0;
  private seq = 0;
  private readonly verifyEveryWords: number;
  private readonly now: () => number;

  constructor(opts: ClaimBufferOptions = {}) {
    this.now = opts.now ?? (() => Date.now());
    const w = opts.verifyEveryWords ?? DEFAULT_VERIFY_EVERY_WORDS;
    this.verifyEveryWords = Math.min(
      MAX_VERIFY_EVERY_WORDS,
      Math.max(MIN_VERIFY_EVERY_WORDS, Math.round(w)),
    );
  }

  private combined(): string {
    return this.entries.map((e) => e.text).join(" ").replace(/\s{2,}/g, " ").trim();
  }

  private buildClaim(isRepair = false): Claim {
    const text = stripDisfluencies(this.combined());
    const claim: Claim = {
      id: `claim-${this.now()}-${this.seq++}`,
      text,
      sourceUtteranceIds: this.entries.map((e) => e.id),
      createdAt: this.now(),
      isRepair,
    };
    return claim;
  }

  private flush(reason: FlushReason): ClaimEvent | null {
    if (this.entries.length === 0) return null;
    const claim = this.buildClaim(false);
    this.entries = [];
    this.armed = false;
    if (!claim.text) return null;
    return { type: "flush", claim, reason };
  }

  /** Feed a finalized utterance. Returns any claims/repairs it triggers. */
  push(utterance: { id: string; text: string; at?: number }): ClaimEvent[] {
    const at = utterance.at ?? this.now();
    this.lastActivityAt = at;
    const raw = utterance.text ?? "";

    // 1) Repair openers, anchored to the start, supersede in-flight work.
    if (startsWithRepair(raw)) {
      const events: ClaimEvent[] = [];
      // A repair carries the corrected value; strip the opener and emit the remainder,
      // marked so the caller withdraws prior in-flight verifications before running it.
      // A bare "no wait" with no corrected value has nothing to run, so we skip it.
      this.entries = [{ id: utterance.id, text: stripRepairOpener(raw), at }];
      const claim = this.buildClaim(true);
      this.entries = [];
      this.armed = false;
      if (claim.text) events.push({ type: "repair", claim, withdrawInFlight: true });
      return events;
    }

    // 2) Pure filler never joins a claim, but it IS speech — reset idle timing.
    if (isFillerOnly(raw)) return [];

    this.entries.push({ id: utterance.id, text: raw, at });
    this.enforceFrontDrop();

    const combined = this.combined();
    const events: ClaimEvent[] = [];

    // 3) Arm on a marker (only if the batch could ever be worth checking).
    if (!this.armed && hasAssertionMarker(combined) && !isQuestion(combined)) {
      this.armed = true;
    }

    if (this.armed) {
      // Flush only when the sentence reads finished, or the hard cap forces it.
      if (!endsIncomplete(combined)) {
        const e = this.flush("marker");
        if (e) events.push(e);
        return events;
      }
      if (wordCount(combined) >= ARMED_HARD_WORD_CAP) {
        const e = this.flush("marker"); // clipped, but a stalled buffer is worse
        if (e) events.push(e);
        return events;
      }
      // Held: waiting for the end of the sentence (see INCOMPLETE_HOLD_MS in poll).
      return events;
    }

    // 4) Not armed: a long run of plain declarative content still gets checked, but
    //    only if it carries declarative substance (else it's not worth the API call).
    if (wordCount(combined) >= this.verifyEveryWords && hasDeclarativeSubstance(combined)) {
      const e = this.flush("words");
      if (e) events.push(e);
    }
    return events;
  }

  /** Call periodically. Handles the two idle timeouts. */
  poll(now?: number): ClaimEvent[] {
    const t = now ?? this.now();
    if (this.entries.length === 0) return [];
    const idle = t - this.lastActivityAt;
    const events: ClaimEvent[] = [];

    if (this.armed) {
      // Held claim, only missing the sentence's end — short hold then flush (clipped).
      if (idle >= INCOMPLETE_HOLD_MS) {
        const e = this.flush("idle");
        if (e) events.push(e);
      }
      return events;
    }

    // Nothing armed: wait long, and only flush if there's something worth checking.
    if (idle >= IDLE_FLUSH_MS && hasDeclarativeSubstance(this.combined())) {
      const e = this.flush("idle");
      if (e) events.push(e);
    }
    return events;
  }

  /** Session end — flush whatever remains if it's worth checking. */
  end(): ClaimEvent[] {
    if (this.entries.length === 0) return [];
    if (!hasDeclarativeSubstance(this.combined())) {
      this.entries = [];
      this.armed = false;
      return [];
    }
    const e = this.flush("session_end");
    return e ? [e] : [];
  }

  reset(): void {
    this.entries = [];
    this.armed = false;
  }

  get isArmed(): boolean {
    return this.armed;
  }

  get pendingText(): string {
    return this.combined();
  }

  private enforceFrontDrop(): void {
    // Drop from the FRONT when a batch runs long — the tail is what the claim
    // depends on ("...which is why the 4.2% figure is wrong" needs the 4.2%, but if
    // it must go, keep the newest words that carry the actual assertion).
    while (wordCount(this.combined()) > MAX_BATCH_WORDS && this.entries.length > 1) {
      this.entries.shift();
    }
  }
}
