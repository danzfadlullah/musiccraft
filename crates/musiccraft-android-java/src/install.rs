//! Hands an APK held in memory to the Android system installer through the
//! `PackageInstaller` session API: the bytes stream into an install session
//! and `commit` makes Android show its own confirmation dialog ("Do you want
//! to update this application?"). This is what makes the in-app update
//! possible without a file manager. The manifest must declare
//! `REQUEST_INSTALL_PACKAGES`: without it Android 8 and newer refuse to show
//! the dialog, silently.

use android_activity::AndroidApp;
use jni::objects::{JByteArray, JObject, JValue};
use jni::{JNIEnv, JavaVM};

use crate::{checked, take, take_bool, take_int, with_env};

/// The update APKs' name prefix, completed with the version
/// (`MusicCraft-update-0.6.0.apk`), used for the install session's stream
/// name and for the spare copies in `Download/MusicCraft/` that older
/// versions left behind, so the cleanup after a finished update knows which
/// files are spent.
const APK_PREFIX: &str = "MusicCraft-update-";
/// `PackageInstaller.SessionParams.MODE_FULL_INSTALL`.
const MODE_FULL_INSTALL: i32 = 1;
/// `PendingIntent.FLAG_IMMUTABLE`: required to name mutability explicitly
/// from Android 12 on, and the installer's answer never needs to change.
const FLAG_IMMUTABLE: i32 = 0x0400_0000;
/// `PackageInstaller.ACTION_CONFIRM_INSTALL`: the intent the session's
/// result is delivered through.
const ACTION_CONFIRM_INSTALL: &str = "android.content.pm.action.CONFIRM_INSTALL";
/// How long to wait for the Java main thread to run the install (the 30 MB
/// stream write included) before giving up on it.
const INSTALL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);

/// Streams `bytes` into a `PackageInstaller` session and commits it, so
/// Android opens its install confirmation for the update. The whole session
/// runs on the Java main thread — like the SAF picker's
/// `startActivityForResult`, session work from the app's own thread can take
/// the process down — and this waits (up to 15 s) for it to finish. The
/// dialog itself appears once `commit` is through; this returns before the
/// user answers it.
///
/// Errors are plain strings: they go straight to the update panel.
pub fn install_apk(app: &AndroidApp, bytes: &[u8], version: &str) -> Result<(), String> {
    let (sender, receiver) = std::sync::mpsc::channel();
    let app = app.clone();
    let bytes = bytes.to_vec();
    let version = version.to_owned();
    app.run_on_java_main_thread(Box::new(move || {
        let result = with_env(|env, activity| install(env, activity, &bytes, &version));
        if let Err(err) = &result {
            crate::log(&format!("installer: {err}"));
        }
        let _ = sender.send(result);
    }));
    receiver
        .recv_timeout(INSTALL_TIMEOUT)
        .unwrap_or_else(|_| Err("the installer did not answer".to_owned()))
}

/// The names of the update APKs sitting in `Download/MusicCraft/`, for the
/// cleanup that runs once an update has installed. Older versions staged
/// their update APKs there; the session API streams straight into the
/// installer, so new downloads leave nothing behind.
pub fn downloaded_apk_names(app: &AndroidApp) -> Result<Vec<String>, String> {
    unsafe {
        let vm = JavaVM::from_raw(app.vm_as_ptr().cast())
            .map_err(|e| format!("cannot reach the JVM: {e}"))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("cannot attach to the JVM: {e}"))?;
        let activity = JObject::from_raw(app.activity_as_ptr().cast());
        names(&mut env, &activity)
    }
}

/// Deletes the `Download/MusicCraft/` APK rows with this display name.
pub fn delete_downloaded_apk(app: &AndroidApp, name: &str) -> Result<(), String> {
    unsafe {
        let vm = JavaVM::from_raw(app.vm_as_ptr().cast())
            .map_err(|e| format!("cannot reach the JVM: {e}"))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("cannot attach to the JVM: {e}"))?;
        let activity = JObject::from_raw(app.activity_as_ptr().cast());
        delete(&mut env, &activity, name)
    }
}

