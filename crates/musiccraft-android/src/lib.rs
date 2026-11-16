//! MusicCraft for Android: the NativeActivity entry point. The whole
//! application lives in `musiccraft-app`; this crate only hands the Android
//! app handle over and lets eframe own the activity's surface.
//!
//! `android_main` is the one place the workspace's `unsafe_code = "forbid"`
//! rule bends: edition 2024 requires `#[unsafe(no_mangle)]` for the symbol
//! android-activity's glue looks up. This crate contains nothing else.

#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
fn android_main(android_app: android_activity::AndroidApp) {
    let _ = musiccraft_app::run_android(android_app);
}
