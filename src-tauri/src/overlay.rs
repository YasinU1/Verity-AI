//! The HUD overlay window (spec §9): show/hide, click-through hot-zone, multi-monitor.
//!
//! The HUD window is large but paints only a small region. If the whole window blocked
//! the cursor, its transparent area would swallow clicks meant for the app underneath —
//! so the window is click-through everywhere EXCEPT the painted region.
//!
//! The catch: while a window ignores cursor events the webview receives zero mouse
//! events, so DOM mouseenter can never fire. Rust must poll the global cursor (~120ms)
//! against the painted rectangle and toggle ignore-cursor-events itself. The webview's
//! only job is to report its painted size (via ResizeObserver → set_overlay_hot_zone).

use serde::{Deserialize, Serialize};

/// Poll interval for the cursor-vs-hot-zone check. Fast enough to feel responsive,
/// slow enough not to burn a core.
pub const CURSOR_POLL_MS: u64 = 120;

/// A rectangle in global (screen) coordinates, top-left origin.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Default)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub fn point_in_rect(px: f64, py: f64, r: &Rect) -> bool {
    px >= r.x && px <= r.x + r.width && py >= r.y && py <= r.y + r.height
}

/// Whether the overlay should IGNORE the cursor (be click-through) given the cursor
/// position and the painted hot zone. Click-through everywhere except the paint.
pub fn should_ignore_cursor(cursor_x: f64, cursor_y: f64, hot_zone: Option<&Rect>) -> bool {
    match hot_zone {
        Some(z) => !point_in_rect(cursor_x, cursor_y, z),
        None => true, // nothing painted yet → fully click-through
    }
}

/// Position the HUD on the right edge of a monitor, vertically near the top third so it
/// doesn't cover the centre of a video. Coordinates are top-left origin.
pub fn compute_overlay_position(
    monitor_x: f64,
    monitor_y: f64,
    monitor_w: f64,
    monitor_h: f64,
    win_w: f64,
    win_h: f64,
    margin: f64,
) -> (f64, f64) {
    let x = monitor_x + monitor_w - win_w - margin;
    // Top third, but never off the top; never past the bottom.
    let y = (monitor_y + monitor_h * 0.12).min(monitor_y + monitor_h - win_h - margin);
    (x.max(monitor_x), y.max(monitor_y))
}

/// Read the global cursor position in top-left screen coordinates. AppKit's
/// NSEvent.mouseLocation is bottom-left origin, so we flip against the main screen height.
#[cfg(all(target_os = "macos", feature = "appkit"))]
pub fn cursor_position() -> Option<(f64, f64)> {
    use objc2_app_kit::{NSEvent, NSScreen};
    use objc2_foundation::MainThreadMarker;
    let p = unsafe { NSEvent::mouseLocation() };
    let mtm = MainThreadMarker::new()?;
    let screen = NSScreen::mainScreen(mtm)?;
    let h = screen.frame().size.height;
    Some((p.x, h - p.y))
}

#[cfg(not(all(target_os = "macos", feature = "appkit")))]
pub fn cursor_position() -> Option<(f64, f64)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn zone() -> Rect {
        Rect { x: 1000.0, y: 50.0, width: 400.0, height: 300.0 }
    }

    #[test]
    fn point_inside_and_outside() {
        let z = zone();
        assert!(point_in_rect(1100.0, 100.0, &z));
        assert!(point_in_rect(1000.0, 50.0, &z)); // on the edge
        assert!(!point_in_rect(999.0, 100.0, &z));
        assert!(!point_in_rect(1100.0, 400.0, &z));
    }

    #[test]
    fn ignores_cursor_outside_the_paint() {
        let z = zone();
        // Over the paint → interactive (don't ignore).
        assert!(!should_ignore_cursor(1100.0, 100.0, Some(&z)));
        // Over the transparent area → click-through (ignore).
        assert!(should_ignore_cursor(10.0, 10.0, Some(&z)));
    }

    #[test]
    fn fully_click_through_before_anything_is_painted() {
        assert!(should_ignore_cursor(1100.0, 100.0, None));
    }

    #[test]
    fn overlay_sits_on_the_right_edge() {
        let (x, y) = compute_overlay_position(0.0, 0.0, 1920.0, 1080.0, 440.0, 760.0, 16.0);
        assert_eq!(x, 1920.0 - 440.0 - 16.0);
        assert!(y >= 0.0 && y + 760.0 <= 1080.0);
    }

    #[test]
    fn overlay_position_respects_a_second_monitor_origin() {
        // A monitor to the right, origin at x=1920.
        let (x, _) = compute_overlay_position(1920.0, 0.0, 1920.0, 1080.0, 440.0, 760.0, 16.0);
        assert_eq!(x, 1920.0 + 1920.0 - 440.0 - 16.0);
    }
}
