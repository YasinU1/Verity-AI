# Verity

A macOS desktop app that fact-checks speech in real time. It listens to whatever is being said — a debate, a podcast, a call, a YouTube video — transcribes it live, extracts the checkable claims, verifies them against the open web, and surfaces verdicts on a floating HUD that stays visible over full-screen apps.

Built with **Tauri 2** (Rust backend + React/TypeScript webview). macOS only.

---

## The one constraint that shapes everything

**A verdict that arrives after the speaker has moved on is worthless.** Latency here is a correctness constraint, not a performance nicety. Almost every design decision below is buying time on the path between someone speaking and a verdict appearing:

- Audio never crosses IPC — captured PCM is published to an in-process Rust bus and read directly by the transcription module (no base64 round trip per frame).
- Retrieval is **lexical, not embeddings** — free, no index to warm, and it keys on the literals (`4.2%`, `ONS`, `2019`) that actually link two statements in a debate.
- The debate brief is assembled **mechanically**, not summarized by an LLM — no extra round trip, and it can't hallucinate a "settled" fact.
- Verification **prefetches** the search for claims it already knows need one, so the model reaches a verdict on its first call instead of spending a round trip naming a query.

The second hard rule: **API keys never enter page context.** All model/search network calls happen in Rust; the webview never makes a provider request.

## Architecture

```
cpal (mic / system loopback, device rate)
  → resample to 24 kHz mono PCM               (audio.rs)
  → in-process AudioBus                       (bus.rs)      ← PCM never crosses into the webview
  → WebSocket to OpenAI Realtime              (transcription.rs, client-side turn detection)
  → transcript events → webview
  → ClaimBuffer (batching)                    (lib/claimBuffer.ts)
  → SessionIndex + debate brief               (lib/sessionIndex.ts, lib/debateBrief.ts)
  → invoke("verify_claim") → Rust             (verification.rs: Exa prefetch + LLM tool loop)
  → verdict → store → dashboard + HUD         (store/session.ts, windows/*)
```

Two windows share one frontend bundle, routed by URL hash: `#main` (the docked dashboard) and `#overlay` (the click-through HUD).

### Rust modules (`src-tauri/src/`)

| Module | Job |
|---|---|
| `audio.rs` | cpal capture, resample to 24 kHz mono, device + loopback enumeration |
| `bus.rs` | in-process PCM broadcast with a dropped-frame counter |
| `transcription.rs` | OpenAI Realtime WebSocket, client-side turn detection, backoff reconnect |
| `verification.rs` | the whole engine — prompt, tool loop, Exa, prefetch, both providers |
| `vision.rs` | periodic screenshots via xcap, downscaled + JPEG |
| `documents.rs` | PDF/text extraction for user context |
| `notch.rs` | NSScreen measurement + notch/island geometry |
| `panel.rs` | converts windows to non-activating NSPanels; collection behaviour + level |
| `overlay.rs` | HUD show/hide, click-through hot-zone polling, multi-monitor |
| `tray.rs` | top-centre auto-hiding dock island reveal state machine |
| `auto_start.rs` | frontmost-app detection, browser tab URL, YouTube parsing |

## Building & running

Prerequisites: **Rust** (stable), **Node 18+**, and the Tauri prerequisites for macOS (Xcode command-line tools).

```bash
npm install
npm run tauri dev      # run the app (dev)
npm run tauri build    # produce a .app bundle
```

The AppKit private-API layer (NSPanel conversion, screen-saver window level, notch
measurement, NSWorkspace frontmost app) is behind a default-on Cargo feature `appkit`.
It is what lets the HUD float over full-screen apps. To compile/test the pure logic
without it (e.g. on CI, or for a fast iteration loop):

```bash
cd src-tauri && cargo test --no-default-features   # pure logic, no objc2
cd src-tauri && cargo test                          # includes the appkit layer
```

### Configuration

Open the dashboard and paste your keys (they stay local and are used only by the Rust
backend). For development you can instead set `VITE_OPENAI_API_KEY`,
`VITE_ANTHROPIC_API_KEY`, `VITE_EXA_API_KEY`.

- **Transcription**: OpenAI Realtime (`gpt-live-transcribe`).
- **Verification**: OpenAI (`gpt-4o-mini`) or Anthropic (`claude-haiku-4-5`), chosen by which key is present — both cheap/fast tiers on purpose.
- **Search**: Exa. Web search is optional but strongly recommended; without it, claims that need current data can only be answered from model memory.

## Testing

```bash
npm test                              # vitest (frontend logic)
cd src-tauri && cargo test            # cargo (backend logic + appkit)
cd src-tauri && cargo test -- --ignored   # live smoke tests (need macOS + Automation permission)
```

Coverage concentrates on the subtle, bug-prone logic: turn detection (silence alone
never closes a turn; the ceiling always does), claim batching (waits for the rest of a
figure, two idle timeouts, spelled-out numerals, repair openers only at the start),
lexical retrieval (finds a rebuttal sharing only a figure; recency never buries it),
verification (the right conditional prompt sections; search withheld when unavailable;
prefetched evidence reaches the first turn), YouTube URL parsing (every shape; rejects
impostors), and the macOS collection-behaviour / notch-guard math.

## Billing (rough)

| Meter | Rate |
|---|---|
| `gpt-live-transcribe` | $0.017/min, continuous while live |
| `gpt-4o-mini` | $0.15 / $0.60 per M tokens, per verification |
| Exa search | $7 / 1k requests |

≈ $1.75–2.45 per hour of live session at ~3 verifications/min. Transcription is the
floor and it's time-based — which is why auto-start requires an actual video, not just a
browser being frontmost, and why every claim answered from knowledge is a real saving.

## Design notes worth reading in the code

Each of these is commented at its call site with *why* it holds the value it does:

- **Turn detection** commits only when enough cumulative voice AND a trailing pause coincide — silence alone closing a turn fragments the transcript on a throat-clear.
- **`unverifiable`** means the evidence doesn't settle the claim — it is explicitly *not* the safe default. Without that instruction the model returns grey for everything.
- **`misleading` and `false` are both red** in the HUD. Amber for misleading reads as "minor", which inverts the point — a misleading claim is a true statement arranged to deceive.
- **The HUD is a non-activating NSPanel** with `CanJoinAllSpaces | Stationary | FullScreenAuxiliary` (Managed *cleared*, not OR'd) at `NSScreenSaverWindowLevel`. Missing any one of those makes it invisible over full-screen apps in a way that looks like a different bug.
