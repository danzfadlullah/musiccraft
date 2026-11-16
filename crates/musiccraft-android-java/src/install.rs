//! Hands an APK held in memory to the Android system installer: the bytes go
//! into `Download/MusicCraft/` through MediaStore, and the resulting content
//! URI opens in an `ACTION_VIEW` install intent. This is what makes the
//! in-app update possible without a file manager.

use android_activity::AndroidApp;
use jni::objects::{JByteArray, JObject, JValue};
use jni::{JNIEnv, JavaVM};

use crate::{checked, take};

const MIME_APK: &str = "application/vnd.android.package-archive";
const APK_NAME: &str = "MusicCraft-update.apk";
const FOLDER: &str = "Download/MusicCraft";
/// FLAG_ACTIVITY_NEW_TASK (required when starting from a non-activity
/// context stack) | FLAG_GRANT_READ_URI_PERMISSION (the installer may read
/// our MediaStore entry).
const INTENT_FLAGS: i32 = 0x1000_0000 | 0x0000_0001;

/// Saves `bytes` as an APK in the public Downloads folder and asks the system
/// installer to open it. Call from a thread attached to the JVM (the egui
/// thread on Android is the activity's main Java thread). On success the
/// system's install dialog is on screen; the user confirms the install.
///
/// Errors are plain strings: they go straight to the update panel.
pub fn install_apk(app: &AndroidApp, bytes: &[u8]) -> Result<(), String> {
    unsafe {
        let vm = JavaVM::from_raw(app.vm_as_ptr().cast())
            .map_err(|e| format!("cannot reach the JVM: {e}"))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("cannot attach to the JVM: {e}"))?;
        let activity = JObject::from_raw(app.activity_as_ptr().cast());
        install(&mut env, &activity, bytes)
    }
}

fn install<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject,
    bytes: &[u8],
) -> Result<(), String> {
    let uri = save_to_downloads(env, activity, bytes)?;
    let action_view = env.get_static_field(
        "android/content/Intent",
        "ACTION_VIEW",
        "Ljava/lang/String;",
    );
    let action = take(env, action_view)?;
    let mime_call = env.new_string(MIME_APK);
    let mime: JObject = checked(env, mime_call)?.into();
    let new_intent = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;Landroid/net/Uri;)V",
        &[JValue::Object(&action), JValue::Object(&mime)],
    );
    let intent = checked(env, new_intent)?;
    let set_data = env.call_method(
        &intent,
        "setDataAndType",
        "(Landroid/net/Uri;Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&uri), JValue::Object(&mime)],
    );
    checked(env, set_data)?;
    let add_flags = env.call_method(
        &intent,
        "addFlags",
        "(I)Landroid/content/Intent;",
        &[JValue::Int(INTENT_FLAGS)],
    );
    checked(env, add_flags)?;
    let start = env.call_method(
        activity,
        "startActivity",
        "(Landroid/content/Intent;)V",
        &[JValue::Object(&intent)],
    );
    checked(env, start)?;
    Ok(())
}

/// Writes the bytes into `Download/MusicCraft/` via MediaStore and returns
/// the content URI. MediaStore (not a plain file path) because scoped
/// storage only lets a `content://` URI be handed to another app.
fn save_to_downloads<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject,
    bytes: &[u8],
) -> Result<JObject<'local>, String> {
    let new_values = env.new_object("android/content/ContentValues", "()V", &[]);
    let values = checked(env, new_values)?;
    put_string(env, &values, "_display_name", APK_NAME)?;
    put_string(env, &values, "mime_type", MIME_APK)?;
    put_string(env, &values, "relative_path", FOLDER)?;
    let downloads_uri = env.get_static_field(
        "android/provider/MediaStore$Downloads",
        "EXTERNAL_CONTENT_URI",
        "Landroid/net/Uri;",
    );
    let downloads = take(env, downloads_uri)?;
    let resolver_call = env.call_method(
        activity,
        "getContentResolver",
        "()Landroid/content/ContentResolver;",
        &[],
    );
    let resolver = take(env, resolver_call)?;
    let insert = env.call_method(
        &resolver,
        "insert",
        "(Landroid/net/Uri;Landroid/content/ContentValues;)Landroid/net/Uri;",
        &[JValue::Object(&downloads), JValue::Object(&values)],
    );
    let uri = take(env, insert)?;
    if uri.as_raw().is_null() {
        return Err("MediaStore refused the download entry".to_owned());
    }
    let data_call = env.byte_array_from_slice(bytes);
    let data: JByteArray = checked(env, data_call)?;
    let open = env.call_method(
        &resolver,
        "openOutputStream",
        "(Landroid/net/Uri;)Ljava/io/OutputStream;",
        &[JValue::Object(&uri)],
    );
    let stream = take(env, open)?;
    let write = env.call_method(&stream, "write", "([B)V", &[JValue::Object(&data)]);
    checked(env, write)?;
    let flush = env.call_method(&stream, "flush", "()V", &[]);
    checked(env, flush)?;
    let close = env.call_method(&stream, "close", "()V", &[]);
    checked(env, close)?;
    Ok(uri)
}

fn put_string<'local>(
    env: &mut JNIEnv<'local>,
    values: &JObject,
    key: &str,
    value: &str,
) -> Result<(), String> {
    let key_call = env.new_string(key);
    let key: JObject = checked(env, key_call)?.into();
    let value_call = env.new_string(value);
    let value: JObject = checked(env, value_call)?.into();
    let put = env.call_method(
        values,
        "put",
        "(Ljava/lang/String;Ljava/lang/String;)Z",
        &[JValue::Object(&key), JValue::Object(&value)],
    );
    checked(env, put)?;
    Ok(())
}
