//! Converting Tauri windows into non-activating NSPanels and giving them the collection
//! behaviour + window level that let the HUD float over full-screen apps (spec §9).
//!
//! Three things are required; missing any one makes the HUD invisible in a way that
//! looks like a different bug:
//!   1. Collection behaviour — CLEAR Managed, don't OR around it. Managed/Transient/
//!      Stationary are mutually exclusive; OR-ing CanJoinAllSpaces on top of a Managed
//!      mask leaves a self-contradictory value and the Space manager keeps treating the
//!      window as single-Space.
//!   2. NSScreenSaverWindowLevel — NSFloatingWindowLevel only floats above ordinary
//!      windows in the active Space; a full-screened Chrome outranks it.
//!   3. A non-activating NSPanel — the NonactivatingPanel style mask is honoured only on
//!      an NSPanel, and Tauri only makes NSWindows. Without it, clicking the HUD
//!      activates the app and macOS switches Spaces, yanking the user out of the video.

// --- Bit values (pure; unit-tested so the mask logic is verified without AppKit) ---

pub const NS_CB_CAN_JOIN_ALL_SPACES: u64 = 1 << 0;
pub const NS_CB_MOVE_TO_ACTIVE_SPACE: u64 = 1 << 1;
pub const NS_CB_MANAGED: u64 = 1 << 2;
pub const NS_CB_TRANSIENT: u64 = 1 << 3;
pub const NS_CB_STATIONARY: u64 = 1 << 4;
pub const NS_CB_PARTICIPATES_IN_CYCLE: u64 = 1 << 5;
pub const NS_CB_IGNORES_CYCLE: u64 = 1 << 6;
pub const NS_CB_FULLSCREEN_PRIMARY: u64 = 1 << 7;
pub const NS_CB_FULLSCREEN_AUXILIARY: u64 = 1 << 8;
pub const NS_CB_FULLSCREEN_NONE: u64 = 1 << 9;

pub const NS_STYLE_MASK_NONACTIVATING_PANEL: u64 = 1 << 7;
/// The level that actually outranks a full-screen window.
pub const NS_SCREEN_SAVER_WINDOW_LEVEL: i64 = 1000;

/// Rewrite a collection-behaviour mask per the spec: clear the single-Space / managed /
/// cycle / fullscreen-primary bits, set the all-Spaces / stationary / fullscreen-aux /
/// ignores-cycle bits. Crucially this CLEARS rather than ORs, so no contradictory mask
/// survives.
pub fn adjust_collection_behavior(current: u64) -> u64 {
    let clear = NS_CB_MANAGED
        | NS_CB_TRANSIENT
        | NS_CB_MOVE_TO_ACTIVE_SPACE
        | NS_CB_FULLSCREEN_PRIMARY
        | NS_CB_FULLSCREEN_NONE
        | NS_CB_PARTICIPATES_IN_CYCLE;
    let set = NS_CB_CAN_JOIN_ALL_SPACES
        | NS_CB_STATIONARY
        | NS_CB_FULLSCREEN_AUXILIARY
        | NS_CB_IGNORES_CYCLE;
    (current & !clear) | set
}

#[cfg(all(target_os = "macos", feature = "appkit"))]
mod imp {
    use super::*;
    use objc2::rc::Retained;
    use objc2::runtime::{AnyClass, AnyObject, Bool, ClassBuilder, Sel};
    use objc2::{msg_send, sel};
    use once_cell::sync::OnceCell;

    extern "C" fn can_become_key(_this: &AnyObject, _cmd: Sel) -> Bool {
        // Losing this makes text fields silently swallow every keystroke.
        Bool::YES
    }
    extern "C" fn can_become_main(_this: &AnyObject, _cmd: Sel) -> Bool {
        Bool::NO
    }

