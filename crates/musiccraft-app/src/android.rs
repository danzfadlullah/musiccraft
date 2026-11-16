//! Android glue (only built for `target_os = "android"`): keeps the
//! NativeActivity handle, points settings, autosave and takes at the app's
//! private storage where the desktop would use the user's config folder, and
//! writes a plain-text log the user can send in when something breaks.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

static APP: OnceLock<android_activity::AndroidApp> = OnceLock::new();
static LOG_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Called once from `run_android`, before anything reads folders or logs.
pub fn set_app(app: android_activity::AndroidApp) {
    let _ = APP.set(app);
}

/// The activity's private folder, standing in for the desktop config folder.
pub fn data_dir() -> Option<PathBuf> {
    APP.get().and_then(|app| app.internal_data_path())
}

/// The NativeActivity handle for eframe's event loop, after `set_app`.
pub fn app_handle() -> Option<android_activity::AndroidApp> {
    APP.get().cloned()
}

/// Folders the log is written to, best first: the app's folder on shared
/// storage (`Android/data/<package>/files/MusicCraft/logs`), then its private
/// storage, then a `MusicCraft` folder in Downloads. Downloads only works on
/// Android 10 or with all-files access; failures there are fine, one of the
/// other two always succeeds.
fn log_dirs() -> &'static [PathBuf] {
    LOG_DIRS.get_or_init(|| {
        let mut dirs = Vec::new();
        if let Some(app) = APP.get() {
            if let Some(dir) = app.external_data_path() {
                dirs.push(dir.join("MusicCraft").join("logs"));
            }
            if let Some(dir) = app.internal_data_path() {
                dirs.push(dir.join("MusicCraft").join("logs"));
            }
        }
        dirs.push(PathBuf::from("/storage/emulated/0/Download/MusicCraft"));
        dirs
    })
}

/// Installs the panic hook and logs the start-up banner, including where the
/// log itself lives.
pub fn init_logging() {
    std::panic::set_hook(Box::new(|info| {
        let message = format!("PANIC: {info}");
        eprintln!("MusicCraft {message}");
        log(&message);
    }));
    log(&format!(
        "MusicCraft starting (Android API {}); logging to {}",
        android_activity::AndroidApp::sdk_version(),
        log_dirs()
            .iter()
            .map(|d| d.join("musiccraft.log").display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
}

/// Appends `message` to `musiccraft.log` in every log folder. Opening the file
/// per line keeps the writer simple; the app logs only a few lines per run.
pub fn log(message: &str) {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or_default();
    let line = format!("[{millis}] {message}\n");
    for dir in log_dirs() {
        if fs::create_dir_all(dir).is_err() {
            continue;
        }
        if let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join("musiccraft.log"))
        {
            let _ = file.write_all(line.as_bytes());
        }
    }
}
