//! Live transcription (spec §5).
//!
//! Connects to OpenAI Realtime and streams 24 kHz mono PCM from the [`AudioBus`].
//!
//! Turn detection is CLIENT-SIDE and must be: the live transcription model rejects a
//! server-side VAD configuration, so we detect turns ourselves and commit the buffer
//! explicitly. Both halves of the commit condition are required — enough cumulative
//! voice AND a trailing pause — because silence alone closing a turn means a throat
//! clear fragments the transcript.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde::Serialize;

use crate::bus::AudioBus;

// --- Turn-detection thresholds. Each value buys time on the speak→verdict path or
//     prevents a specific fragmentation bug; see the commit condition below. ---

/// A frame counts as speech above this RMS. Tuned above room-tone / fan noise.
pub const VOICED_RMS: f32 = 0.012;
/// Cumulative voice required before a pause is allowed to close a turn. Stops a cough
/// or "um—" from being committed as its own (empty) utterance.
pub const MIN_VOICED_MS: u32 = 1_200;
/// Trailing silence that then closes a turn once enough voice has accrued.
pub const SILENCE_HOLD_MS: u32 = 600;
/// Hard ceiling for unbroken speech — commit even mid-sentence so a monologue still
/// produces transcript to check rather than buffering forever.
pub const MAX_UTTERANCE_MS: u32 = 15_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitReason {
    /// Enough voice, then a pause — the natural end of a turn.
    Pause,
    /// The unbroken-speech ceiling was hit.
    Ceiling,
}

/// Pure, testable turn detector. Fed frames of 24 kHz mono PCM; tells the caller when
/// to commit the buffer.
#[derive(Debug, Default)]
pub struct TurnDetector {
    voiced_ms: u32,
    silence_ms: u32,
    total_ms: u32,
    sample_rate: u32,
}

impl TurnDetector {
    pub fn new(sample_rate: u32) -> Self {
        Self { sample_rate: sample_rate.max(1), ..Default::default() }
    }

    pub fn rms(frame: &[f32]) -> f32 {
        if frame.is_empty() {
            return 0.0;
        }
        let sum_sq: f32 = frame.iter().map(|s| s * s).sum();
        (sum_sq / frame.len() as f32).sqrt()
    }

    fn frame_ms(&self, samples: usize) -> u32 {
        ((samples as f64 / self.sample_rate as f64) * 1000.0).round() as u32
    }

    /// Feed one frame. Returns Some(reason) if a turn should be committed now; the
    /// detector resets on commit so the next call begins a fresh utterance.
    pub fn push_frame(&mut self, frame: &[f32]) -> Option<CommitReason> {
        let dur = self.frame_ms(frame.len());
        self.total_ms += dur;

        if Self::rms(frame) >= VOICED_RMS {
            self.voiced_ms += dur;
            self.silence_ms = 0; // voice breaks the trailing-silence run
        } else {
            self.silence_ms += dur;
        }

        // Ceiling first: commit unconditionally, even with zero trailing silence.
        if self.total_ms >= MAX_UTTERANCE_MS {
            self.reset();
            return Some(CommitReason::Ceiling);
        }

        // Pause: BOTH halves required. Silence alone must never close a turn.
        if self.voiced_ms >= MIN_VOICED_MS && self.silence_ms >= SILENCE_HOLD_MS {
            self.reset();
            return Some(CommitReason::Pause);
        }
        None
    }

    pub fn reset(&mut self) {
        self.voiced_ms = 0;
        self.silence_ms = 0;
        self.total_ms = 0;
    }

    pub fn voiced_ms(&self) -> u32 {
        self.voiced_ms
    }
    pub fn silence_ms(&self) -> u32 {
        self.silence_ms
    }
}

// --- Reconnect policy ---

/// Exponential backoff: 500ms → 15s, doubling, capped. Attempt is 0-indexed.
pub fn backoff_delay(attempt: u32) -> Duration {
    const BASE_MS: u64 = 500;
    const CAP_MS: u64 = 15_000;
    let ms = BASE_MS.saturating_mul(1u64 << attempt.min(20)).min(CAP_MS);
    Duration::from_millis(ms)
}

pub const MAX_RECONNECT_ATTEMPTS: u32 = 6;

