// Number normalization, shared by claim batching and session retrieval.
//
// The transcriber writes what people SAY, which is often words: "forty", "the vast
// majority", "two point three percent". Digit-only matching misses all of it — and a
// number is the single unambiguous link between two statements in a debate, so
// missing "forty" is not a small bug. We therefore normalize both digits and
// spelled-out numerals to a canonical numeric string, and match on that.

const ONES: Record<string, number> = {
  zero: 0, one: 1, two: 2, three: 3, four: 4, five: 5, six: 6, seven: 7,
  eight: 8, nine: 9, ten: 10, eleven: 11, twelve: 12, thirteen: 13,
  fourteen: 14, fifteen: 15, sixteen: 16, seventeen: 17, eighteen: 18,
  nineteen: 19,
};

const TENS: Record<string, number> = {
  twenty: 20, thirty: 30, forty: 40, fifty: 50, sixty: 60, seventy: 70,
  eighty: 80, ninety: 90,
};

const SCALES: Record<string, number> = {
  hundred: 100, thousand: 1_000, million: 1_000_000, billion: 1_000_000_000,
  trillion: 1_000_000_000_000,
};

// Words that carry a checkable quantity even without a figure. Retrieval boosts a
// statement pointing at a number; these are the wordy way of pointing at one.
export const QUANTIFIER_WORDS = new Set([
  "most", "majority", "minority", "few", "many", "several", "half", "third",
  "quarter", "double", "triple", "twice", "dozen", "none", "all", "every",
]);

/** Canonicalize a single numeric token: "4.2%", "4.2 percent", "4.2" → "4.2". */
export function canonicalizeNumber(raw: string): string | null {
  const cleaned = raw.replace(/[,%$£€]/g, "").replace(/\s+percent\b/i, "").trim();
  if (!/^-?\d+(\.\d+)?$/.test(cleaned)) return null;
  // Strip an insignificant trailing ".0" so "4" and "4.0" collide.
  const n = Number(cleaned);
  if (!Number.isFinite(n)) return null;
  return String(n);
}

/**
 * Extract every number from a piece of text as canonical numeric strings —
 * covering digit forms and spelled-out numerals (including compound "forty two"
 * and scaled "three million").
 */
export function extractNumbers(text: string): string[] {
  const out: string[] = [];
  const lower = text.toLowerCase();

  // Digit forms first: 4.2%, 4,200, $3, 2019.
  const digitRe = /-?\$?\d[\d,]*(\.\d+)?\s*(?:percent|%)?/gi;
  for (const m of lower.matchAll(digitRe)) {
    const c = canonicalizeNumber(m[0]);
    if (c !== null) out.push(c);
  }

  // Spelled-out numerals: scan word runs and fold them into values.
  for (const value of spelledNumbers(lower)) {
    out.push(String(value));
  }

  return out;
}

/** Parse runs of spelled-out number words into their integer values. */
function spelledNumbers(lower: string): number[] {
  const tokens = lower.split(/[^a-z]+/).filter(Boolean);
  const results: number[] = [];

  let current = 0; // accumulator for the number being built
  let group = 0; // sub-total below the current scale word
  let active = false; // are we mid-number?

  const flush = () => {
    if (active) results.push(current + group);
    current = 0;
    group = 0;
    active = false;
  };

  for (const tok of tokens) {
    if (tok in ONES) {
      group += ONES[tok];
      active = true;
    } else if (tok in TENS) {
      group += TENS[tok];
      active = true;
    } else if (tok === "hundred") {
      group = (group || 1) * 100;
      active = true;
    } else if (tok in SCALES) {
      const scale = SCALES[tok];
      current += (group || 1) * scale;
      group = 0;
      active = true;
    } else if (tok === "and" && active) {
      // "two hundred and five" — keep the run going.
      continue;
    } else {
      flush();
    }
  }
  flush();

  return results;
}

/** True if the text contains any figure — digit or spelled-out. */
export function hasNumber(text: string): boolean {
  return extractNumbers(text).length > 0;
}
