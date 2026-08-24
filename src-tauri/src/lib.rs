//! Verity — real-time misinformation checker. Tauri backend entrypoint.
//!
//! Two windows share one frontend bundle, routed by URL hash: `main` (the docked
//! dashboard) and `overlay` (the click-through HUD). All model/search network calls
//! happen here in Rust — API keys never enter page context.

pub mod audio;
pub mod auto_start;
pub mod bus;
pub mod documents;
pub mod notch;
pub mod overlay;
pub mod panel;
pub mod transcription;
pub mod tray;
pub mod verification;
pub mod vision;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};

use audio::CaptureHandle;
use bus::AudioBus;
use overlay::Rect;
use transcription::TranscriptionHandle;
use verification::{VerdictResult, VerifyEngine, VerifyError};
use vision::VisionHandle;

/// Shared application state. Handles are kept so captures/sessions can be stopped, and
/// the reqwest client + semaphore are shared so the verification concurrency cap (3) is
/// GLOBAL across every verify_claim call.
pub struct AppState {
    bus: AudioBus,
    http: reqwest::Client,
    verify_sem: Arc<tokio::sync::Semaphore>,
    capture: Mutex<Option<CaptureHandle>>,
    vision: Mutex<Option<VisionHandle>>,
    transcription: Mutex<Option<TranscriptionHandle>>,
    hot_zone: Mutex<Option<Rect>>,
    dock_pinned: AtomicBool,
    overlay_manual_ignore: Mutex<Option<bool>>,
    notch: Mutex<notch::NotchGeometry>,
}

impl AppState {
    fn new() -> Self {
        Self {
            bus: AudioBus::new(256),
            http: reqwest::Client::new(),
            verify_sem: Arc::new(tokio::sync::Semaphore::new(verification::MAX_CONCURRENCY)),
            capture: Mutex::new(None),
            vision: Mutex::new(None),
            transcription: Mutex::new(None),
            hot_zone: Mutex::new(None),
            dock_pinned: AtomicBool::new(false),
            overlay_manual_ignore: Mutex::new(None),
            notch: Mutex::new(notch::compute_notch(0.0, 0.0, 0.0, 1440.0)),
        }
    }
}

// --- Overlay / windowing commands ---

#[tauri::command]
fn toggle_overlay(app: AppHandle) -> Result<bool, String> {
    let win = app.get_webview_window("overlay").ok_or("overlay window missing")?;
    let visible = win.is_visible().unwrap_or(false);
    if visible {
        win.hide().map_err(|e| e.to_string())?;
        Ok(false)
    } else {
        win.show().map_err(|e| e.to_string())?;
        Ok(true)
    }
}

