// Verdict → colour/icon/label. false and misleading are BOTH red on purpose: amber for
// "misleading" reads as "minor", which inverts the point (a misleading claim is a true
// statement arranged to deceive — the harder kind to catch). Distinct ICONS keep them
// tellable apart despite the shared colour.

import type { Verdict } from "./types";

export interface VerdictStyle {
  label: string;
  color: string;
  icon: string; // distinct glyph per verdict
  hudAlarm: boolean; // published to the HUD?
}

export const VERDICT_STYLE: Record<Verdict, VerdictStyle> = {
  false: { label: "False", color: "#ef4444", icon: "✕", hudAlarm: true },
  misleading: { label: "Misleading", color: "#ef4444", icon: "▲", hudAlarm: true },
  verified: { label: "Verified", color: "#22c55e", icon: "✓", hudAlarm: false },
  context_needed: { label: "Context needed", color: "#eab308", icon: "?", hudAlarm: false },
  unverifiable: { label: "Unverifiable", color: "#94a3b8", icon: "–", hudAlarm: false },
};

export function verdictStyle(v: Verdict): VerdictStyle {
  return VERDICT_STYLE[v];
}