/// A bad key or an unknown model will never succeed on retry — treat as terminal so we
/// don't burn six reconnects and a delay proving it.
pub fn is_retryable_close(code: u16, reason: &str) -> bool {
    let r = reason.to_lowercase();
    // 4001/4003-style auth failures, or an explicit invalid-model / invalid-api-key.
    if r.contains("invalid api key")
        || r.contains("unauthorized")
        || r.contains("authentication")
        || r.contains("invalid model")
        || r.contains("unknown model")
        || r.contains("model_not_found")
    {
        return false;
    }
    // 1008 policy violation / 4000-range auth codes are terminal; transient network
    // and server codes are retryable.
    !matches!(code, 1008 | 4001 | 4003)
}

// --- PCM16 encoding for the wire ---

/// Encode f32 [-1,1] samples as little-endian PCM16, base64 — the wire format the
/// Realtime endpoint expects for `input_audio_buffer.append`.
pub fn encode_pcm16_base64(frame: &[f32]) -> String {
    let mut bytes = Vec::with_capacity(frame.len() * 2);
    for &s in frame {
        let clamped = s.clamp(-1.0, 1.0);
        let v = (clamped * i16::MAX as f32) as i16;
        bytes.extend_from_slice(&v.to_le_bytes());
    }
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// The session.update payload. The model goes in the session CONFIG, not the query
/// string: the endpoint reads ?model= as the realtime *session* model, a different
/// thing from the transcription model we actually want.
pub fn session_update_json(model: &str) -> serde_json::Value {
    serde_json::json!({
        "type": "session.update",
        "session": {
            "input_audio_format": "pcm16",
            "input_audio_transcription": { "model": model },
            // We do our own turn detection; server VAD is disabled (and rejected).
            "turn_detection": null
        }
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct TranscriptEvent {
    pub kind: TranscriptKind,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum TranscriptKind {
    Partial,
    Final,
    Error,
}

pub struct TranscriptionConfig {
    pub api_key: String,
    pub model: String,
}

pub struct TranscriptionHandle {
    stop: Arc<AtomicBool>,
}

impl TranscriptionHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Spawn the transcription session on the current tokio runtime. `emit` forwards
/// transcript events to the webview (the caller wires it to a Tauri event).
pub fn spawn(
    bus: AudioBus,
    config: TranscriptionConfig,
    emit: impl Fn(TranscriptEvent) + Send + Sync + 'static,
) -> TranscriptionHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_task = stop.clone();
    let emit = Arc::new(emit);

    tokio::spawn(async move {
        let mut attempt = 0u32;
        while !stop_task.load(Ordering::Relaxed) && attempt < MAX_RECONNECT_ATTEMPTS {
            match run_once(&bus, &config, emit.clone(), stop_task.clone()).await {
                Ok(()) => break, // stopped cleanly
                Err(SessionError::Terminal(msg)) => {
                    emit(TranscriptEvent { kind: TranscriptKind::Error, text: msg });
                    break;
                }
                Err(SessionError::Transient(msg)) => {
                    log::warn!("transcription transient error (attempt {attempt}): {msg}");
                    tokio::time::sleep(backoff_delay(attempt)).await;
                    attempt += 1;
                }
            }
        }
    });

    TranscriptionHandle { stop }
}

enum SessionError {
    Transient(String),
    Terminal(String),
}

async fn run_once(
    bus: &AudioBus,
    config: &TranscriptionConfig,
    emit: Arc<dyn Fn(TranscriptEvent) + Send + Sync>,
    stop: Arc<AtomicBool>,
) -> Result<(), SessionError> {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::http::header::{AUTHORIZATION, HeaderValue};
    use tokio_tungstenite::tungstenite::Message;

    // Model is configured via session.update, so the URL carries only the intent.
    let mut req = "wss://api.openai.com/v1/realtime?intent=transcription"
        .into_client_request()
        .map_err(|e| SessionError::Terminal(e.to_string()))?;
    req.headers_mut().insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", config.api_key))
            .map_err(|_| SessionError::Terminal("invalid api key header".into()))?,
    );
    req.headers_mut()
        .insert("OpenAI-Beta", HeaderValue::from_static("realtime=v1"));

    let (ws, _resp) = tokio_tungstenite::connect_async(req)
        .await
        .map_err(|e| classify_connect_error(e))?;
    let (mut write, mut read) = ws.split();

    write
        .send(Message::Text(session_update_json(&config.model).to_string()))
        .await
        .map_err(|e| SessionError::Transient(e.to_string()))?;

    // Writer: pull PCM from the bus, run turn detection, append + commit.
    let mut rx = bus.subscribe();
    let mut detector = TurnDetector::new(crate::audio::TARGET_RATE);
    let bus_writer = bus.clone();
    let stop_writer = stop.clone();

    let writer = async move {
        while !stop_writer.load(Ordering::Relaxed) {
            let Some(frame) = bus_writer.recv_counting(&mut rx).await else {
                break;
            };
            let append = serde_json::json!({
                "type": "input_audio_buffer.append",
                "audio": encode_pcm16_base64(&frame),
            });
            if write.send(Message::Text(append.to_string())).await.is_err() {
                break;
            }
            if detector.push_frame(&frame).is_some() {
                let commit = serde_json::json!({ "type": "input_audio_buffer.commit" });
                if write.send(Message::Text(commit.to_string())).await.is_err() {
                    break;
                }
            }
        }
        let _ = write.close().await;
    };

    // Reader: parse transcription events and emit them.
    let emit_reader = emit.clone();
    let stop_reader = stop.clone();
    let reader = async move {
        while let Some(msg) = read.next().await {
            if stop_reader.load(Ordering::Relaxed) {
                break;
            }
            match msg {
                Ok(Message::Text(txt)) => {
                    if let Some(ev) = parse_server_event(&txt) {
                        emit_reader(ev);
                    }
                }
                Ok(Message::Close(frame)) => {
                    if let Some(f) = frame {
                        let code: u16 = f.code.into();
                        if !is_retryable_close(code, &f.reason) {
                            return Err(SessionError::Terminal(format!(
                                "transcription closed: {}",
                                f.reason
                            )));
                        }
                    }
                    return Err(SessionError::Transient("connection closed".into()));
                }
                Err(e) => return Err(SessionError::Transient(e.to_string())),
                _ => {}
            }
        }
        Ok(())
    };

    tokio::select! {
        _ = writer => Ok(()),
        r = reader => r,
    }
}

fn classify_connect_error(e: tokio_tungstenite::tungstenite::Error) -> SessionError {
    let s = e.to_string();
    if !is_retryable_close(0, &s) {
        SessionError::Terminal(s)
    } else {
        SessionError::Transient(s)
    }
}

/// Turn a Realtime server frame into a transcript event, if it is one we care about.
pub fn parse_server_event(txt: &str) -> Option<TranscriptEvent> {
    let v: serde_json::Value = serde_json::from_str(txt).ok()?;
    let ty = v.get("type")?.as_str()?;
    match ty {
        "conversation.item.input_audio_transcription.delta" => Some(TranscriptEvent {
            kind: TranscriptKind::Partial,
            text: v.get("delta")?.as_str()?.to_string(),
        }),
        "conversation.item.input_audio_transcription.completed" => Some(TranscriptEvent {
            kind: TranscriptKind::Final,
            text: v.get("transcript")?.as_str()?.to_string(),
        }),
        "error" => Some(TranscriptEvent {
            kind: TranscriptKind::Error,
            text: v
                .get("error")
                .and_then(|e| e.get("message"))
                .and_then(|m| m.as_str())
                .unwrap_or("unknown error")
                .to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 24_000;

    fn voiced(ms: u32) -> Vec<f32> {
        vec![0.5; (RATE as u64 * ms as u64 / 1000) as usize]
    }
    fn silent(ms: u32) -> Vec<f32> {
        vec![0.0; (RATE as u64 * ms as u64 / 1000) as usize]
    }

    #[test]
    fn rms_of_constant_signal_is_its_amplitude() {
        assert!((TurnDetector::rms(&[0.5; 100]) - 0.5).abs() < 1e-6);
        assert_eq!(TurnDetector::rms(&[0.0; 100]), 0.0);
    }

    #[test]
    fn silence_alone_never_closes_a_turn() {
        let mut d = TurnDetector::new(RATE);
        // Long silence, no voice at all — must never commit (below the ceiling).
        for _ in 0..10 {
            assert_eq!(d.push_frame(&silent(1000)), None);
        }
    }

    #[test]
    fn a_short_pause_after_little_speech_does_not_close() {
        let mut d = TurnDetector::new(RATE);
        // 500ms voice (< MIN_VOICED_MS), then a 700ms pause (> SILENCE_HOLD_MS).
        assert_eq!(d.push_frame(&voiced(500)), None);
        assert_eq!(d.push_frame(&silent(700)), None); // voiced_ms too low → no commit
    }

    #[test]
    fn enough_voice_then_a_pause_commits() {
        let mut d = TurnDetector::new(RATE);
        assert_eq!(d.push_frame(&voiced(1_300)), None); // over MIN_VOICED_MS
        assert_eq!(d.push_frame(&silent(700)), Some(CommitReason::Pause));
    }

    #[test]
    fn a_pause_shorter_than_the_hold_does_not_commit() {
        let mut d = TurnDetector::new(RATE);
        d.push_frame(&voiced(1_300));
        assert_eq!(d.push_frame(&silent(400)), None); // < SILENCE_HOLD_MS
    }

    #[test]
    fn voice_resets_the_trailing_silence_run() {
        let mut d = TurnDetector::new(RATE);
        d.push_frame(&voiced(1_300));
        d.push_frame(&silent(500)); // approaching the hold...
        d.push_frame(&voiced(100)); // ...but voice resets it
        assert_eq!(d.silence_ms(), 0);
        assert_eq!(d.push_frame(&silent(500)), None); // 500 < 600 again
    }

    #[test]
    fn unbroken_speech_commits_at_the_ceiling() {
        let mut d = TurnDetector::new(RATE);
        // 15s of pure voice, no pause at all — the ceiling must still commit.
        let mut committed = None;
        for _ in 0..15 {
            if let Some(r) = d.push_frame(&voiced(1_000)) {
                committed = Some(r);
                break;
            }
        }
        assert_eq!(committed, Some(CommitReason::Ceiling));
    }

    #[test]
    fn detector_resets_after_a_commit() {
        let mut d = TurnDetector::new(RATE);
        d.push_frame(&voiced(1_300));
        d.push_frame(&silent(700));
        assert_eq!(d.voiced_ms(), 0);
        assert_eq!(d.silence_ms(), 0);
    }

    #[test]
    fn backoff_grows_then_caps_at_15s() {
        assert_eq!(backoff_delay(0), Duration::from_millis(500));
        assert_eq!(backoff_delay(1), Duration::from_millis(1_000));
        assert_eq!(backoff_delay(2), Duration::from_millis(2_000));
        assert_eq!(backoff_delay(3), Duration::from_millis(4_000));
        assert_eq!(backoff_delay(4), Duration::from_millis(8_000));
        assert_eq!(backoff_delay(5), Duration::from_millis(15_000)); // capped
        assert_eq!(backoff_delay(9), Duration::from_millis(15_000));
    }

    #[test]
    fn bad_key_and_unknown_model_are_not_retryable() {
        assert!(!is_retryable_close(4001, "Invalid API key provided"));
        assert!(!is_retryable_close(1008, "unknown model gpt-live-transcribe"));
        assert!(!is_retryable_close(0, "authentication failed"));
        // Transient network / server closes ARE retryable.
        assert!(is_retryable_close(1006, "abnormal closure"));
        assert!(is_retryable_close(1011, "server error"));
    }

    #[test]
    fn session_update_puts_model_in_config_not_query() {
        let j = session_update_json("gpt-live-transcribe");
        assert_eq!(
            j["session"]["input_audio_transcription"]["model"],
            "gpt-live-transcribe"
        );
        assert!(j["session"]["turn_detection"].is_null());
    }

    #[test]
    fn pcm16_base64_roundtrips_length() {
        // 2 samples → 4 bytes → base64 of 4 bytes.
        let b64 = encode_pcm16_base64(&[0.0, 1.0]);
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(b64)
            .unwrap();
        assert_eq!(bytes.len(), 4);
        // Full-scale 1.0 encodes to i16::MAX.
        assert_eq!(i16::from_le_bytes([bytes[2], bytes[3]]), i16::MAX);
    }

    #[test]
    fn parses_final_transcript_event() {
        let txt = r#"{"type":"conversation.item.input_audio_transcription.completed","transcript":"unemployment is 8 percent"}"#;
        let ev = parse_server_event(txt).unwrap();
        assert_eq!(ev.kind, TranscriptKind::Final);
        assert_eq!(ev.text, "unemployment is 8 percent");
    }

    #[test]
    fn parses_partial_and_ignores_unknown() {
        let d = r#"{"type":"conversation.item.input_audio_transcription.delta","delta":"unem"}"#;
        assert_eq!(parse_server_event(d).unwrap().kind, TranscriptKind::Partial);
        assert!(parse_server_event(r#"{"type":"response.created"}"#).is_none());
    }
}
