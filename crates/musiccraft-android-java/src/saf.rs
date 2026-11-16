//! The system's SAF file picker (`ACTION_OPEN_DOCUMENT` and friends): the
//! app's stand-in for the desktop file dialogs.
//!
//! Android delivers the picker's answer to `onActivityResult`, which plain
//! native code never sees, so the activity is a small Java subclass
//! (`MusicCraftActivity`, in `crates/musiccraft-android/java`) that stows the
//! answer in a static field. `pick` launches the picker, waits for that
//! field, and materialises the picked `content://` URIs as cache files the
//! rest of the app reads and writes like any other file. A `Save` pick
//! registers the cache file with `sync`, which keeps pushing the app's later
//! writes to the same URI.

use std::path::PathBuf;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use jni::JNIEnv;
use jni::objects::{JObject, JString, JValue};

use crate::{app, checked, take, take_bool, take_int, with_env};

/// Which panel the picker opens (`musiccraft-file-dialog`'s Mode; folders
/// have no cache-file meaning and never reach here).
#[derive(Clone, Debug)]
pub enum SafMode {
    /// One existing file.
    File,
    /// One or more existing files.
    Files,
    /// A file to write into; `suggested_name` seeds the panel.
    Save { suggested_name: String },
}

/// A pick request.
pub struct SafRequest {
    pub mode: SafMode,
}

/// The static field on the activity class that carries the answer.
const RESULT_FIELD: &str = "pickResult";
const PICK_REQUEST: i32 = 42;
/// The cache folder picks materialise into, under the app's private storage.
const CACHE: &str = "cache/saf";
/// How much of a picked file is copied per `read` call.
const CHUNK: usize = 65_536;
/// How long `pick` waits for the panel before giving up.
const DEADLINE: Duration = Duration::from_secs(600);

/// Cache files that mirror a content URI they were picked through: the URI,
/// the cache file, and how much of it has been pushed so far.
static REGISTRY: Mutex<Vec<(String, PathBuf, u64)>> = Mutex::new(Vec::new());

/// Opens the SAF panel and waits for the answer: the picked files, copied
/// into the app's cache, or none when it was cancelled. Runs on the
/// file-dialog thread, which is built for waiting.
pub fn pick(request: SafRequest) -> Vec<PathBuf> {
    crate::log(&format!("SAF: opening the picker ({:?})", request.mode));
    clear_result();
    let mode = request.mode.clone();
    if let Err(err) = launch_on_main_thread(request) {
        crate::log(&format!("SAF: {err}"));
        return Vec::new();
    }
    let deadline = Instant::now() + DEADLINE;
    let mut broken = 0u32;
    loop {
        thread::sleep(Duration::from_millis(150));
        match poll_result() {
            Ok(Some(answer)) => {
                return match answer {
                    Ok(uris) => {
                        crate::log(&format!("SAF: picked {} URI(s)", uris.len()));
                        uris.iter()
                            .filter_map(|uri| materialize(uri, &mode))
                            .collect()
                    }
                    Err(err) => {
                        crate::log(&format!("SAF: {err}"));
                        Vec::new()
                    }
                };
            }
            // A read that keeps failing will not fix itself: give up, so the
            // picker stays reachable instead of hanging to the deadline.
            Ok(None) => broken = 0,
            Err(err) => {
                broken += 1;
                crate::log(&format!("SAF: {err}"));
                if broken >= 4 {
                    return Vec::new();
                }
            }
        }
        if Instant::now() > deadline {
            crate::log("SAF: the picker did not answer within ten minutes");
            return Vec::new();
        }
    }
}

/// Pushes cache files that changed since the last push to their URIs; run
/// this every frame so a pick's later saves reach the file the user chose.
pub fn sync() {
    let Ok(mut registry) = REGISTRY.lock() else {
        return;
    };
    let mut kept = Vec::new();
    for (uri, path, pushed) in registry.drain(..) {
        let Ok(meta) = std::fs::metadata(&path) else {
            continue;
        };
        let len = meta.len();
        if len == pushed || len == 0 {
            kept.push((uri, path, pushed));
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            kept.push((uri, path, pushed));
            continue;
        };
        crate::log(&format!("SAF: saving {} bytes to {uri}", bytes.len()));
        match with_env(|env, activity| write_uri(env, activity, &uri, &bytes)) {
            Ok(()) => kept.push((uri, path, len)),
            Err(err) => {
                crate::log(&format!("SAF: cannot save to {uri}: {err}"));
                kept.push((uri, path, pushed));
            }
        }
    }
    registry.extend(kept);
}