#[tauri::command]
fn set_overlay_click_through(
    app: AppHandle,
    state: State<AppState>,
    enabled: bool,
) -> Result<(), String> {
    // A manual override; None hands control back to the hot-zone poll loop.
    *state.overlay_manual_ignore.lock().unwrap() = Some(enabled);
    if let Some(win) = app.get_webview_window("overlay") {
        win.set_ignore_cursor_events(enabled).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// The webview reports its painted region (via ResizeObserver) so Rust can poll the
/// cursor against it — the only way to get click-through-except-paint, since an ignored
/// window delivers no DOM mouse events.
#[tauri::command]
fn set_overlay_hot_zone(state: State<AppState>, rect: Option<Rect>) -> Result<(), String> {
    *state.hot_zone.lock().unwrap() = rect;
    // A fresh hot zone means the manual override no longer applies.
    *state.overlay_manual_ignore.lock().unwrap() = None;
    Ok(())
}

#[tauri::command]
fn focus_dashboard(app: AppHandle) -> Result<(), String> {
    if let Some(win) = app.get_webview_window("main") {
        win.show().map_err(|e| e.to_string())?;
        win.set_focus().map_err(|e| e.to_string())?;
    }
    Ok(())
}

#[derive(Serialize)]
struct DockChrome {
    notch: notch::NotchGeometry,
    island: notch::IslandRect,
    reveal_band: notch::RevealBand,
}

#[tauri::command]
fn dock_chrome(state: State<AppState>) -> DockChrome {
    let g = *state.notch.lock().unwrap();
    let island = notch::compute_island(&g);
    let reveal_band = notch::compute_reveal_band(&island);
    DockChrome { notch: g, island, reveal_band }
}

#[tauri::command]
fn set_dock_pinned(state: State<AppState>, pinned: bool) {
    state.dock_pinned.store(pinned, Ordering::Relaxed);
}

#[tauri::command]
fn get_active_app() -> auto_start::AutoStartStatus {
    auto_start::poll()
}

// --- Audio ---

#[tauri::command]
fn list_audio_devices() -> Vec<audio::AudioDevice> {
    audio::list_devices()
}

#[tauri::command]
fn start_audio_capture(state: State<AppState>, device_id: Option<String>) -> Result<(), String> {
    let mut guard = state.capture.lock().unwrap();
    if let Some(mut h) = guard.take() {
        h.stop();
    }
    let handle = audio::start_capture(state.bus.clone(), device_id)?;
    *guard = Some(handle);
    Ok(())
}

#[tauri::command]
fn stop_audio_capture(state: State<AppState>) {
    if let Some(mut h) = state.capture.lock().unwrap().take() {
        h.stop();
    }
}

#[derive(Serialize)]
struct CaptureStatus {
    capturing: bool,
    transcribing: bool,
    dropped_frames: u64,
    published_frames: u64,
}

#[tauri::command]
fn capture_status(state: State<AppState>) -> CaptureStatus {
    CaptureStatus {
        capturing: state.capture.lock().unwrap().is_some(),
        transcribing: state.transcription.lock().unwrap().is_some(),
        dropped_frames: state.bus.dropped(),
        published_frames: state.bus.published(),
    }
}

// --- Vision ---

#[tauri::command]
fn list_monitors() -> Vec<vision::MonitorInfo> {
    vision::list_monitors()
}

#[tauri::command]
fn start_vision_capture(
    app: AppHandle,
    state: State<AppState>,
    monitor_index: Option<u32>,
    interval_ms: Option<u64>,
) -> Result<(), String> {
    let mut guard = state.vision.lock().unwrap();
    if let Some(h) = guard.take() {
        h.stop();
    }
    let app2 = app.clone();
    let handle = vision::spawn(
        monitor_index.unwrap_or(0),
        interval_ms.unwrap_or(4000),
        vision::DEFAULT_MAX_DIM,
        move |frame| {
            let _ = app2.emit("vision-frame", frame);
        },
    );
    *guard = Some(handle);
    Ok(())
}

#[tauri::command]
fn stop_vision_capture(state: State<AppState>) {
    if let Some(h) = state.vision.lock().unwrap().take() {
        h.stop();
    }
}

// --- Documents ---

#[tauri::command]
fn extract_document_text(path: String) -> Result<String, String> {
    documents::extract_document_text(&path).map(|t| documents::normalize_extracted(&t))
}

// --- Transcription ---

#[tauri::command]
fn start_transcription(
    app: AppHandle,
    state: State<AppState>,
    api_key: String,
    model: Option<String>,
) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("transcription: OpenAI API key required".into());
    }
    let mut guard = state.transcription.lock().unwrap();
    if let Some(h) = guard.take() {
        h.stop();
    }
    let app2 = app.clone();
    let handle = transcription::spawn(
        state.bus.clone(),
        transcription::TranscriptionConfig {
            api_key,
            model: model.unwrap_or_else(|| "gpt-live-transcribe".into()),
        },
        move |ev| {
            let _ = app2.emit("transcript", ev);
        },
    );
    *guard = Some(handle);
    Ok(())
}

#[tauri::command]
fn stop_transcription(state: State<AppState>) {
    if let Some(h) = state.transcription.lock().unwrap().take() {
        h.stop();
    }
}

// --- Verification ---

#[derive(Deserialize)]
struct VerifyRequest {
    claim: String,
    #[serde(default)]
    brief: String,
    #[serde(default)]
    openai_key: Option<String>,
    #[serde(default)]
    anthropic_key: Option<String>,
    #[serde(default)]
    exa_key: Option<String>,
    #[serde(default = "default_true")]
    web_search_enabled: bool,
    #[serde(default = "default_auto")]
    provider: String,
    #[serde(default = "default_openai_model")]
    openai_model: String,
    #[serde(default = "default_anthropic_model")]
    anthropic_model: String,
}

fn default_true() -> bool {
    true
}
fn default_auto() -> String {
    "auto".into()
}
fn default_openai_model() -> String {
    "gpt-4o-mini".into()
}
fn default_anthropic_model() -> String {
    "claude-haiku-4-5".into()
}

