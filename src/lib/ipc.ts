// Typed wrappers over the Tauri command surface (spec §4). Keeping every invoke() in
// one place means the webview never constructs a raw provider request — all model and
// search calls happen in Rust, and no API key is ever used in page context.

import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type { VerdictCard, Verdict, Provenance } from "./types";

export interface AudioDevice {
  id: string;
  name: string;
  is_default: boolean;
  supports_loopback: boolean;
}

export interface MonitorInfo {
  id: number;
  name: string;
  width: number;
  height: number;
  is_primary: boolean;
}

export interface DockChrome {
  notch: { has_notch: boolean; menu_bar_height: number; notch_width: number; screen_width: number };
  island: { width: number; height: number; center_x: number; straddles_notch: boolean };
  reveal_band: { x: number; width: number; height: number };
}

export interface Rect {
  x: number;
  y: number;
  width: number;
  height: number;
}

// The raw verdict result the Rust engine returns (snake_case provenance).
interface RawVerdict {
  claim: string;
  verdict: Verdict;
  rationale: string;
  correction: string | null;
  provenance: {
    web_search_used: boolean;
    provider: "openai" | "anthropic";
    model: string;
    latency_ms: number;
    sources: { title: string; url: string; publishedDate?: string | null; text?: string }[];
    prefetched: boolean;
  };
}

export interface VerifyRequest {
  claim: string;
  brief?: string;
  openai_key?: string;
  anthropic_key?: string;
  exa_key?: string;
  web_search_enabled?: boolean;
  provider?: string;
  openai_model?: string;
  anthropic_model?: string;
}

export const ipc = {
  toggleOverlay: () => invoke<boolean>("toggle_overlay"),
  setOverlayClickThrough: (enabled: boolean) =>
    invoke<void>("set_overlay_click_through", { enabled }),
  setOverlayHotZone: (rect: Rect | null) => invoke<void>("set_overlay_hot_zone", { rect }),
  focusDashboard: () => invoke<void>("focus_dashboard"),
  captureStatus: () =>
    invoke<{ capturing: boolean; transcribing: boolean; dropped_frames: number; published_frames: number }>(
      "capture_status",
    ),
  setDockPinned: (pinned: boolean) => invoke<void>("set_dock_pinned", { pinned }),
  dockChrome: () => invoke<DockChrome>("dock_chrome"),
  getActiveApp: () => invoke<Record<string, unknown>>("get_active_app"),

  listAudioDevices: () => invoke<AudioDevice[]>("list_audio_devices"),
  startAudioCapture: (deviceId: string | null) =>
    invoke<void>("start_audio_capture", { deviceId }),
  stopAudioCapture: () => invoke<void>("stop_audio_capture"),

  listMonitors: () => invoke<MonitorInfo[]>("list_monitors"),
  startVisionCapture: (monitorIndex: number, intervalMs: number) =>
    invoke<void>("start_vision_capture", { monitorIndex, intervalMs }),
  stopVisionCapture: () => invoke<void>("stop_vision_capture"),

  extractDocumentText: (path: string) => invoke<string>("extract_document_text", { path }),

  startTranscription: (apiKey: string, model?: string) =>
    invoke<void>("start_transcription", { apiKey, model }),
  stopTranscription: () => invoke<void>("stop_transcription"),

  verifyClaim: async (req: VerifyRequest): Promise<VerdictCard> => {
    const raw = await invoke<RawVerdict>("verify_claim", { req });
    return rawToCard(raw);
  },
};

let cardSeq = 0;

function rawToCard(raw: RawVerdict): VerdictCard {
  const provenance: Provenance = {
    webSearchUsed: raw.provenance.web_search_used,
    provider: raw.provenance.provider,
    model: raw.provenance.model,
    latencyMs: raw.provenance.latency_ms,
    prefetched: raw.provenance.prefetched,
    sources: raw.provenance.sources.map((s) => ({
      title: s.title,
      url: s.url,
      publishedDate: s.publishedDate ?? null,
      text: s.text,
    })),
  };
  return {
    id: `card-${Date.now()}-${cardSeq++}`,
    claim: raw.claim,
    verdict: raw.verdict,
    rationale: raw.rationale,
    correction: raw.correction,
    provenance,
    createdAt: Date.now(),
  };
}

// --- Event streams from Rust ---

export interface TranscriptEvent {
  kind: "partial" | "final" | "error";
  text: string;
}

export const onTranscript = (cb: (e: TranscriptEvent) => void): Promise<UnlistenFn> =>
  listen<TranscriptEvent>("transcript", (e) => cb(e.payload));

export const onVisionFrame = (cb: (frame: unknown) => void): Promise<UnlistenFn> =>
  listen("vision-frame", (e) => cb(e.payload));

export const onDockState = (cb: (expanded: boolean) => void): Promise<UnlistenFn> =>
  listen<boolean>("dock-state", (e) => cb(e.payload));

/** True in a real Tauri window; false in a plain browser/test (so UI can no-op). */
export const inTauri = (): boolean =>
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in window;