fn clear_result() {
    let cleared = with_env(|env, activity| {
        let class = checked(env, env.get_object_class(activity))?;
        let lookup = env.get_static_field_id(&class, RESULT_FIELD, "Ljava/lang/String;");
        let field = checked(env, lookup)?;
        let set = env.set_static_field(&class, field, JValue::Object(&JObject::null()));
        checked(env, set)
    });
    if let Err(err) = cleared {
        crate::log(&format!("SAF: {err}"));
    }
}

/// Builds the picker intent and starts it for a result. The activity's
/// `startActivityForResult` must run on the Java main thread, so it goes
/// through `run_on_java_main_thread`.
fn launch_on_main_thread(request: SafRequest) -> Result<(), String> {
    let Some(app) = app() else {
        return Err("the activity is not set up yet".to_owned());
    };
    app.run_on_java_main_thread(Box::new(move || {
        let result = crate::with_env(|env, activity| launch(env, activity, &request));
        if let Err(err) = result {
            crate::log(&format!("SAF: {err}"));
            let _ = set_result_string(&format!("!error:{err}"));
        }
    }));
    Ok(())
}

fn launch(env: &mut JNIEnv, activity: &JObject, request: &SafRequest) -> Result<(), String> {
    let (action_name, is_save) = match &request.mode {
        SafMode::File | SafMode::Files => ("ACTION_OPEN_DOCUMENT", false),
        SafMode::Save { .. } => ("ACTION_CREATE_DOCUMENT", true),
    };
    let action_field =
        env.get_static_field("android/content/Intent", action_name, "Ljava/lang/String;");
    let action = take(env, action_field)?;
    let new_intent = env.new_object(
        "android/content/Intent",
        "(Ljava/lang/String;)V",
        &[JValue::Object(&action)],
    );
    let intent = checked(env, new_intent)?;
    let mime = checked(
        env,
        env.new_string(if is_save {
            "application/octet-stream"
        } else {
            "*/*"
        }),
    )?;
    let set_type = env.call_method(
        &intent,
        "setType",
        "(Ljava/lang/String;)Landroid/content/Intent;",
        &[JValue::Object(&mime)],
    );
    checked(env, set_type)?;
    if !is_save {
        let openable = env.get_static_field(
            "android/content/Intent",
            "CATEGORY_OPENABLE",
            "Ljava/lang/String;",
        );
        let openable = take(env, openable)?;
        let add = env.call_method(
            &intent,
            "addCategory",
            "(Ljava/lang/String;)Landroid/content/Intent;",
            &[JValue::Object(&openable)],
        );
        checked(env, add)?;
    }
    match &request.mode {
        SafMode::Files => {
            let key = checked(env, env.new_string("android.intent.extra.ALLOW_MULTIPLE"))?;
            let put = env.call_method(
                &intent,
                "putExtra",
                "(Ljava/lang/String;Z)Landroid/content/Intent;",
                &[JValue::Object(&key), JValue::Bool(1)],
            );
            checked(env, put)?;
        }
        SafMode::Save { suggested_name } => {
            let key = checked(env, env.new_string("android.intent.extra.TITLE"))?;
            let name = checked(env, env.new_string(suggested_name))?;
            let put = env.call_method(
                &intent,
                "putExtra",
                "(Ljava/lang/String;Ljava/lang/String;)Landroid/content/Intent;",
                &[JValue::Object(&key), JValue::Object(&name)],
            );
            checked(env, put)?;
        }
        SafMode::File => {}
    }
    let start = env.call_method(
        activity,
        "startActivityForResult",
        "(Landroid/content/Intent;I)V",
        &[JValue::Object(&intent), JValue::Int(PICK_REQUEST)],
    );
    checked(env, start)?;
    Ok(())
}

