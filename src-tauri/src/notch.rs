//! Notch + menu-bar measurement and the dock island geometry (spec §9).
//!
//! NSScreen is main-thread-only, so we measure once at startup and cache. The geometry
//! math is pure and unit-tested; the AppKit read is feature-gated.

use serde::Serialize;

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct NotchGeometry {
    pub has_notch: bool,
    /// On a notched Mac, safeAreaInsets.top IS the menu-bar height.
    pub menu_bar_height: f64,
    pub notch_width: f64,
    pub screen_width: f64,
}

/// Compute the notch width from NSScreen readings, with the guard the spec demands.
///
/// auxiliaryTopLeftArea / auxiliaryTopRightArea bracket the cutout — but they return
/// NSZeroRect on a notchless screen, which would compute the ENTIRE screen width as the
/// notch. So we only trust them once safeAreaInsets.top has confirmed a notch, and we
/// reject any width exceeding half the screen.
pub fn compute_notch(
    safe_area_top: f64,
    aux_left_max_x: f64,
    aux_right_min_x: f64,
    screen_width: f64,
) -> NotchGeometry {
    let has_notch = safe_area_top > 0.0;
    let mut notch_width = 0.0;
    if has_notch {
        let w = aux_right_min_x - aux_left_max_x;
        if w > 0.0 && w <= screen_width / 2.0 {
            notch_width = w;
        }
    }
    NotchGeometry {
        has_notch,
        menu_bar_height: if has_notch { safe_area_top } else { 24.0 },
        notch_width,
        screen_width,
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct IslandRect {
    pub width: f64,
    pub height: f64,
    pub center_x: f64,
    /// True when the island straddles a hardware cutout (reads as the notch grown).
    pub straddles_notch: bool,
}

/// The dock island. On a notched Mac it straddles the cutout (cutout width + ~18pt each
/// side, exactly menu-bar tall) so it reads as the notch having grown. Notchless screens
/// get a centred ~168×26pt pill.
pub const ISLAND_SIDE_PAD: f64 = 18.0;
pub const PILL_WIDTH: f64 = 168.0;
pub const PILL_HEIGHT: f64 = 26.0;

pub fn compute_island(g: &NotchGeometry) -> IslandRect {
    if g.has_notch && g.notch_width > 0.0 {
        IslandRect {
            width: g.notch_width + ISLAND_SIDE_PAD * 2.0,
            height: g.menu_bar_height.max(1.0),
            center_x: g.screen_width / 2.0, // the camera cutout is horizontally centred
            straddles_notch: true,
        }
    } else {
        IslandRect {
            width: PILL_WIDTH,
            height: PILL_HEIGHT,
            center_x: g.screen_width / 2.0,
            straddles_notch: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq)]
pub struct RevealBand {
    pub x: f64,
    pub width: f64,
    pub height: f64,
}

/// The hover band that reveals the island: the island plus ~44pt either side (wider than
/// what's visible, or the target is fussy), from the screen top to just past the island.
/// NOT the full window width — that would fire every time the user reaches for a menu.
pub const REVEAL_SIDE_PAD: f64 = 44.0;

pub fn compute_reveal_band(island: &IslandRect) -> RevealBand {
    let width = island.width + REVEAL_SIDE_PAD * 2.0;
    RevealBand {
        x: island.center_x - width / 2.0,
        width,
        height: island.height + 8.0, // just past the island
    }
}

/// Measure the main screen via NSScreen (main thread only). Cache the result.
#[cfg(all(target_os = "macos", feature = "appkit"))]
pub fn measure() -> NotchGeometry {
    use objc2_app_kit::NSScreen;
    use objc2_foundation::MainThreadMarker;

    let Some(mtm) = MainThreadMarker::new() else {
        // Not on the main thread — return a safe notchless default.
        return compute_notch(0.0, 0.0, 0.0, 0.0);
    };
    let Some(screen) = NSScreen::mainScreen(mtm) else {
        return compute_notch(0.0, 0.0, 0.0, 0.0);
    };
    // These NSScreen reads are `unsafe` in objc2 (they touch AppKit state); they're safe
    // here because we hold a MainThreadMarker and a live main screen.
    let (screen_width, safe_top, aux_left_max_x, aux_right_min_x) = unsafe {
        let frame = screen.frame();
        let screen_width = frame.size.width;
        let safe_top = screen.safeAreaInsets().top;
        // auxiliary areas only make sense once a notch is confirmed.
        let (l, r) = if safe_top > 0.0 {
            let left = screen.auxiliaryTopLeftArea();
            let right = screen.auxiliaryTopRightArea();
            (left.origin.x + left.size.width, right.origin.x)
        } else {
            (0.0, 0.0)
        };
        (screen_width, safe_top, l, r)
    };

    compute_notch(safe_top, aux_left_max_x, aux_right_min_x, screen_width)
}

#[cfg(not(all(target_os = "macos", feature = "appkit")))]
pub fn measure() -> NotchGeometry {
    // Off-platform / feature off: assume a notchless screen with a standard menu bar.
    compute_notch(0.0, 0.0, 0.0, 1440.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notchless_screen_reports_no_notch() {
        let g = compute_notch(0.0, 0.0, 0.0, 1440.0);
        assert!(!g.has_notch);
        assert_eq!(g.notch_width, 0.0);
        assert_eq!(g.menu_bar_height, 24.0); // standard menu bar
    }

    #[test]
    fn notched_screen_measures_the_cutout() {
        // 1512-wide MBP: left area 0..700, right area 812..1512 → notch 812-700 = 112.
        let g = compute_notch(37.0, 700.0, 812.0, 1512.0);
        assert!(g.has_notch);
        assert_eq!(g.notch_width, 112.0);
        assert_eq!(g.menu_bar_height, 37.0);
    }

    #[test]
    fn guard_rejects_a_width_over_half_the_screen() {
        // A bogus reading that would compute most of the screen as notch is rejected.
        let g = compute_notch(37.0, 0.0, 1400.0, 1512.0);
        assert_eq!(g.notch_width, 0.0);
    }

    #[test]
    fn guard_ignores_aux_areas_without_a_confirmed_notch() {
        // safe-area inset is 0 → notchless → aux readings (even if nonzero) are ignored.
        let g = compute_notch(0.0, 700.0, 812.0, 1512.0);
        assert!(!g.has_notch);
        assert_eq!(g.notch_width, 0.0);
    }

    #[test]
    fn island_straddles_a_real_notch() {
        let g = compute_notch(37.0, 700.0, 812.0, 1512.0);
        let island = compute_island(&g);
        assert!(island.straddles_notch);
        assert_eq!(island.width, 112.0 + 36.0);
        assert_eq!(island.height, 37.0);
        assert_eq!(island.center_x, 756.0);
    }

    #[test]
    fn island_is_a_centred_pill_on_a_notchless_screen() {
        let g = compute_notch(0.0, 0.0, 0.0, 1440.0);
        let island = compute_island(&g);
        assert!(!island.straddles_notch);
        assert_eq!(island.width, PILL_WIDTH);
        assert_eq!(island.height, PILL_HEIGHT);
        assert_eq!(island.center_x, 720.0);
    }

    #[test]
    fn reveal_band_is_wider_than_the_island_but_not_the_screen() {
        let g = compute_notch(0.0, 0.0, 0.0, 1440.0);
        let island = compute_island(&g);
        let band = compute_reveal_band(&island);
        assert_eq!(band.width, PILL_WIDTH + 88.0);
        assert!(band.width < g.screen_width); // never the whole width
        assert_eq!(band.x, island.center_x - band.width / 2.0);
    }
}
