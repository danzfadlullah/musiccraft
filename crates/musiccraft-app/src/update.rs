//! In-app updates: checks the rolling `android` pre-release on the GitHub
//! Releases tab, downloads the APK with a progress bar, and hands it to the
//! system installer (`musiccraft-android-installer`). Only the Android build
//! talks to the network; elsewhere this is a no-op stub so the settings
//! window can show the tab without the feature.

use std::sync::{Arc, Mutex};
#[cfg(target_os = "android")]
use std::thread;

use eframe::egui;

use musiccraft_ui::{UpdateInfo, UpdateStatus};

#[cfg(target_os = "android")]
const RELEASE_API: &str =
    "https://api.github.com/repos/danzfadlullah/musiccraft/releases/tags/android";
#[cfg(target_os = "android")]
const APK_NAME: &str = "MusicCraft-android.apk";
#[cfg(target_os = "android")]
const USER_AGENT: &str = "MusicCraft-Android";

/// The commit this APK was made from; CI stamps it into every build. Empty
/// on a plain `cargo apk build`, which then cannot tell old from new.
pub fn build_sha() -> &'static str {
    option_env!("MUSICCRAFT_BUILD_SHA").unwrap_or("")
}

/// The newest build on the Releases tab.
#[cfg(target_os = "android")]
struct Latest {
    sha: String,
    published: String,
    url: String,
    size: u64,
}

pub struct Updater {
    status: Arc<Mutex<UpdateStatus>>,
    /// A finished download, waiting for the UI thread (the activity's Java
    /// thread) to hand it to the installer — JNI belongs there.
    ready: Arc<Mutex<Option<Vec<u8>>>>,
    #[cfg_attr(not(target_os = "android"), allow(dead_code))]
    ctx: egui::Context,
}

impl Updater {
    pub fn new(ctx: egui::Context) -> Self {
        Self {
            status: Arc::new(Mutex::new(UpdateStatus::Idle)),
            ready: Arc::new(Mutex::new(None)),
            ctx,
        }
    }

    /// What the settings window shows right now.
    pub fn info(&self) -> UpdateInfo {
        UpdateInfo {
            build: build_sha().to_owned(),
            status: self.status.lock().unwrap().clone(),
        }
    }

    /// Starts a background check against the rolling pre-release.
    pub fn check(&self) {
        #[cfg(target_os = "android")]
        {
            *self.status.lock().unwrap() = UpdateStatus::Checking;
            let status = Arc::clone(&self.status);
            let ctx = self.ctx.clone();
            thread::spawn(move || {
                let next = match latest_build() {
                    Ok(latest) => match build_sha() {
                        "" => UpdateStatus::Available {
                            build: short(&latest.sha),
                            published: latest.published,
                            size: latest.size,
                        },
                        sha if latest.sha.starts_with(sha) => UpdateStatus::UpToDate,
                        _ => UpdateStatus::Available {
                            build: short(&latest.sha),
                            published: latest.published,
                            size: latest.size,
                        },
                    },
                    Err(err) => UpdateStatus::Failed(err),
                };
                crate::log(&format!("update check: {:?}", next));
                *status.lock().unwrap() = next;
                ctx.request_repaint();
            });
        }
    }

    /// Starts the background download of the newest APK; `poll` hands the
    /// result to the installer once it is in.
    pub fn download(&self) {
        #[cfg(target_os = "android")]
        {
            *self.status.lock().unwrap() = UpdateStatus::Downloading { progress: 0.0 };
            let status = Arc::clone(&self.status);
            let ready = Arc::clone(&self.ready);
            let ctx = self.ctx.clone();
            thread::spawn(move || {
                match download_latest(&status, &ctx) {
                    Ok(bytes) => {
                        let note = save_fallback(&bytes);
                        crate::log(&format!("update downloaded: {} bytes; {note}", bytes.len()));
                        *ready.lock().unwrap() = Some(bytes);
                        *status.lock().unwrap() = UpdateStatus::Installing;
                    }
                    Err(err) => {
                        crate::log(&format!("update download failed: {err}"));
                        *status.lock().unwrap() = UpdateStatus::Failed(err);
                    }
                }
                ctx.request_repaint();
            });
        }
    }

