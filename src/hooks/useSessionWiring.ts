import { useEffect } from "react";
import { onTranscript, inTauri } from "../lib/ipc";
import { useSession } from "../store/session";

// Subscribe to transcript events from Rust and drive the idle-flush poll. Mounted once
// (in the dashboard window) so the pipeline runs even when the HUD is the only thing on
// screen. The idle poll is what makes the two claim-buffer timeouts fire.
export function useSessionWiring() {
  const ingestPartial = useSession((s) => s.ingestPartial);
  const ingestFinal = useSession((s) => s.ingestFinal);
  const pollIdle = useSession((s) => s.pollIdle);

  useEffect(() => {
    if (!inTauri()) return;
    let unlisten: (() => void) | undefined;
    onTranscript((e) => {
      if (e.kind === "final") ingestFinal(e.text);
      else if (e.kind === "partial") ingestPartial(e.text);
      // 'error' events surface elsewhere; the pipeline ignores them.
    }).then((u) => (unlisten = u));
    return () => unlisten?.();
  }, [ingestFinal, ingestPartial]);

  useEffect(() => {
    // Poll for idle-based flushes (a finished-but-unflushed claim, or a stalled batch).
    const id = window.setInterval(() => pollIdle(), 500);
    return () => window.clearInterval(id);
  }, [pollIdle]);
}