    /// Register (once) an NSPanel subclass that can become key but not main.
    fn panel_subclass() -> &'static AnyClass {
        static CLASS: OnceCell<usize> = OnceCell::new();
        let ptr = CLASS.get_or_init(|| {
            let superclass = AnyClass::get("NSPanel").expect("NSPanel must exist");
            let mut builder = ClassBuilder::new("VerityNonactivatingPanel", superclass)
                .expect("failed to create VerityNonactivatingPanel");
            unsafe {
                builder.add_method(
                    sel!(canBecomeKeyWindow),
                    can_become_key as extern "C" fn(&AnyObject, Sel) -> Bool,
                );
                builder.add_method(
                    sel!(canBecomeMainWindow),
                    can_become_main as extern "C" fn(&AnyObject, Sel) -> Bool,
                );
            }
            builder.register() as *const AnyClass as usize
        });
        unsafe { &*(*ptr as *const AnyClass) }
    }

    /// Convert a live NSWindow into our non-activating panel and apply the collection
    /// behaviour + level. Idempotent: re-classing a live window that AppKit is mid-flight
    /// ordering in throws an Objective-C exception that unwinds through Rust and aborts
    /// the process — so we convert EXACTLY ONCE, guarded by the current class.
    pub fn convert(ns_window: *mut AnyObject) {
        if ns_window.is_null() {
            return;
        }
        unsafe {
            let obj = &*ns_window;
            let target = panel_subclass();

            let current_class: *const AnyClass = msg_send![obj, class];
            let already: Bool = msg_send![obj, isKindOfClass: target];
            if !already.as_bool() && current_class != (target as *const AnyClass) {
                use objc2::runtime::AnyObject as O;
                // object_setClass onto our NSPanel subclass.
                let _: *const AnyClass = {
                    extern "C" {
                        fn object_setClass(obj: *mut O, cls: *const AnyClass) -> *const AnyClass;
                    }
                    object_setClass(ns_window, target)
                };
            }

            // Style mask: OR in the non-activating panel bit.
            let mask: u64 = msg_send![obj, styleMask];
            let _: () = msg_send![obj, setStyleMask: mask | NS_STYLE_MASK_NONACTIVATING_PANEL];

            let _: () = msg_send![obj, setFloatingPanel: Bool::YES];
            let _: () = msg_send![obj, setBecomesKeyOnlyIfNeeded: Bool::YES];
            let _: () = msg_send![obj, setHidesOnDeactivate: Bool::NO];

            // Collection behaviour — clear-then-set (never a contradictory OR).
            let current: u64 = msg_send![obj, collectionBehavior];
            let _: () = msg_send![obj, setCollectionBehavior: adjust_collection_behavior(current)];

            // Level LAST — setAlwaysOnTop (called earlier by Tauri) resets it, so the
            // screen-saver level must be applied after.
            let _: () = msg_send![obj, setLevel: NS_SCREEN_SAVER_WINDOW_LEVEL];

            let _ = Retained::<O>::retain(ns_window); // keep a ref alive across the call
        }
    }
}

/// Convert a Tauri window to a floating non-activating panel. No-op without AppKit.
#[cfg(all(target_os = "macos", feature = "appkit"))]
pub fn make_panel(window: &tauri::WebviewWindow) {
    if let Ok(ptr) = window.ns_window() {
        imp::convert(ptr as *mut objc2::runtime::AnyObject);
    }
}

#[cfg(not(all(target_os = "macos", feature = "appkit")))]
pub fn make_panel(_window: &tauri::WebviewWindow) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clears_managed_and_sets_all_spaces() {
        // Every window is born Managed.
        let born = NS_CB_MANAGED;
        let out = adjust_collection_behavior(born);
        assert_eq!(out & NS_CB_MANAGED, 0, "Managed must be cleared, not OR'd around");
        assert_ne!(out & NS_CB_CAN_JOIN_ALL_SPACES, 0);
        assert_ne!(out & NS_CB_STATIONARY, 0);
        assert_ne!(out & NS_CB_FULLSCREEN_AUXILIARY, 0);
        assert_ne!(out & NS_CB_IGNORES_CYCLE, 0);
    }

    #[test]
    fn clears_the_full_set_of_wrong_bits() {
        let messy = NS_CB_MANAGED
            | NS_CB_TRANSIENT
            | NS_CB_MOVE_TO_ACTIVE_SPACE
            | NS_CB_FULLSCREEN_PRIMARY
            | NS_CB_FULLSCREEN_NONE
            | NS_CB_PARTICIPATES_IN_CYCLE;
        let out = adjust_collection_behavior(messy);
        for bit in [
            NS_CB_MANAGED,
            NS_CB_TRANSIENT,
            NS_CB_MOVE_TO_ACTIVE_SPACE,
            NS_CB_FULLSCREEN_PRIMARY,
            NS_CB_FULLSCREEN_NONE,
            NS_CB_PARTICIPATES_IN_CYCLE,
        ] {
            assert_eq!(out & bit, 0);
        }
    }

    #[test]
    fn result_is_not_self_contradictory() {
        let out = adjust_collection_behavior(NS_CB_MANAGED);
        // Managed and CanJoinAllSpaces must never both be set.
        assert!(!((out & NS_CB_MANAGED != 0) && (out & NS_CB_CAN_JOIN_ALL_SPACES != 0)));
    }

    #[test]
    fn screen_saver_level_outranks_floating() {
        // Floating is 3; screen-saver is 1000 — must be far above.
        assert!(NS_SCREEN_SAVER_WINDOW_LEVEL > 3);
    }
}
