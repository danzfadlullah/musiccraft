# MusicCraft on Android

MusicCraft runs on Android phones and tablets as a NativeActivity: the same
egui interface, the same engine, the same command registry, driven by touch.
The Rust code is shared with the desktop; only the entry point, the folders
and the file dialogs differ. Android 10 (API 29) or newer is required: audio
goes through AAudio and MIDI keyboards through AMidi, and both libraries
link from API 29.

## What is Android-specific

| Piece | What it does |
| --- | --- |
| `crates/musiccraft-android` | The `cdylib` Android app: exports `android_main` and hands the `AndroidApp` handle to `musiccraft_app::run_android`. Built with [cargo-apk](https://github.com/rust-mobile/cargo-apk); its `[package.metadata.android]` block lives in that crate's `Cargo.toml`. |
| `crates/musiccraft-android-java` | The pieces that must talk to Java through JNI: streams a downloaded APK into a `PackageInstaller` session and commits it, so the system installer opens, sweeps out a spent update's APKs, and drives the system's SAF file picker. |
| `musiccraft_app::run_android` | The shared application with no command line: the activity's surface is the window, and the app's private storage stands in for the desktop config folder (`crates/musiccraft-app/src/android.rs`). |
| Touch style | On Android the theme gets wider hit targets and spacing (`apply_touch_style`); egui turns touch events into pointer input, so taps, drags and long presses work everywhere the mouse does. The system's display density sets the UI scale. |
| Screen | Landscape only (`sensorLandscape`: both landscape sides, following the sensor) and fullscreen: the activity sets `FLAG_FULLSCREEN | FLAG_LAYOUT_IN_SCREEN`, so the status bar stays out of the way. The system's gesture pill remains; that is up to the launcher. |
| File dialogs | rfd has no Android backend, so `musiccraft-file-dialog` builds against the system's SAF panels: **open** shows the file picker (`ACTION_OPEN_DOCUMENT`, multi-select included), and whatever is picked is copied into the app's cache and handed to the app as a normal file. **Save** opens `ACTION_CREATE_DOCUMENT` seeded with the suggested name, and every later write to that project is pushed back to the picked location automatically. Folder picks have no SAF equivalent here and answer "cancelled"; file-type filters are passed to the panel as mime types (`EXTRA_MIME_TYPES`), with an `application/octet-stream` fallback so files the provider does not type (a `.mcproj`) stay visible. |
| Soundfonts | **Options → Soundfonts** lists the soundfont files the project's zone samplers play plus the bundled TimGM6mb, each with its preset counts — or why the file cannot be read, which is also logged — and picks a new `.sf2`/`.sf3` to add as a zone sampler channel (`channel.add_soundfont`; `soundfont.presets` lists the presets, `channel.load_soundfont` switches). The bundled font is verified at every startup and rewritten when corrupt, so a broken install heals itself. The samples stay inside the soundfont file — zones reference them as `soundfont:<file>#<index>` and the sample cache decodes them on demand. Looping, envelopes and pan are not applied yet. |
| MIDI | `midir` speaks Android MIDI, so USB and Bluetooth keyboards show up in Settings once connected. |
| Audio | `cpal` opens the device through Android's audio stack; the engine runs in the callback exactly as on the desktop. |

Not available on Android (yet): CLAP and VST3 plugin hosting, Demucs stem
splitting (no ONNX Runtime), audio input recording, folder picks in the file
panels, and the live-control server. Everything else — channel rack, piano
roll, playlist, mixer, the built-in instruments and effects, export — is the
desktop code.

## The SAF picker and the Java activity

Android hands a file panel's answer to `onActivityResult`, which a plain
`NativeActivity` never forwards to native code. The app therefore ships one
small Java class, `MusicCraftActivity` (in `crates/musiccraft-android/java`),
an `NativeActivity` subclass whose `onActivityResult` stows the answer — the
picked `content://` URIs, newline-separated — in a static field. The manifest
points the activity at that class (`name` plus `has_code = true`), and the
workflow compiles it with `javac` + `d8` into a `classes.dex` that is zip-
injected into the APK, which is then realigned (`zipalign`) and re-signed
(`apksigner`).

On the Rust side, `musiccraft-android-java` polls that field over JNI from
the file-dialog thread, copies each picked URI's bytes through the content
resolver into the app's private cache, and hands the cache paths back as the
dialog's answer. A save pick registers its cache file, and the app pushes
every later write back to the picked URI each frame (`sync`), so saving
lands where the user chose.

## Updates

The app checks its own updates against this fork's rolling `android`
pre-release: once at start-up, from **Options → Updates → Check for
updates**, and again
from **Settings → Updates**. When a
newer build is published there, a pop-up offers it wherever you are — with
the changelog (the commits since the previous release, which CI writes into
the release notes) — and the tab offers **Download and install**. The pop-up
stays on screen for the whole install: the offer, the download's progress
bar, a spinner while the installer opens and the result, so it no longer
closes when you press **Download and install**. The
APK streams down with a progress bar and goes straight into a
`PackageInstaller` session (crates/musiccraft-android-java/src/install.rs):
the download finishes into the installer by itself, no file manager and no
APK copy in Downloads in between. Before it may install, the download's
SHA-256 fingerprint must match the one GitHub reports for the release asset;
a truncated or altered APK is refused with the two fingerprints in the log.
The new build
installs in place — same signature, no uninstall.

Two permissions make that work. The manifest declares
`REQUEST_INSTALL_PACKAGES`: without it Android 8 and newer refuse to show
the installer's confirmation dialog — the session commits but nothing
appears, which is exactly the silent failure this replaced. And the user
must allow the app once under **Install unknown apps**: the session's
answer comes back to the app's activity (`MusicCraftActivity`, the install
session's status receiver), and when the system asks for the user's action
it opens that settings screen — or, once granted, the dialog asking *Do
you want to update this application?* — itself. A session whose receiver
cannot be launched (the earlier bare-action, immutable PendingIntent)
waits forever with nothing on screen. If the installer cannot be opened
all the same, the tab says where the APK copy is
instead: `Android/data/io.github.bhimio1.musiccraft/files/MusicCraft/update/`,
open it with a file manager.

The spare copy in the app's storage carries the version in its name
(`MusicCraft-update-0.10.9.apk`). Once an update has installed,
the next start sweeps the spent ones out (its version is not newer than the
one running; a download whose install was cancelled survives) — both the
spare copies and the `Download/MusicCraft/` entries older versions left
behind. **Options → Updates** has the switch: *Delete the update APK after
it installs*, on by
default.

Every APK carries the app's semantic version (the workspace version, shown
in **Settings → Updates**) and the commit it was built from
(`MUSICCRAFT_BUILD_SHA`). The update check compares versions first: a
release whose version is older than the installed one is ignored, and the
same version rebuilt (the rolling release after every push) still offers
itself, so a plain `cargo apk build` — which has no commit stamp — gets
offered the release too. The `android.permission.INTERNET` permission exists
for this check alone; nothing else in the app reaches the network.

