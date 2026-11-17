//! In-app updates: checks the rolling `android` pre-release on the GitHub
//! Releases tab, downloads the APK with a progress bar, and hands it to the
//! system installer (`musiccraft-android-java`). Only the Android build
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
/// The update APKs' name prefix in storage, completed with the version
/// (`MusicCraft-update-0.6.0.apk`), so the cleanup knows which files are
/// spent (`install.rs` uses the same naming for its Downloads copy).
#[cfg(target_os = "android")]
const APK_PREFIX: &str = "MusicCraft-update-";
#[cfg(target_os = "android")]
const USER_AGENT: &str = "MusicCraft-Android";

/// The commit this APK was made from; CI stamps it into every build. Empty
/// on a plain `cargo apk build`.
pub fn build_sha() -> &'static str {
    option_env!("MUSICCRAFT_BUILD_SHA").unwrap_or("")
}

/// The app's semantic version: the workspace version every crate shares, so
/// the settings panel shows something readable instead of a commit.
pub fn build_version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// The newest build on the Releases tab.
#[cfg(target_os = "android")]
struct Latest {
    /// The release's semantic version, from its notes' `Version:` line;
    /// empty when the notes predate stamping.
    version: String,
    /// The release notes' changelog: the commit subjects since the previous
    /// release, from the notes' `Changes:` section.
    changes: Vec<String>,
    sha: String,
    published: String,
    url: String,
    size: u64,
    /// The release asset's SHA-256 fingerprint (`sha256:…`), which the
    /// download must match before it may install; empty when GitHub does
    /// not report one.
    digest: String,
}

/// A finished download and the version it is, waiting for the UI thread (the
/// activity's Java thread) to hand it to the installer — JNI belongs there.
type ReadyDownload = Arc<Mutex<Option<(Vec<u8>, String)>>>;

