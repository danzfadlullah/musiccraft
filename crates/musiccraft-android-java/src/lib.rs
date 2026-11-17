//! The pieces of MusicCraft on Android that must talk to Java, through JNI:
//!
//! - `install_apk` hands a downloaded APK to the system installer (the
//!   in-app update), and
//! - `pick` opens the system's SAF file picker and turns what it returns
//!   into cache files the app reads and writes like any other.
//!
//! The workspace forbids `unsafe` code; like `musiccraft-android`'s
//! `android_main`, this crate is a deliberate exception, because JNI cannot
//! be written safely. It holds nothing else and does not inherit the
//! workspace lints (docs/android.md, "Notes for contributors").

mod install;
mod saf;

use std::sync::OnceLock;

use android_activity::AndroidApp;
use jni::errors::Result as JniResult;
use jni::objects::{JObject, JString, JValueOwned};
use jni::{JNIEnv, JavaVM};

pub use install::{delete_downloaded_apk, downloaded_apk_names, install_apk};
pub use saf::{SafMode, SafRequest, pick, sync};

static APP: OnceLock<AndroidApp> = OnceLock::new();

/// Called once from `run_android`, before anything picks or installs.
pub fn set_app(app: AndroidApp) {
    let _ = APP.set(app);
}

pub(crate) fn app() -> Option<&'static AndroidApp> {
    APP.get()
}

/// Reports a failure to logcat (native stderr is mirrored there), so a
/// picker or installer problem leaves a trace in `musiccraft.log`'s console
/// companions without any caller having to thread errors around.
pub(crate) fn log(message: &str) {
    eprintln!("MusicCraft: {message}");
}

/// Attaches the calling thread to the JVM for one JNI session.
pub(crate) fn with_env<T>(
    f: impl FnOnce(&mut JNIEnv, &JObject) -> Result<T, String>,
) -> Result<T, String> {
    let app = app().ok_or_else(|| "the activity is not set up yet".to_owned())?;
    unsafe {
        let vm = JavaVM::from_raw(app.vm_as_ptr().cast())
            .map_err(|e| format!("cannot reach the JVM: {e}"))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("cannot attach to the JVM: {e}"))?;
        let activity = JObject::from_raw(app.activity_as_ptr().cast());
        f(&mut env, &activity)
    }
}

/// Maps a JNI error to a message and clears any pending Java exception, so a
/// failure cannot poison the next JNI call. A thrown exception's type and
/// message join the error, so the log names the real cause (for example
/// ActivityNotFoundException when the installer cannot be opened).
pub(crate) fn checked<T>(env: &mut JNIEnv, result: JniResult<T>) -> Result<T, String> {
    result.map_err(|e| {
        let detail = exception_message(env);
        format!("JNI: {e}{detail}")
    })
}

/// Reads and clears a pending Java exception, as " (Type: message)".
fn exception_message(env: &mut JNIEnv) -> String {
    let throwable = match env.exception_occurred() {
        Ok(throwable) => throwable,
        Err(_) => {
            let _ = env.exception_clear();
            return String::new();
        }
    };
    let _ = env.exception_clear();
    if throwable.as_raw().is_null() {
        return String::new();
    }
    let object = JObject::from(throwable);
    let text = env
        .call_method(&object, "toString", "()Ljava/lang/String;", &[])
        .and_then(|value| value.l());
    let text = match text {
        Ok(string) => {
            let string = JString::from(string);
            env.get_string(&string)
                .map(|text| text.to_string_lossy().into_owned())
        }
        Err(_) => Err(jni::errors::Error::JniCall(jni::errors::JniError::Unknown)),
    };
    match text {
        Ok(text) => format!(" ({text})"),
        Err(_) => " (a Java exception was thrown)".to_owned(),
    }
}

/// Pulls the object out of a JNI result, clearing any pending Java exception
/// on the way so a failure cannot poison the next JNI call.
pub(crate) fn take<'local>(
    env: &mut JNIEnv<'local>,
    result: JniResult<JValueOwned<'local>>,
) -> Result<JObject<'local>, String> {
    let object = checked(env, result)?;
    object.l().map_err(|e| format!("JNI: {e}"))
}

/// Pulls the `jboolean` out of a JNI result, like [`take`] for primitives.
pub(crate) fn take_bool(env: &mut JNIEnv, result: JniResult<JValueOwned>) -> Result<bool, String> {
    checked(env, result)?.z().map_err(|e| format!("JNI: {e}"))
}

/// Pulls the `jint` out of a JNI result, like [`take`] for primitives.
pub(crate) fn take_int(env: &mut JNIEnv, result: JniResult<JValueOwned>) -> Result<i32, String> {
    checked(env, result)?.i().map_err(|e| format!("JNI: {e}"))
}