/// The answer in the activity's static field, if it has landed: the picked
/// URIs, or why there are none. `Err` means the field could not be read at
/// all (a broken JNI session), which the caller treats as a dead pick.
fn poll_result() -> Result<Option<Result<Vec<String>, String>>, String> {
    let raw = with_env(|env, activity| {
        // The activity's own class object, not a name lookup: FindClass on a
        // plain native thread uses the system class loader, which cannot see
        // the app's classes, and the pick would never be read.
        let class = checked(env, env.get_object_class(activity))?;
        let get = env.get_static_field(&class, RESULT_FIELD, "Ljava/lang/String;");
        let field = take(env, get)?;
        if field.as_raw().is_null() {
            return Ok(None);
        }
        let jstring = JString::from(field);
        let got = env.get_string(&jstring);
        let text = checked(env, got)?;
        Ok(Some(text.to_string_lossy().into_owned()))
    })?;
    Ok(raw.map(|raw| parse_result(&raw)))
}

fn parse_result(raw: &str) -> Result<Vec<String>, String> {
    if raw == "!cancelled" {
        return Err("cancelled".to_owned());
    }
    if let Some(err) = raw.strip_prefix("!error:") {
        return Err(err.to_owned());
    }
    Ok(raw.split('\n').map(str::to_owned).collect())
}

fn set_result_string(value: &str) -> Result<(), String> {
    with_env(|env, activity| {
        let text = checked(env, env.new_string(value))?;
        let class = checked(env, env.get_object_class(activity))?;
        let lookup = env.get_static_field_id(&class, RESULT_FIELD, "Ljava/lang/String;");
        let field = checked(env, lookup)?;
        let set = env.set_static_field(&class, field, JValue::Object(&text));
        checked(env, set)
    })
}

/// Copies a picked URI's bytes into the app's cache and returns the copy's
/// path; a `Save` pick starts the copy empty and registers it with `sync`,
/// so the app's writes reach the file the user chose.
fn materialize(uri: &str, mode: &SafMode) -> Option<PathBuf> {
    let app = app()?;
    let name = with_env(|env, activity| display_name(env, activity, uri))
        .ok()
        .flatten()
        .unwrap_or_else(|| fallback_name(uri));
    let dir = app
        .internal_data_path()
        .map(|dir| dir.join(CACHE))
        .or_else(|| app.external_data_path().map(|dir| dir.join(CACHE)))?;
    if std::fs::create_dir_all(&dir).is_err() {
        return None;
    }
    let path = dir.join(name);
    if let SafMode::Save { .. } = mode {
        if std::fs::write(&path, b"").is_err() {
            crate::log("SAF: the cache file could not be created");
            return None;
        }
        if let Ok(mut registry) = REGISTRY.lock() {
            registry.push((uri.to_owned(), path.clone(), 0));
        }
    } else {
        let bytes = match with_env(|env, activity| read_uri(env, activity, uri)) {
            Ok(bytes) => bytes,
            Err(err) => {
                crate::log(&format!("SAF: {uri} could not be read: {err}"));
                return None;
            }
        };
        if std::fs::write(&path, &bytes).is_err() {
            crate::log("SAF: the picked file could not be copied into the cache");
            return None;
        }
        crate::log(&format!(
            "SAF: copied {} bytes to {}",
            bytes.len(),
            path.display()
        ));
    }
    Some(path)
}

