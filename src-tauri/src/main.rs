// Prevent an extra console window on Windows in release. macOS-only app, but harmless.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    verity_lib::run()
}
