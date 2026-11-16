//! Android glue (only built for `target_os = "android"`): keeps the
//! NativeActivity handle, points settings, autosave and takes at the app's
//! private storage where the desktop would use the user's config folder, and
//! writes a plain-text log the user can send in when something breaks.

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

static APP: OnceLock<android_activity::AndroidApp> = OnceLock::new();
static LOG_DIRS: OnceLock<Vec<PathBuf>> = OnceLock::new();
static LOGGING: AtomicBool = AtomicBool::new(true);

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

/// The app's folder on shared storage
/// (`Android/data/<package>/files/MusicCraft`), where the update APK keeps a
/// spare copy the user can reach with a file manager.
pub fn external_dir() -> Option<PathBuf> {
    APP.get()
        .and_then(|app| app.external_data_path())
        .map(|dir| dir.join("MusicCraft"))
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

/// Turns the log file on or off (the Settings toggle). Panics are logged
/// either way: a crash report must survive a switched-off log.
pub fn set_logging(enabled: bool) {
    LOGGING.store(enabled, Ordering::Relaxed);
}

/// Installs the panic hook and logs the start-up banner, including where the
/// log itself lives.
pub fn init_logging() {
    std::panic::set_hook(Box::new(|info| {
        let message = format!("PANIC: {info}");
        log_panic(&message);
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

/// Appends `message` to `musiccraft.log` in every log folder, and mirrors it
/// to logcat (native stderr lands there). Opening the file per line keeps
/// the writer simple.
pub fn log(message: &str) {
    if !LOGGING.load(Ordering::Relaxed) {
        return;
    }
    write_line(message);
}

/// Writes a line even when logging is switched off: a crash report must
/// survive a switched-off log.
pub fn log_panic(message: &str) {
    write_line(message);
}

fn write_line(message: &str) {
    eprintln!("MusicCraft: {message}");
    let line = format!("[{}] {message}\n", timestamp());
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

/// The current UTC time as `YYYY-MM-DD HH:MM:SS.mmm`, computed from the
/// Unix epoch with the civil-from-days algorithm (no time crate).
fn timestamp() -> String {
    let millis = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or_default();
    let days = millis.div_euclid(86_400_000);
    let mut rest = millis.rem_euclid(86_400_000);
    let (year, month, day) = {
        // Howard Hinnant's civil_from_days, shifted to the March era.
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z.rem_euclid(146_097);
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        (if m <= 2 { y + 1 } else { y }, m, d)
    };
    let hour = rest / 3_600_000;
    rest %= 3_600_000;
    let minute = rest / 60_000;
    rest %= 60_000;
    let second = rest / 1000;
    let milli = rest % 1000;
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{milli:03}")
}
