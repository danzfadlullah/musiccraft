//! Names the APK this process runs from. The delta updater reads it as the
//! patch's base: it hashes the installed APK, matches it against the
//! release manifest's bases and rebuilds the update from these bytes.

use android_activity::AndroidApp;
use jni::objects::{JObject, JString};
use jni::{JNIEnv, JavaVM};

/// The path of the installed APK (`ApplicationInfo.sourceDir`), or why it
/// could not be read.
pub fn own_apk_path(app: &AndroidApp) -> Result<String, String> {
    unsafe {
        let vm = JavaVM::from_raw(app.vm_as_ptr().cast())
            .map_err(|e| format!("cannot reach the JVM: {e}"))?;
        let mut env = vm
            .attach_current_thread()
            .map_err(|e| format!("cannot attach to the JVM: {e}"))?;
        let activity = JObject::from_raw(app.activity_as_ptr().cast());
        own_path(&mut env, &activity)
    }
}

/// The activity's `ApplicationInfo.sourceDir`.
fn own_path(env: &mut JNIEnv<'_>, activity: &JObject) -> Result<String, String> {
    let call = env.call_method(
        activity,
        "getApplicationInfo",
        "()Landroid/content/pm/ApplicationInfo;",
        &[],
    );
    let info = crate::take(env, call)?;
    let field = env.get_field(&info, "sourceDir", "Ljava/lang/String;");
    let source = crate::take(env, field)?;
    let source = JString::from(source);
    env.get_string(&source)
        .map(|text| text.to_string_lossy().into_owned())
        .map_err(|e| format!("JNI: {e}"))
}