/// The picked file's name, from the document provider's `_display_name`.
fn display_name(env: &mut JNIEnv, activity: &JObject, uri: &str) -> Result<Option<String>, String> {
    let uri = new_uri(env, uri)?;
    let resolver = resolver_of(env, activity)?;
    let column = checked(env, env.new_string("_display_name"))?;
    let array = env.new_object_array(1, "java/lang/String", column);
    let projection = checked(env, array)?;
    let query = env.call_method(
        &resolver,
        "query",
        "(Landroid/net/Uri;[Ljava/lang/String;Ljava/lang/String;[Ljava/lang/String;Ljava/lang/String;)Landroid/database/Cursor;",
        &[
            JValue::Object(&uri),
            JValue::Object(&projection),
            JValue::Object(&JObject::null()),
            JValue::Object(&JObject::null()),
            JValue::Object(&JObject::null()),
        ],
    );
    let cursor = take(env, query)?;
    if cursor.as_raw().is_null() {
        return Ok(None);
    }
    let move_first = env.call_method(&cursor, "moveToFirst", "()Z", &[]);
    let moved = take_bool(env, move_first)?;
    let name = if moved {
        let get = env.call_method(
            &cursor,
            "getString",
            "(I)Ljava/lang/String;",
            &[JValue::Int(0)],
        );
        let field = take(env, get)?;
        if field.as_raw().is_null() {
            None
        } else {
            let jstring = JString::from(field);
            let got = env.get_string(&jstring);
            let text = checked(env, got)?;
            Some(text.to_string_lossy().into_owned())
        }
    } else {
        None
    };
    let close = env.call_method(&cursor, "close", "()V", &[]);
    checked(env, close)?;
    Ok(name)
}

/// Reads a picked URI's whole stream through the content resolver.
fn read_uri(env: &mut JNIEnv, activity: &JObject, uri: &str) -> Result<Vec<u8>, String> {
    let uri = new_uri(env, uri)?;
    let resolver = resolver_of(env, activity)?;
    let open = env.call_method(
        &resolver,
        "openInputStream",
        "(Landroid/net/Uri;)Ljava/io/InputStream;",
        &[JValue::Object(&uri)],
    );
    let stream = take(env, open)?;
    let buffer = checked(env, env.new_byte_array(CHUNK as i32))?;
    let mut chunk = [0i8; CHUNK];
    let mut bytes = Vec::new();
    loop {
        let read = env.call_method(&stream, "read", "([B)I", &[JValue::Object(&buffer)]);
        let count = take_int(env, read)?;
        if count <= 0 {
            break;
        }
        checked(env, env.get_byte_array_region(&buffer, 0, &mut chunk))?;
        bytes.extend(chunk[..count as usize].iter().map(|byte| *byte as u8));
    }
    let close = env.call_method(&stream, "close", "()V", &[]);
    checked(env, close)?;
    Ok(bytes)
}

/// Writes `bytes` to a picked URI through the content resolver; the tail of
/// `sync`, pushing a save-picked cache file's changes back.
fn write_uri(env: &mut JNIEnv, activity: &JObject, uri: &str, bytes: &[u8]) -> Result<(), String> {
    let uri = new_uri(env, uri)?;
    let resolver = resolver_of(env, activity)?;
    let open = env.call_method(
        &resolver,
        "openOutputStream",
        "(Landroid/net/Uri;)Ljava/io/OutputStream;",
        &[JValue::Object(&uri)],
    );
    let stream = take(env, open)?;
    let data = checked(env, env.byte_array_from_slice(bytes))?;
    let write = env.call_method(&stream, "write", "([B)V", &[JValue::Object(&data)]);
    checked(env, write)?;
    let flush = env.call_method(&stream, "flush", "()V", &[]);
    checked(env, flush)?;
    let close = env.call_method(&stream, "close", "()V", &[]);
    checked(env, close)?;
    Ok(())
}

fn resolver_of<'local>(
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

fn new_uri<'local>(env: &mut JNIEnv<'local>, uri: &str) -> Result<JObject<'local>, String> {
    let text = checked(env, env.new_string(uri))?;
    let parsed = env.call_static_method(
        "android/net/Uri",
        "parse",
        "(Ljava/lang/String;)Landroid/net/Uri;",
        &[JValue::Object(&text)],
    );
    take(env, parsed)
}

/// A name for a URI the provider would not describe: its last path segment.
fn fallback_name(uri: &str) -> String {
    let segment = uri.rsplit('/').find(|part| !part.is_empty());
    match segment {
        Some(part) => part.split('%').next().unwrap_or(part).to_owned(),
        None => "picked.bin".to_owned(),
    }
}
