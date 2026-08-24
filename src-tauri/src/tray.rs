//! The top-centre auto-hiding dock panel (spec §9).
//!
//! The dashboard collapses into an island in the menu-bar strip and expands on hover.
//! Rust polls the cursor against a reveal band (computed from the notch geometry) and
//! toggles the reveal. The state machine is pure and unit-tested; the cursor read and
//! window moves are the platform side.

use crate::notch::{compute_island, compute_reveal_band, NotchGeometry, RevealBand};
use crate::overlay::{point_in_rect, Rect};

pub const DOCK_POLL_MS: u64 = 120;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DockState {
    Collapsed,
    Expanded,
}

/// Decide the next dock state from the cursor position and whether the dock is pinned.
///
/// Pinned always stays expanded. Otherwise: reveal when the cursor enters the band,
/// collapse when it leaves the (larger) expanded window bounds — using the window
/// bounds for collapse, not the band, gives hysteresis so the panel doesn't flicker at
/// the band's edge while the user interacts with it.
pub fn next_dock_state(
    current: DockState,
    pinned: bool,
    cursor: Option<(f64, f64)>,
    band: &RevealBand,
    expanded_bounds: &Rect,
) -> DockState {
    if pinned {
        return DockState::Expanded;
    }
    let Some((cx, cy)) = cursor else {
        return DockState::Collapsed;
    };
    let band_rect = Rect { x: band.x, y: 0.0, width: band.width, height: band.height };
    match current {
        DockState::Collapsed => {
            if point_in_rect(cx, cy, &band_rect) {
                DockState::Expanded
            } else {
                DockState::Collapsed
            }
        }
        DockState::Expanded => {
            // Stay expanded while the cursor is anywhere over the expanded panel OR still
            // in the reveal band.
            if point_in_rect(cx, cy, expanded_bounds) || point_in_rect(cx, cy, &band_rect) {
                DockState::Expanded
            } else {
                DockState::Collapsed
            }
        }
    }
}

/// The reveal band for the current screen, straddling the notch/island.
pub fn reveal_band_for(geometry: &NotchGeometry) -> RevealBand {
    compute_reveal_band(&compute_island(geometry))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notch::compute_notch;

    fn setup() -> (RevealBand, Rect) {
        let g = compute_notch(0.0, 0.0, 0.0, 1440.0); // notchless
        let band = reveal_band_for(&g);
        // Expanded dashboard window, centred, below the band.
        let expanded = Rect { x: 720.0 - 360.0, y: 0.0, width: 720.0, height: 140.0 };
        (band, expanded)
    }

    #[test]
    fn pinned_is_always_expanded() {
        let (band, exp) = setup();
        assert_eq!(
            next_dock_state(DockState::Collapsed, true, None, &band, &exp),
            DockState::Expanded
        );
    }

    #[test]
    fn hover_over_the_band_expands() {
        let (band, exp) = setup();
        let center = (band.x + band.width / 2.0, 5.0);
        assert_eq!(
            next_dock_state(DockState::Collapsed, false, Some(center), &band, &exp),
            DockState::Expanded
        );
    }

    #[test]
    fn cursor_away_from_the_band_stays_collapsed() {
        let (band, exp) = setup();
        assert_eq!(
            next_dock_state(DockState::Collapsed, false, Some((10.0, 500.0)), &band, &exp),
            DockState::Collapsed
        );
    }

    #[test]
    fn expanded_stays_while_cursor_is_over_the_panel() {
        let (band, exp) = setup();
        // Cursor down over the panel body (below the band) — should NOT collapse.
        let over_panel = (720.0, 100.0);
        assert_eq!(
            next_dock_state(DockState::Expanded, false, Some(over_panel), &band, &exp),
            DockState::Expanded
        );
    }

    #[test]
    fn expanded_collapses_when_cursor_leaves_entirely() {
        let (band, exp) = setup();
        assert_eq!(
            next_dock_state(DockState::Expanded, false, Some((10.0, 500.0)), &band, &exp),
            DockState::Collapsed
        );
    }

    #[test]
    fn no_cursor_collapses_when_unpinned() {
        let (band, exp) = setup();
        assert_eq!(
            next_dock_state(DockState::Expanded, false, None, &band, &exp),
            DockState::Collapsed
        );
    }
}