/// The install session: `PackageInstaller` → session parameters → session →
/// the APK's bytes → `commit`. Runs on the Java main thread (see
/// [`install_apk`]). Every step is checked, so a failure names the step.
fn install(
    env: &mut JNIEnv<'_>,
    activity: &JObject,
    bytes: &[u8],
    version: &str,
) -> Result<(), String> {
    let manager_call = env.call_method(
        activity,
        "getPackageManager",
        "()Landroid/content/pm/PackageManager;",
        &[],
    );
    let manager = take(env, manager_call)?;
    let installer_call = env.call_method(
        &manager,
        "getPackageInstaller",
        "()Landroid/content/pm/PackageInstaller;",
        &[],
    );
    let installer = take(env, installer_call)?;
    let params = env.new_object(
        "android/content/pm/PackageInstaller$SessionParams",
        "(I)V",
        &[JValue::Int(MODE_FULL_INSTALL)],
    );
    let params = checked(env, params)?;
    let create = env.call_method(
        &installer,
        "createSession",
        "(Landroid/content/pm/PackageInstaller$SessionParams;)I",
        &[JValue::Object(&params)],
    );
    let session_id = take_int(env, create)?;
    let open = env.call_method(
        &installer,
        "openSession",
        "(I)Landroid/content/pm/PackageInstaller$Session;",
        &[JValue::Int(session_id)],
    );
    let session = take(env, open)?;
    if let Err(err) = write_apk(env, &session, bytes, version) {
        let abandon = env.call_method(&session, "abandon", "()V", &[]);
        let _ = checked(env, abandon);
        return Err(err);
    }
    commit(env, activity, &session)
}

/// Writes the APK's bytes into the session's stream. The stream is claimed
/// with the update's name, so a failed session is recognizable in the
/// installer's own records.
fn write_apk(
    env: &mut JNIEnv<'_>,
    session: &JObject,
    bytes: &[u8],
    version: &str,
) -> Result<(), String> {
    let name = format!("{APK_PREFIX}{version}.apk");
    let name_call = env.new_string(name);
    let name: JObject = checked(env, name_call)?.into();
    let open_write = env.call_method(
        session,
        "openWrite",
        "(Ljava/lang/String;JJ)Ljava/io/OutputStream;",
        &[
            JValue::Object(&name),
            JValue::Long(0),
            JValue::Long(bytes.len() as i64),
        ],
    );
    let stream = take(env, open_write)?;
    let data_call = env.byte_array_from_slice(bytes);
    let data: JByteArray = checked(env, data_call)?;
    let write = env.call_method(&stream, "write", "([B)V", &[JValue::Object(&data)]);
    checked(env, write)?;
    let flush = env.call_method(&stream, "flush", "()V", &[]);
    checked(env, flush)?;
    let close = env.call_method(&stream, "close", "()V", &[]);
    checked(env, close)?;
    Ok(())
}

/// Commits the session with a `PendingIntent` for the result, which makes
/// Android show the install confirmation. The session is closed afterwards —
/// `commit` takes over from there.
fn commit(env: &mut JNIEnv<'_>, activity: &JObject, session: &JObject) -> Result<(), String> {
    let action_call = env.new_string(ACTION_CONFIRM_INSTALL);
    let action: JObject = checked(env, action_call)?.into();
    let new_intent = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&action)],
    );
    let intent = checked(env, new_intent)?;
    let pending = env.call_static_method(
        "android/app/PendingIntent",
        "getActivity",
        "(Landroid/content/Context;ILandroid/content/Intent;I)Landroid/app/PendingIntent;",
        &[
            JValue::Object(activity),
            JValue::Int(0),
            JValue::Object(&intent),
            JValue::Int(FLAG_IMMUTABLE),
        ],
    );
    let pending = take(env, pending)?;
    let sender = env.call_method(
        &pending,
        "getIntentSender",
        "()Landroid/content/IntentSender;",
        &[],
    );
    let sender = take(env, sender)?;
    let send = env.call_method(
        session,
        "commit",
        "(Landroid/content/IntentSender;)V",
        &[JValue::Object(&sender)],
    );
    checked(env, send)?;
    let close = env.call_method(session, "close", "()V", &[]);
    checked(env, close)?;
    Ok(())
}

