import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { useSession } from "../store/session";
import { ipc, inTauri } from "../lib/ipc";
import { VerdictCardView } from "../components/VerdictCardView";

// The HUD. At rest it's a small logo; when there are alerts it becomes a pill with a
// count; clicking unfolds the transcript with the verdict cards stacked below.
//
// Only false + misleading cards reach here (the store's `alerts` derivation) — these are
// the alarms. verified / context_needed / unverifiable stay in the dashboard.
export function Overlay() {
  const alerts = useSession((s) => s.alerts);
  const [expanded, setExpanded] = useState(false);
  const paintRef = useRef<HTMLDivElement>(null);

  // Report the painted region to Rust so it can make the transparent area click-through.
  // Rust polls the cursor against this rect (an ignored window fires no DOM mouse events,
  // so the webview can't do it). We report screen coordinates using the window origin.
  useLayoutEffect(() => {
    if (!inTauri()) return;
    const el = paintRef.current;
    if (!el) return;
    const report = () => {
      const r = el.getBoundingClientRect();
      ipc
        .setOverlayHotZone({
          x: window.screenX + r.left,
          y: window.screenY + r.top,
          width: r.width,
          height: r.height,
        })
        .catch(() => {});
    };
    report();
    const ro = new ResizeObserver(report);
    ro.observe(el);
    window.addEventListener("resize", report);
    return () => {
      ro.disconnect();
      window.removeEventListener("resize", report);
    };
  }, [expanded, alerts.length]);

  useEffect(() => {
    // Auto-expand momentarily when a new alert lands so the user notices.
    if (alerts.length > 0) setExpanded(true);
  }, [alerts.length]);

  const orderedAlerts = [...alerts].sort((a, b) => b.createdAt - a.createdAt);

  return (
    <div className="h-screen w-screen flex justify-end items-start p-3">
      <div ref={paintRef} className="hud-paint flex flex-col items-end" style={{ maxHeight: "94vh" }}>
        <button
          className="flex items-center gap-2 px-3 py-2 rounded-full shadow-lg select-none"
          style={{ background: alerts.length ? "#ef4444" : "rgba(18,18,20,0.9)" }}
          onClick={() => setExpanded((v) => !v)}
        >
          <span className="text-sm font-bold text-white">V</span>
          {alerts.length > 0 && (
            <span className="text-xs font-semibold text-white">
              {alerts.length} alert{alerts.length > 1 ? "s" : ""}
            </span>
          )}
        </button>

        {expanded && alerts.length > 0 && (
          <div
            className="mt-2 w-[360px] overflow-y-auto rounded-xl bg-panel p-2"
            style={{ maxHeight: "80vh" }}
          >
            {orderedAlerts.map((c) => (
              <VerdictCardView key={c.id} card={c} compact />
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