#[tauri::command]
async fn verify_claim(state: State<'_, AppState>, req: VerifyRequest) -> Result<VerdictResult, String> {
    // Pull shared handles out before the await so we don't hold the State guard across it.
    let engine = VerifyEngine::with_shared(
        state.http.clone(),
        state.verify_sem.clone(),
        req.openai_key.filter(|k| !k.is_empty()),
        req.anthropic_key.filter(|k| !k.is_empty()),
        req.exa_key.filter(|k| !k.is_empty()),
        req.web_search_enabled,
        req.provider,
        req.openai_model,
        req.anthropic_model,
    );

    match engine.verify(&req.claim, &req.brief).await {
        Ok(result) => Ok(result),
        // Surfaced distinctly so the caller can drop (not retry) a saturated check.
        Err(VerifyError::Saturated) => Err("SATURATED".into()),
        Err(VerifyError::NoProvider) => Err("NO_PROVIDER".into()),
        Err(VerifyError::Failed(e)) => Err(e),
    }
}

// --- Setup: measure the screen, convert windows to panels, start the poll loop ---

fn apply_window_chrome(app: &AppHandle) {
    // Convert both windows to non-activating floating panels ONCE (panel::make_panel is
    // idempotent — re-classing a live window on every reveal aborts the process).
    for label in ["overlay", "main"] {
        if let Some(win) = app.get_webview_window(label) {
            let _ = win.set_always_on_top(true);
            panel::make_panel(&win); // applies collection behaviour + screen-saver level
        }
    }
}

/// One poll tick, run on the main thread (NSScreen/NSEvent need it): toggle overlay
/// click-through against the painted hot zone, and drive the dock reveal state.
fn poll_tick(app: &AppHandle) {
    let state = app.state::<AppState>();

    // Overlay click-through.
    if let Some(win) = app.get_webview_window("overlay") {
        let manual = *state.overlay_manual_ignore.lock().unwrap();
        let ignore = match manual {
            Some(v) => v, // an explicit override wins until the next hot-zone update
            None => {
                let zone = *state.hot_zone.lock().unwrap();
                match overlay::cursor_position() {
                    Some((cx, cy)) => overlay::should_ignore_cursor(cx, cy, zone.as_ref()),
                    None => true,
                }
            }
        };
        let _ = win.set_ignore_cursor_events(ignore);
    }

    // Dock reveal → emit a state change the frontend animates.
    let g = *state.notch.lock().unwrap();
    let band = tray::reveal_band_for(&g);
    let pinned = state.dock_pinned.load(Ordering::Relaxed);
    let expanded_bounds = Rect {
        x: g.screen_width / 2.0 - 360.0,
        y: 0.0,
        width: 720.0,
        height: 140.0,
    };
    let next = tray::next_dock_state(
        tray::DockState::Collapsed,
        pinned,
        overlay::cursor_position(),
        &band,
        &expanded_bounds,
    );
    let _ = app.emit("dock-state", matches!(next, tray::DockState::Expanded));
}

pub fn run() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .manage(AppState::new())
        .setup(|app| {
            let handle = app.handle().clone();

            // Accessory app: no Space of its own, so activating Verity leaves the current
            // Space (and any full-screen video) alone. Trade-off: no Dock icon, no Cmd-Tab.
            #[cfg(target_os = "macos")]
            let _ = app.set_activation_policy(tauri::ActivationPolicy::Accessory);

            // Measure the screen once, on the main thread, and cache it.
            {
                let g = notch::measure();
                *app.state::<AppState>().notch.lock().unwrap() = g;
            }

            apply_window_chrome(&handle);

            // Cursor/dock poll loop. Each tick hops to the main thread because
            // NSEvent.mouseLocation + NSScreen must be read there.
            let loop_handle = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(overlay::CURSOR_POLL_MS));
                let h = loop_handle.clone();
                let _ = loop_handle.run_on_main_thread(move || poll_tick(&h));
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Every command is registered here — a command defined but omitted from this
            // list is silently unavailable to the webview (a real shipped failure mode).
            toggle_overlay,
            set_overlay_click_through,
            set_overlay_hot_zone,
            focus_dashboard,
            capture_status,
            set_dock_pinned,
            dock_chrome,
            get_active_app,
            list_audio_devices,
            start_audio_capture,
            stop_audio_capture,
            list_monitors,
            start_vision_capture,
            stop_vision_capture,
            extract_document_text,
            start_transcription,
            stop_transcription,
            verify_claim,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Verity");
}
