//! Android glue (only built for `target_os = "android"`): keeps the
//! NativeActivity handle so settings, autosave and takes can find the app's
//! private storage, where the desktop would use the user's config folder.

use std::path::PathBuf;
use std::sync::OnceLock;

static APP: OnceLock<android_activity::AndroidApp> = OnceLock::new();

/// Called once from `run_android`, before anything reads folders.
pub fn set_app(app: android_activity::AndroidApp) {
    let _ = APP.set(app);
}

/// The activity's private folder, standing in for the desktop config folder.
pub fn data_dir() -> Option<PathBuf> {
    APP.get().and_then(|app| app.internal_data_path())
}