## Logs

The app writes one log file per session, named after the session's start
(`musiccraft-20261117-062022.log`) — what it does as it does it: start-up,
every command the interface runs, file picks and their answers, audio and
MIDI (re)starts, the update check, download and install, and panics — to,
in order:

1. `Android/data/io.github.bhimio1.musiccraft/files/MusicCraft/logs/` on
   shared storage — open it with the phone's file manager (Android 10) or
   `adb pull` from a computer,
2. the app's private storage, same `MusicCraft/logs` layout,
3. `Download/MusicCraft/` — where the system allows it.

Writing the log can be switched off in **Settings → General** ("Write the
log file"); panics are logged either way. Old sessions' logs are pruned to
the newest ten per folder, so a session's log never mixes into another's
and the folders cannot grow without bound.

When reporting a crash, send that file, or run `adb logcat` while opening the
app. The Actions tab can also build a debug APK (Run workflow → profile:
debug): bigger, but `adb logcat` shows everything and a debugger can attach.

## Build

The GitHub Actions workflow `.github/workflows/android.yml` builds one APK
for both ARM ABIs (`arm64-v8a` and `armeabi-v7a`) on every push that touches
the Android pieces. Every build of `main` also refreshes the rolling
[`android` pre-release](https://github.com/danzfadlullah/musiccraft/releases/tag/android)
on the Releases tab — that address always holds the newest APK, no need to
open the Actions run for the artifact.

## Signing

CI signs with the committed test keystore
`crates/musiccraft-android/release.keystore` (password `musiccraft-ci`).
Because every run uses the same key, a new APK installs straight over an
older one — no uninstall in between. The key is for test builds only: it is
public in the repo, so never ship an APK signed with it to a store or to
anyone outside the project.

To sign with a kept key instead, add the repo's Actions secrets
`KEYSTORE_BASE64` (the keystore, base64-encoded) and `KEYSTORE_PASSWORD`;
the workflow then prefers them over the committed key. For a real, locally
kept key:

```sh
keytool -genkeypair -v -keystore musiccraft.keystore -alias musiccraft \
  -keyalg RSA -keysize 2048 -validity 10000 \
  -dname "CN=MusicCraft, O=MusicCraft"
```

and point `[package.metadata.android.signing.release]` at it.

## Building locally

You need the SDK, the NDK, a JDK (for the activity's Java subclass) and
[cargo-apk](https://github.com/rust-mobile/cargo-apk):

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo install cargo-apk --locked
export ANDROID_HOME=~/Android/Sdk
export NDK_HOME="$ANDROID_HOME"/ndk/27.2.12479018
cargo apk build -p musiccraft-android --release
```

The APK lands in `target/release/apk/MusicCraft.apk` (both ARM ABIs packed
together). For a device:

```sh
cargo apk build -p musiccraft-android --release
adb install target/release/apk/MusicCraft.apk
```

`cargo apk run` builds, installs and starts the activity in one step.

A locally built APK does not carry the `classes.dex` (the workflow injects
it), so its SAF file panels answer "cancelled" until you compile and inject
it yourself:

```sh
javac -source 1.8 -target 1.8 -classpath "$ANDROID_HOME/platforms/android-34/android.jar" \
  -d build/classes crates/musiccraft-android/java/io/github/bhimio1/musiccraft/MusicCraftActivity.java
"$ANDROID_HOME/build-tools/34.0.0/d8" --release --min-api 29 \
  --lib "$ANDROID_HOME/platforms/android-34/android.jar" --output build/dex \
  build/classes/io/github/bhimio1/musiccraft/MusicCraftActivity.class
cd target/release/apk && zip MusicCraft.apk classes.dex  # from build/dex
"$ANDROID_HOME/build-tools/34.0.0/zipalign" -f -p 4 MusicCraft.apk aligned.apk
"$ANDROID_HOME/build-tools/34.0.0/apksigner" sign --ks crates/musiccraft-android/release.keystore \
  --ks-pass pass:musiccraft-ci --min-sdk-version 29 aligned.apk
```

## Notes for contributors

- The `unsafe_code = "forbid"` workspace rule has two deliberate exceptions:
  `musiccraft-android`'s `android_main` needs `#[unsafe(no_mangle)]` because
  that exact symbol is what android-activity's glue looks up, and
  `musiccraft-android-java` talks to Java through JNI (the update installer
  and the SAF picker), which cannot be written safely. Both crates hold
  nothing else and do not inherit the workspace lints.
- Settings, autosaves and recorded takes live in the app's private storage
  (`internalDataPath`), so they survive across runs but are not user-visible
  files.
- The desktop build is untouched: `musiccraft-app` is now a library plus a
  thin `main.rs`, and every Android difference sits behind
  `#[cfg(target_os = "android")]`.
