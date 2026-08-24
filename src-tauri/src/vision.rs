//! Periodic screen capture (spec §4). Screenshots via xcap, downscaled and
//! JPEG-compressed so a frame is cheap to hand to a vision model for on-screen context
//! (slides, chyrons, shared documents). Deliberately low-rate — this is context, not a
//! video feed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use base64::Engine;
use serde::Serialize;

/// Compute downscaled dimensions preserving aspect ratio, longest side <= max_dim.
/// A 5K screenshot is pointless to send whole; the model reads text fine downscaled and
/// the payload shrinks by an order of magnitude.
pub fn scaled_dimensions(w: u32, h: u32, max_dim: u32) -> (u32, u32) {
    if w <= max_dim && h <= max_dim {
        return (w.max(1), h.max(1));
    }
    let longest = w.max(h) as f64;
    let scale = max_dim as f64 / longest;
    (
        ((w as f64 * scale).round() as u32).max(1),
        ((h as f64 * scale).round() as u32).max(1),
    )
}

pub const DEFAULT_MAX_DIM: u32 = 1280;
pub const JPEG_QUALITY: u8 = 70;

#[derive(Serialize, Clone, Debug)]
pub struct MonitorInfo {
    pub id: u32,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub is_primary: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct VisionFrame {
    pub monitor: String,
    pub width: u32,
    pub height: u32,
    /// JPEG bytes, base64. A `data:` URL for the webview to display, or model input.
    pub jpeg_base64: String,
}

pub fn list_monitors() -> Vec<MonitorInfo> {
    match xcap::Monitor::all() {
        Ok(monitors) => monitors
            .into_iter()
            .enumerate()
            .map(|(i, m)| MonitorInfo {
                id: i as u32,
                name: m.name().unwrap_or_else(|_| format!("Display {i}")),
                width: m.width().unwrap_or(0),
                height: m.height().unwrap_or(0),
                is_primary: m.is_primary().unwrap_or(false),
            })
            .collect(),
        Err(e) => {
            log::error!("list_monitors failed: {e}");
            Vec::new()
        }
    }
}

/// Capture one monitor by index, downscaled and JPEG-encoded.
pub fn capture_monitor(index: u32, max_dim: u32) -> Result<VisionFrame, String> {
    let monitors = xcap::Monitor::all().map_err(|e| e.to_string())?;
    let monitor = monitors
        .get(index as usize)
        .ok_or_else(|| format!("vision: monitor {index} not found"))?;
    let name = monitor.name().unwrap_or_else(|_| "display".into());
    let image = monitor.capture_image().map_err(|e| e.to_string())?;

    let (w, h) = scaled_dimensions(image.width(), image.height(), max_dim);
    let resized = image::imageops::resize(&image, w, h, image::imageops::FilterType::Triangle);

    let mut buf = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, JPEG_QUALITY);
    encoder
        .encode(resized.as_raw(), w, h, image::ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;

    Ok(VisionFrame {
        monitor: name,
        width: w,
        height: h,
        jpeg_base64: base64::engine::general_purpose::STANDARD.encode(buf.into_inner()),
    })
}

pub struct VisionHandle {
    stop: Arc<AtomicBool>,
}

impl VisionHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

/// Start periodic capture of `monitor_index` every `interval_ms`, emitting each frame.
pub fn spawn(
    monitor_index: u32,
    interval_ms: u64,
    max_dim: u32,
    emit: impl Fn(VisionFrame) + Send + Sync + 'static,
) -> VisionHandle {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_task = stop.clone();
    tokio::spawn(async move {
        while !stop_task.load(Ordering::Relaxed) {
            // Capture is blocking (talks to the window server) — keep it off the async
            // executor so it can't stall other tasks.
            match tokio::task::spawn_blocking(move || capture_monitor(monitor_index, max_dim)).await {
                Ok(Ok(frame)) => emit(frame),
                Ok(Err(e)) => log::warn!("vision capture failed: {e}"),
                Err(e) => log::warn!("vision task join failed: {e}"),
            }
            tokio::time::sleep(Duration::from_millis(interval_ms.max(500))).await;
        }
    });
    VisionHandle { stop }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_images_are_not_upscaled() {
        assert_eq!(scaled_dimensions(800, 600, 1280), (800, 600));
    }

    #[test]
    fn large_images_scale_to_the_longest_side() {
        // 5120x2880 → longest side 1280.
        let (w, h) = scaled_dimensions(5120, 2880, 1280);
        assert_eq!(w, 1280);
        assert_eq!(h, 720);
    }

    #[test]
    fn portrait_scales_on_height() {
        let (w, h) = scaled_dimensions(1000, 4000, 1000);
        assert_eq!(h, 1000);
        assert_eq!(w, 250);
    }

    #[test]
    fn never_returns_zero() {
        let (w, h) = scaled_dimensions(1, 1, 1280);
        assert!(w >= 1 && h >= 1);
    }
}
