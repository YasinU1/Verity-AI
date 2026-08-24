//! Audio capture (spec §3/§4).
//!
//! cpal captures at the device's native rate; we resample to 24 kHz mono PCM and
//! publish frames to the in-process [`AudioBus`]. 24 kHz mono is what the OpenAI
//! Realtime transcription endpoint expects, and doing the conversion here keeps the
//! webview out of the audio path entirely.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde::Serialize;

use crate::bus::AudioBus;

/// The transcription endpoint's expected input rate.
pub const TARGET_RATE: u32 = 24_000;

// --- Pure DSP (unit-tested; no hardware involved) ---

/// Average all channels down to mono. Live transcription is mono; capturing stereo and
/// discarding a channel would silently halve loudness on hard-panned sources.
pub fn downmix_to_mono(interleaved: &[f32], channels: u16) -> Vec<f32> {
    if channels <= 1 {
        return interleaved.to_vec();
    }
    let ch = channels as usize;
    let frames = interleaved.len() / ch;
    let mut out = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut sum = 0.0f32;
        for c in 0..ch {
            sum += interleaved[f * ch + c];
        }
        out.push(sum / ch as f32);
    }
    out
}

/// Linear-interpolation resample. Cheap and good enough for speech at these rates;
/// a polyphase filter would be higher fidelity but the transcription model does not
/// need it, and latency is the constraint that matters here.
pub fn resample_linear(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
    if in_rate == out_rate || input.is_empty() {
        return input.to_vec();
    }
    let ratio = out_rate as f64 / in_rate as f64;
    let out_len = ((input.len() as f64) * ratio).round() as usize;
    let mut out = Vec::with_capacity(out_len);
    let last = input.len() - 1;
    for i in 0..out_len {
        let src = i as f64 / ratio;
        let idx = src.floor() as usize;
        let frac = src - idx as f64;
        let a = input[idx.min(last)] as f64;
        let b = input[(idx + 1).min(last)] as f64;
        out.push((a + (b - a) * frac) as f32);
    }
    out
}

pub fn resample_to_24k_mono(interleaved: &[f32], in_rate: u32, channels: u16) -> Vec<f32> {
    let mono = downmix_to_mono(interleaved, channels);
    resample_linear(&mono, in_rate, TARGET_RATE)
}

/// Heuristic: does a device name look like a loopback / system-audio capture device?
/// macOS has no first-class "record system output", so users route it through a
/// virtual device (BlackHole, Loopback, an Aggregate/Multi-Output). We surface which
/// devices can do that so the UI can pick one for the "system" audio source.
pub fn is_loopback_name(name: &str) -> bool {
    let n = name.to_lowercase();
    ["blackhole", "loopback", "aggregate", "soundflower", "multi-output", "vb-cable"]
        .iter()
        .any(|k| n.contains(k))
}

#[derive(Serialize, Clone, Debug)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub supports_loopback: bool,
}

pub fn list_devices() -> Vec<AudioDevice> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();
    let default_name = host.default_input_device().and_then(|d| d.name().ok());
    let mut out = Vec::new();
    if let Ok(devs) = host.input_devices() {
        for d in devs {
            if let Ok(name) = d.name() {
                out.push(AudioDevice {
                    is_default: default_name.as_ref() == Some(&name),
                    supports_loopback: is_loopback_name(&name),
                    id: name.clone(),
                    name,
                });
            }
        }
    }
    out
}

// --- Capture stream (integration; cpal Stream is !Send so it lives on its own thread) ---

pub struct CaptureHandle {
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl CaptureHandle {
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for CaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Start capturing from `device_id` (or the default input) into `bus`.
pub fn start_capture(
    bus: AudioBus,
    device_id: Option<String>,
) -> Result<CaptureHandle, String> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<(), String>>();

    let join = std::thread::spawn(move || {
        match build_stream(bus, device_id.as_deref()) {
            Ok(stream) => {
                use cpal::traits::StreamTrait;
                if let Err(e) = stream.play() {
                    let _ = ready_tx.send(Err(format!("audio: play failed: {e}")));
                    return;
                }
                let _ = ready_tx.send(Ok(()));
                // Own the stream on this thread until asked to stop.
                while !stop_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = ready_tx.send(Err(e));
            }
        }
    });

    match ready_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Ok(())) => Ok(CaptureHandle { stop, join: Some(join) }),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("audio: stream did not start within 5s".into()),
    }
}