    /// Runs on the UI thread each frame: hands a finished download to the
    /// system installer and keeps the settings window's info current.
    pub fn poll(&mut self) {
        #[cfg(target_os = "android")]
        if let Some(bytes) = self.ready.lock().unwrap().take() {
            let installer = crate::android::app_handle()
                .and_then(|app| musiccraft_android_installer::install_apk(&app, &bytes).ok());
            let note = match installer {
                Some(()) => {
                    crate::log("update: the system installer is open");
                    String::new()
                }
                None => {
                    crate::log("update: the installer did not open");
                    match fallback_path() {
                        Some(path) => format!(
                            "The installer did not open; the APK is at {} — \
                             open it with a file manager to install.",
                            path.display()
                        ),
                        None => "The installer did not open and no copy of the APK \
                             could be kept; check for the update again."
                            .to_owned(),
                    }
                }
            };
            *self.status.lock().unwrap() = UpdateStatus::Ready { note };
            self.ctx.request_repaint();
        }
        #[cfg(not(target_os = "android"))]
        let _ = self.ready.lock().unwrap().take();
    }
}

/// The short commit shown in the panel.
#[cfg(target_os = "android")]
fn short(sha: &str) -> String {
    sha.chars().take(7).collect()
}

#[cfg(target_os = "android")]
fn latest_build() -> Result<Latest, String> {
    let reply = ureq::get(RELEASE_API)
        .set("User-Agent", USER_AGENT)
        .set("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| format!("cannot reach GitHub: {e}"))?;
    let text = reply
        .into_string()
        .map_err(|e| format!("the release reply is not text: {e}"))?;
    let json: serde_json::Value =
        serde_json::from_str(&text).map_err(|e| format!("the release reply is not JSON: {e}"))?;
    let sha = json
        .get("target_commitish")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let published = json
        .get("published_at")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_owned();
    let asset = json
        .get("assets")
        .and_then(|v| v.as_array())
        .and_then(|assets| {
            assets
                .iter()
                .find(|a| a.get("name").and_then(|v| v.as_str()) == Some(APK_NAME))
                .cloned()
        })
        .ok_or_else(|| format!("{APK_NAME} is not on the release yet"))?;
    Ok(Latest {
        sha,
        published,
        url: asset
            .get("browser_download_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
        size: asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
    })
}

#[cfg(target_os = "android")]
fn download_latest(status: &Mutex<UpdateStatus>, ctx: &egui::Context) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let latest = latest_build()?;
    let reply = ureq::get(&latest.url)
        .set("User-Agent", USER_AGENT)
        .call()
        .map_err(|e| format!("cannot reach GitHub: {e}"))?;
    let total = reply
        .header("content-length")
        .and_then(|v| v.parse::<u64>().ok())
        .filter(|&n| n > 0)
        .unwrap_or(latest.size)
        .max(1);
    let mut reader = reply.into_reader();
    let mut bytes = Vec::with_capacity(total as usize);
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = reader
            .read(&mut chunk)
            .map_err(|e| format!("download interrupted: {e}"))?;
        if read == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..read]);
        *status.lock().unwrap() = UpdateStatus::Downloading {
            progress: (bytes.len() as f32 / total as f32).min(1.0),
        };
        ctx.request_repaint();
    }
    Ok(bytes)
}

/// A spare copy of the APK inside the app's own storage, in case the
/// installer could not be opened and the user has to go through a file
/// manager after all.
#[cfg(target_os = "android")]
fn save_fallback(bytes: &[u8]) -> String {
    if let Some(path) = fallback_path() {
        return match std::fs::write(&path, bytes) {
            Ok(()) => format!("A copy is at {}", path.display()),
            Err(err) => format!("(a copy could not be kept: {err})"),
        };
    }
    String::new()
}

#[cfg(target_os = "android")]
fn fallback_path() -> Option<std::path::PathBuf> {
    crate::android::external_dir().map(|dir| {
        let path = dir.join("update").join(APK_NAME);
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        path
    })
}