/// The content resolver the activity's MediaStore calls go through.
fn resolver<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject,
) -> Result<JObject<'local>, String> {
    let call = env.call_method(
        activity,
        "getContentResolver",
        "()Landroid/content/ContentResolver;",
        &[],
    );
    take(env, call)
}

fn downloads_uri<'local>(env: &mut JNIEnv<'local>) -> Result<JObject<'local>, String> {
    let field = env.get_static_field(
        "android/provider/MediaStore$Downloads",
        "EXTERNAL_CONTENT_URI",
        "Landroid/net/Uri;",
    );
    take(env, field)
}

/// A Java `String[]` holding `values`.
fn string_array<'local>(
    env: &mut JNIEnv<'local>,
    values: &[String],
) -> Result<jni::objects::JObjectArray<'local>, String> {
    let empty = checked(env, env.new_string(""))?;
    let new_array = env.new_object_array(values.len() as i32, "java/lang/String", empty);
    let array = checked(env, new_array)?;
    for (index, value) in values.iter().enumerate() {
        let item = checked(env, env.new_string(value))?;
        let set = env.set_object_array_element(&array, index as i32, item);
        checked(env, set)?;
    }
    Ok(array)
}

/// Queries `Download/MusicCraft/` for the update APKs' display names.
fn names<'local>(env: &mut JNIEnv<'local>, activity: &JObject) -> Result<Vec<String>, String> {
    let resolver = resolver(env, activity)?;
    let downloads = downloads_uri(env)?;
    let projection = string_array(env, &["_display_name".to_owned()])?;
    let selection = checked(env, env.new_string("_display_name LIKE ?"))?;
    let args = string_array(env, &[format!("{APK_PREFIX}%")])?;
    let query = env.call_method(
        &resolver,
        "query",
        "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
        &[
            JValue::Object(&downloads),
            JValue::Object(&projection),
            JValue::Object(&selection),
            JValue::Object(&args),
            JValue::Object(&JObject::null()),
        ],
    );
    let cursor = take(env, query)?;
    let mut found = Vec::new();
    loop {
        let next = env.call_method(&cursor, "moveToNext", "()Z", &[]);
        if !take_bool(env, next)? {
            break;
        }
        let get = env.call_method(
            &cursor,
            "getString",
            "(I)Ljava/lang/String;",
            &[JValue::Int(0)],
        );
        let name = take(env, get)?;
        if name.as_raw().is_null() {
            continue;
        }
        let jstring = jni::objects::JString::from(name);
        let got = env.get_string(&jstring);
        let text = checked(env, got)?;
        found.push(text.to_string_lossy().into_owned());
    }
    let close = env.call_method(&cursor, "close", "()V", &[]);
    checked(env, close)?;
    Ok(found)
}

/// Deletes the `Download/MusicCraft/` rows whose display name is `name`.
fn delete<'local>(env: &mut JNIEnv<'local>, activity: &JObject, name: &str) -> Result<(), String> {
    let resolver = resolver(env, activity)?;
    let downloads = downloads_uri(env)?;
    let selection = checked(env, env.new_string("_display_name = ?"))?;
    let args = string_array(env, &[name.to_owned()])?;
    let call = env.call_method(
        &resolver,
        "delete",
        "(Landroid/net/Uri;Ljava/lang/String;[Ljava/lang/String;)I",
        &[
            JValue::Object(&downloads),
            JValue::Object(&selection),
            JValue::Object(&args),
        ],
    );
    take_int(env, call)?;
    Ok(())
}