fn build_stream(bus: AudioBus, device_id: Option<&str>) -> Result<cpal::Stream, String> {
    use cpal::traits::{DeviceTrait, HostTrait};
    let host = cpal::default_host();

    let device = match device_id {
        Some(id) => host
            .input_devices()
            .map_err(|e| e.to_string())?
            .find(|d| d.name().ok().as_deref() == Some(id))
            .ok_or_else(|| format!("audio: device '{id}' not found"))?,
        None => host
            .default_input_device()
            .ok_or_else(|| "audio: no default input device".to_string())?,
    };

    let config = device.default_input_config().map_err(|e| e.to_string())?;
    let sample_rate = config.sample_rate().0;
    let channels = config.channels();
    let err_fn = |e| log::error!("audio stream error: {e}");

    let publish = move |samples: Vec<f32>| {
        let pcm = resample_to_24k_mono(&samples, sample_rate, channels);
        if !pcm.is_empty() {
            bus.publish(Arc::new(pcm));
        }
    };

    let stream = match config.sample_format() {
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config.into(),
            move |data: &[f32], _| publish(data.to_vec()),
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config.into(),
            move |data: &[i16], _| {
                publish(data.iter().map(|s| *s as f32 / i16::MAX as f32).collect())
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config.into(),
            move |data: &[u16], _| {
                publish(
                    data.iter()
                        .map(|s| (*s as f32 / u16::MAX as f32) * 2.0 - 1.0)
                        .collect(),
                )
            },
            err_fn,
            None,
        ),
        fmt => return Err(format!("audio: unsupported sample format {fmt:?}")),
    };

    stream.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn downmix_averages_stereo() {
        // L/R interleaved: (1,0), (0,1) → 0.5, 0.5
        let out = downmix_to_mono(&[1.0, 0.0, 0.0, 1.0], 2);
        assert_eq!(out, vec![0.5, 0.5]);
    }

    #[test]
    fn downmix_mono_is_identity() {
        assert_eq!(downmix_to_mono(&[0.1, 0.2, 0.3], 1), vec![0.1, 0.2, 0.3]);
    }

    #[test]
    fn resample_identity_when_rates_match() {
        let x = vec![0.0, 0.5, 1.0];
        assert_eq!(resample_linear(&x, 24_000, 24_000), x);
    }

    #[test]
    fn resample_downsample_shrinks_length() {
        // 48k → 24k halves the sample count (±1).
        let x: Vec<f32> = (0..48).map(|i| i as f32).collect();
        let out = resample_linear(&x, 48_000, 24_000);
        assert!((out.len() as i64 - 24).abs() <= 1, "got {}", out.len());
    }

    #[test]
    fn resample_upsample_grows_length() {
        let x: Vec<f32> = (0..24).map(|i| i as f32).collect();
        let out = resample_linear(&x, 24_000, 48_000);
        assert!((out.len() as i64 - 48).abs() <= 1, "got {}", out.len());
    }

    #[test]
    fn full_chain_produces_24k_mono() {
        // 1000 stereo frames at 48k → ~500 mono samples at 24k.
        let stereo: Vec<f32> = (0..2000).map(|i| (i % 2) as f32).collect();
        let out = resample_to_24k_mono(&stereo, 48_000, 2);
        assert!((out.len() as i64 - 500).abs() <= 2, "got {}", out.len());
    }

    #[test]
    fn loopback_detection() {
        assert!(is_loopback_name("BlackHole 2ch"));
        assert!(is_loopback_name("Loopback Audio"));
        assert!(is_loopback_name("Aggregate Device"));
        assert!(!is_loopback_name("MacBook Pro Microphone"));
        assert!(!is_loopback_name("External Headphones"));
    }
}