pub struct Updater {
    status: Arc<Mutex<UpdateStatus>>,
    ready: ReadyDownload,
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
            version: build_version().to_owned(),
            commit: build_sha().to_owned(),
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
                    Ok(latest) => {
                        let current_sha = build_sha();
                        let same_build =
                            !current_sha.is_empty() && latest.sha.starts_with(current_sha);
                        let newer_release = newer(&latest.version, build_version());
                        let older_release = !latest.version.is_empty()
                            && latest.version != build_version()
                            && !newer_release;
                        // The release is this exact build, or an older
                        // version than the one installed: nothing to do.
                        // A newer version, the same version rebuilt (the
                        // rolling release after every push), or a release
                        // without a version stamp: offer the update.
                        if same_build || older_release {
                            UpdateStatus::UpToDate
                        } else {
                            UpdateStatus::Available {
                                version: latest.version,
                                changes: latest.changes,
                                build: short(&latest.sha),
                                published: latest.published,
                                size: latest.size,
                            }
                        }
                    }
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
                    Ok((bytes, version)) => {
                        let note = save_fallback(&bytes, &version);
                        crate::log(&format!("update downloaded: {} bytes; {note}", bytes.len()));
                        *ready.lock().unwrap() = Some((bytes, version));
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
        if let Some((bytes, version)) = self.ready.lock().unwrap().take() {
            let installer = crate::android::app_handle().and_then(|app| {
                // Each step logs: the log file survives a crash, so it names
                // the step that took the app down.
                crate::log("update: handing the APK to the system installer");
                match musiccraft_android_java::install_apk(&app, &bytes, &version) {
                    Ok(()) => {
                        crate::log("update: the system installer is open");
                        Some(())
                    }
                    Err(err) => {
                        crate::log(&format!("update: the installer did not open: {err}"));
                        None
                    }
                }
            });
            let note = match installer {
                Some(()) => String::new(),
                None => match fallback_path(&version) {
                    Some(path) => format!(
                        "The installer did not open; the APK is at {} — \
                         open it with a file manager to install.",
                        path.display()
                    ),
                    None => "The installer did not open and no copy of the APK \
                         could be kept; check for the update again."
                        .to_owned(),
                },
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

/// Whether semantic version `a` is greater than `b` (numeric triples;
/// anything unparsable counts as 0).
#[cfg(target_os = "android")]
fn newer(a: &str, b: &str) -> bool {
    for index in 0..3 {
        let left = version_number(a, index);
        let right = version_number(b, index);
        if left != right {
            return left > right;
        }
    }
    false
}

/// One component of a semantic version, or 0 past the end.
#[cfg(target_os = "android")]
fn version_number(version: &str, index: usize) -> u64 {
    version
        .split('.')
        .nth(index)
        .and_then(|part| part.trim().parse::<u64>().ok())
        .unwrap_or(0)
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
    let body = json
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default();
    let version = body
        .lines()
        .find(|line| line.starts_with("Version: "))
        .map(|line| {
            line["Version: ".len()..]
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_owned()
        })
        .unwrap_or_default();
    // The changelog is the notes' last section: every bullet after the
    // `Changes:` heading.
    let changes = body
        .lines()
        .skip_while(|line| *line != "Changes:")
        .skip(1)
        .filter_map(|line| line.strip_prefix("- "))
        .map(str::to_owned)
        .collect();
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
        version,
        changes,
        sha,
        published,
        url: asset
            .get("browser_download_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
        size: asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0),
        digest: asset
            .get("digest")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_owned(),
    })
}

#[cfg(target_os = "android")]
fn download_latest(
    status: &Mutex<UpdateStatus>,
    ctx: &egui::Context,
) -> Result<(Vec<u8>, String), String> {
    use std::io::Read;

    let latest = latest_build()?;
    let version = latest.version.clone();
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
    crate::log(&format!("update: downloading {} bytes", total));
    let mut reader = reply.into_reader();
    let mut bytes = Vec::with_capacity(total as usize);
    let mut chunk = [0u8; 64 * 1024];
    let mut quarter = 0u8;
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
        // A milestone line per quarter, so the log shows life without spam.
        let reached = (bytes.len() as f32 / total as f32 * 4.0).min(4.0) as u8;
        if reached > quarter {
            quarter = reached;
            crate::log(&format!("update: downloaded {}%", reached * 25));
        }
        ctx.request_repaint();
    }
    // The download only installs when its fingerprint matches the one the
    // release asset carries: a truncated or tampered APK never gets there.
    let digest = apk_fingerprint(&bytes);
    if latest.digest.is_empty() {
        crate::log("update: the release carries no fingerprint; skipped the check");
    } else if same_digest(&latest.digest, &digest) {
        crate::log(&format!("update: fingerprint matched ({digest})"));
    } else {
        return Err(format!(
            "the downloaded APK's fingerprint ({digest}) does not match the release's ({}); \
             try the update again",
            latest.digest
        ));
    }
    Ok((bytes, version))
}

/// The SHA-256 fingerprint of `bytes`, lowercase hex.
#[cfg(target_os = "android")]
fn apk_fingerprint(bytes: &[u8]) -> String {
    hex(ring::digest::digest(&ring::digest::SHA256, bytes).as_ref())
}

#[cfg(target_os = "android")]
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// Whether a release asset's fingerprint (`sha256:…`, GitHub's case) is the
/// download's.
#[cfg(target_os = "android")]
fn same_digest(expected: &str, actual: &str) -> bool {
    expected
        .rsplit(':')
        .next()
        .is_some_and(|value| value.eq_ignore_ascii_case(actual))
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "android")]
    #[test]
    fn a_fingerprint_matches_its_own_hex_regardless_of_case_and_prefix() {
        assert!(super::same_digest(
            "sha256:ABCDEF0123456789",
            "abcdef0123456789"
        ));
        assert!(super::same_digest("abcdef0123456789", "abcdef0123456789"));
        assert!(!super::same_digest("sha256:abcdef", "000000"));
    }

    #[cfg(target_os = "android")]
    #[test]
    fn the_fingerprint_is_sha256_in_lowercase_hex() {
        assert_eq!(
            super::apk_fingerprint(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }
}

/// A spare copy of the APK inside the app's own storage, in case the
/// installer could not be opened and the user has to go through a file
/// manager after all.
#[cfg(target_os = "android")]
fn save_fallback(bytes: &[u8], version: &str) -> String {
    if let Some(path) = fallback_path(version) {
        return match std::fs::write(&path, bytes) {
            Ok(()) => format!("A copy is at {}", path.display()),
            Err(err) => format!("(a copy could not be kept: {err})"),
        };
    }
    String::new()
}

#[cfg(target_os = "android")]
fn fallback_path(version: &str) -> Option<std::path::PathBuf> {
    crate::android::external_dir().map(|dir| {
        let path = dir
            .join("update")
            .join(format!("{APK_PREFIX}{version}.apk"));
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        path
    })
}

/// Deletes the update APKs an installed update left behind: the Downloads
/// copy the installer read and the spare copy in the app's storage. An APK
/// is spent when the version in its name is not newer than the one running,
/// so a download whose install was cancelled survives to be opened again.
/// Runs once at start, when the setting asks for it.
#[cfg(target_os = "android")]
pub fn cleanup_apks() {
    let current = build_version().to_owned();
    let Some(app) = crate::android::app_handle() else {
        return;
    };
    match musiccraft_android_java::downloaded_apk_names(&app) {
        Ok(names) => {
            for name in names {
                let Some(version) = apk_version(&name) else {
                    continue;
                };
                if newer(&version, &current) {
                    continue;
                }
                match musiccraft_android_java::delete_downloaded_apk(&app, &name) {
                    Ok(()) => crate::log(&format!("update: deleted the installed {name}")),
                    Err(err) => crate::log(&format!("update: cannot delete {name}: {err}")),
                }
            }
        }
        Err(err) => crate::log(&format!("update: cannot list the downloaded APKs: {err}")),
    }
    let Some(dir) = crate::android::external_dir().map(|dir| dir.join("update")) else {
        return;
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(version) = apk_version(&name) else {
            continue;
        };
        if newer(&version, &current) {
            continue;
        }
        match std::fs::remove_file(entry.path()) {
            Ok(()) => crate::log(&format!("update: deleted the installed {name}")),
            Err(err) => crate::log(&format!("update: cannot delete {name}: {err}")),
        }
    }
}

/// The version in an update APK's name (`MusicCraft-update-0.6.0.apk`).
#[cfg(target_os = "android")]
fn apk_version(name: &str) -> Option<String> {
    name.strip_prefix(APK_PREFIX)?
        .strip_suffix(".apk")
        .map(str::to_owned)
}
