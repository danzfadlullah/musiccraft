//! MusicCraft desktop application entry point. Everything lives in the
//! library, which the Android entry point (`musiccraft-android`) shares.

// Release builds on Windows are GUI-subsystem so launching the app does not open a
// console window. (Usage errors printed to stderr are therefore not visible there.)
#![cfg_attr(
    all(not(debug_assertions), target_os = "windows"),
    windows_subsystem = "windows"
)]

fn main() -> eframe::Result {
    musiccraft_app::run_desktop()
}
