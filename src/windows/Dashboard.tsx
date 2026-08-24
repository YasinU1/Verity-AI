import { useEffect, useMemo, useState } from "react";
import { useSettings } from "../store/settings";
import { useSession } from "../store/session";
import { useSessionWiring } from "../hooks/useSessionWiring";
import { ipc, inTauri, type AudioDevice } from "../lib/ipc";
import { VerdictCardView } from "../components/VerdictCardView";

export function Dashboard() {
  useSessionWiring();
  const session = useSession();
  const [devices, setDevices] = useState<AudioDevice[]>([]);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!inTauri()) return;
    ipc.listAudioDevices().then(setDevices).catch(() => {});
  }, []);

  const s = useSettings();

  async function startSession() {
    setError(null);
    const keys = s.effectiveKeys();
    if (!keys.openai) {
      setError("An OpenAI API key is required for live transcription.");
      return;
    }
    try {
      if (inTauri()) {
        await ipc.startAudioCapture(s.audioDeviceId);
        await ipc.startTranscription(keys.openai, s.transcriptionModel);
        if (s.visionEnabled) await ipc.startVisionCapture(0, s.visionIntervalMs);
      }
      session.start();
    } catch (e) {
      setError(String(e));
    }
  }

  async function stopSession() {
    if (inTauri()) {
      await ipc.stopTranscription().catch(() => {});
      await ipc.stopAudioCapture().catch(() => {});
      await ipc.stopVisionCapture().catch(() => {});
    }
    session.stop();
  }

  return (
    <div className="h-screen w-screen p-3 flex flex-col gap-3 text-sm">
      <TopBar live={session.live} onStart={startSession} onStop={stopSession} inFlight={session.inFlightCount} />
      {error && <div className="text-xs text-red-400 px-2">{error}</div>}
      <div className="flex-1 grid grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)_minmax(0,1fr)] gap-3 min-h-0">
        <ContextColumn />
        <FeedColumn />
        <SettingsColumn devices={devices} />
      </div>
    </div>
  );
}

function TopBar({
  live,
  onStart,
  onStop,
  inFlight,
}: {
  live: boolean;
  onStart: () => void;
  onStop: () => void;
  inFlight: number;
}) {
  return (
    <div className="drag-region flex items-center justify-between px-3 py-2 rounded-xl bg-panel">
      <div className="flex items-center gap-2">
        <span className="inline-block w-2.5 h-2.5 rounded-full" style={{ background: live ? "#22c55e" : "#666" }} />
        <span className="font-semibold">Verity</span>
        <span className="text-xs text-neutral-400">{live ? "listening" : "idle"}</span>
        {inFlight > 0 && <span className="text-[10px] text-neutral-500">· {inFlight} checking</span>}
      </div>
      <div className="no-drag flex items-center gap-2">
        {live ? (
          <button className="px-3 py-1 rounded-lg bg-red-500/80 hover:bg-red-500 text-white text-xs" onClick={onStop}>
            Stop
          </button>
        ) : (
          <button className="px-3 py-1 rounded-lg bg-emerald-500/80 hover:bg-emerald-500 text-white text-xs" onClick={onStart}>
            Start session
          </button>
        )}
      </div>
    </div>
  );
}

function ContextColumn() {
  const [docs, setDocs] = useState<{ name: string; chars: number }[]>([]);
  const [busy, setBusy] = useState(false);

  async function onDrop(e: React.DragEvent) {
    e.preventDefault();
    if (!inTauri()) return;
    setBusy(true);
    // Tauri exposes dropped paths via the file-drop event in production; here we accept
    // a path typed for dev. Kept minimal — extraction happens in Rust (documents.rs).
    const files = Array.from(e.dataTransfer.files);
    for (const f of files) {
      try {
        const text = await ipc.extractDocumentText((f as unknown as { path?: string }).path ?? f.name);
        setDocs((d) => [...d, { name: f.name, chars: text.length }]);
      } catch {
        /* surfaced inline */
      }
    }
    setBusy(false);
  }

  return (
    <div className="flex flex-col rounded-xl bg-panel p-3 min-h-0">
      <h2 className="text-xs uppercase tracking-wide text-neutral-400 mb-2">Context</h2>
      <div
        className="flex-1 border border-dashed border-neutral-600 rounded-lg flex items-center justify-center text-center text-xs text-neutral-400 p-4"
        onDragOver={(e) => e.preventDefault()}
        onDrop={onDrop}
      >
        {busy ? "Extracting…" : "Drop a PDF, brief, or transcript here to give the checker background."}
      </div>
      <ul className="mt-2 space-y-1">
        {docs.map((d, i) => (
          <li key={i} className="text-xs text-neutral-300 flex justify-between">
            <span className="truncate">{d.name}</span>
            <span className="text-neutral-500">{d.chars} chars</span>
          </li>
        ))}
      </ul>
    </div>
  );
}

