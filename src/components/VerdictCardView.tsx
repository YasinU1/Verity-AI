import type { VerdictCard } from "../lib/types";
import { verdictStyle } from "../lib/verdictStyle";

// A verdict card. Provenance is shown on EVERY card (spec §11): whether web search was
// used, provider + model, latency, and the dated source list — so a verdict answered
// from the model's memory is visibly distinguishable from one backed by sources.
export function VerdictCardView({ card, compact = false }: { card: VerdictCard; compact?: boolean }) {
  const style = verdictStyle(card.verdict);
  const p = card.provenance;
  return (
    <div
      className="rounded-xl px-3 py-2 mb-2 border"
      style={{
        background: "rgba(28,28,32,0.9)",
        borderColor: style.color + "66",
        opacity: card.withdrawn ? 0.5 : 1,
      }}
    >
      <div className="flex items-start gap-2">
        <span
          className="shrink-0 mt-0.5 inline-flex items-center justify-center w-5 h-5 rounded-full text-xs font-bold"
          style={{ background: style.color, color: "#0b0b0d" }}
          aria-label={style.label}
        >
          {style.icon}
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex items-center gap-2">
            <span className="text-xs font-semibold" style={{ color: style.color }}>
              {style.label}
            </span>
            {card.withdrawn && (
              <span className="text-[10px] text-neutral-400">(speaker corrected)</span>
            )}
          </div>
          <div className="text-sm text-neutral-100 leading-snug mt-0.5">{card.claim}</div>
          {!compact && card.rationale && (
            <div className="text-xs text-neutral-300 mt-1 leading-snug">{card.rationale}</div>
          )}
          {card.correction && (
            <div className="text-xs mt-1" style={{ color: style.color }}>
              Actual: {card.correction}
            </div>
          )}

          <div className="flex items-center gap-2 mt-1.5 text-[10px] text-neutral-400">
            <span
              className="px-1.5 py-0.5 rounded"
              style={{ background: p.webSearchUsed ? "#1e3a5f" : "#3a2f1e" }}
              title={p.webSearchUsed ? "Backed by web search" : "Answered from model knowledge"}
            >
              {p.webSearchUsed ? "web" : "memory"}
            </span>
            <span>
              {p.provider}/{p.model}
            </span>
            <span>{p.latencyMs}ms</span>
            {p.prefetched && <span title="Evidence prefetched">prefetch</span>}
          </div>

          {!compact && p.sources.length > 0 && (
            <ul className="mt-1.5 space-y-0.5">
              {p.sources.slice(0, 5).map((s, i) => (
                <li key={i} className="text-[10px] text-neutral-400 truncate">
                  <a href={s.url} target="_blank" rel="noreferrer" className="hover:text-neutral-200">
                    [{i + 1}] {s.title || s.url}
                    {s.publishedDate ? ` · ${s.publishedDate}` : ""}
                  </a>
                </li>
              ))}
            </ul>
          )}
        </div>
      </div>
    </div>
  );
}