function FeedColumn() {
  const transcript = useSession((s) => s.transcript);
  const partial = useSession((s) => s.partial);
  const cards = useSession((s) => s.cards);

  const orderedCards = useMemo(() => [...cards].sort((a, b) => b.createdAt - a.createdAt), [cards]);

  return (
    <div className="flex flex-col rounded-xl bg-panel p-3 min-h-0">
      <h2 className="text-xs uppercase tracking-wide text-neutral-400 mb-2">Live feed</h2>
      <div className="flex-1 overflow-y-auto min-h-0 pr-1">
        {orderedCards.length === 0 && transcript.length === 0 && (
          <p className="text-xs text-neutral-500">Verdicts appear here as claims are checked. Only false and misleading claims are pushed to the HUD.</p>
        )}
        {orderedCards.map((c) => (
          <VerdictCardView key={c.id} card={c} />
        ))}
        <div className="mt-3 border-t border-neutral-700 pt-2 space-y-1">
          {partial && <p className="text-xs text-neutral-500 italic">{partial}…</p>}
          {[...transcript].reverse().slice(0, 20).map((u) => (
            <p key={u.id} className="text-xs text-neutral-400 leading-snug">
              {u.text}
            </p>
          ))}
        </div>
      </div>
    </div>
  );
}

function SettingsColumn({ devices }: { devices: AudioDevice[] }) {
  const s = useSettings();
  return (
    <div className="flex flex-col rounded-xl bg-panel p-3 min-h-0 overflow-y-auto">
      <h2 className="text-xs uppercase tracking-wide text-neutral-400 mb-2">Settings</h2>
      <div className="space-y-3 text-xs">
        <Field label="Audio source">
          <select
            className="input"
            value={s.audioSource}
            onChange={(e) => s.set("audioSource", e.target.value as "system" | "microphone")}
          >
            <option value="system">System audio (loopback)</option>
            <option value="microphone">Microphone</option>
          </select>
        </Field>
        <Field label="Input device">
          <select
            className="input"
            value={s.audioDeviceId ?? ""}
            onChange={(e) => s.set("audioDeviceId", e.target.value || null)}
          >
            <option value="">Default</option>
            {devices.map((d) => (
              <option key={d.id} value={d.id}>
                {d.name}
                {d.supports_loopback ? " (loopback)" : ""}
              </option>
            ))}
          </select>
        </Field>
        <Field label={`Verify every ${s.verifyEveryWords} words`}>
          <input
            type="range"
            min={20}
            max={200}
            value={s.verifyEveryWords}
            onChange={(e) => s.set("verifyEveryWords", Number(e.target.value))}
            className="w-full"
          />
        </Field>
        <Toggle label="Web search (Exa)" checked={s.webSearchEnabled} onChange={(v) => s.set("webSearchEnabled", v)} />
        <Toggle label="Overlay HUD" checked={s.overlayEnabled} onChange={(v) => s.set("overlayEnabled", v)} />
        <Toggle label="Vision (screenshots)" checked={s.visionEnabled} onChange={(v) => s.set("visionEnabled", v)} />
        <Toggle label="Auto-start on YouTube video" checked={s.autoStartEnabled} onChange={(v) => s.set("autoStartEnabled", v)} />

        <Field label="Provider">
          <select
            className="input"
            value={s.verificationProvider}
            onChange={(e) => s.set("verificationProvider", e.target.value as "auto" | "openai" | "anthropic")}
          >
            <option value="auto">Auto (by key)</option>
            <option value="openai">OpenAI</option>
            <option value="anthropic">Anthropic</option>
          </select>
        </Field>

        <div className="pt-2 border-t border-neutral-700">
          <p className="text-[10px] text-neutral-500 mb-2">
            Keys stay local and are used only by the Rust backend — they never enter page network calls.
          </p>
          <KeyField label="OpenAI key" value={s.openAiApiKey} onChange={(v) => s.set("openAiApiKey", v)} />
          <KeyField label="Anthropic key" value={s.anthropicApiKey} onChange={(v) => s.set("anthropicApiKey", v)} />
          <KeyField label="Exa key" value={s.exaApiKey} onChange={(v) => s.set("exaApiKey", v)} />
        </div>
      </div>
    </div>
  );
}

function Field({ label, children }: { label: string; children: React.ReactNode }) {
  return (
    <label className="block">
      <span className="text-neutral-400">{label}</span>
      <div className="mt-1">{children}</div>
    </label>
  );
}

function Toggle({ label, checked, onChange }: { label: string; checked: boolean; onChange: (v: boolean) => void }) {
  return (
    <label className="flex items-center justify-between cursor-pointer">
      <span className="text-neutral-300">{label}</span>
      <input type="checkbox" checked={checked} onChange={(e) => onChange(e.target.checked)} />
    </label>
  );
}

function KeyField({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  return (
    <label className="block mb-2">
      <span className="text-neutral-400 text-[10px]">{label}</span>
      <input
        type="password"
        className="input"
        placeholder="sk-…"
        value={value}
        onChange={(e) => onChange(e.target.value)}
      />
    </label>
  );
}
