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
| `musiccraft_app::run_android` | The shared application with no command line: the activity's surface is the window, and the app's private storage stands in for the desktop config folder (`crates/musiccraft-app/src/android.rs`). |
| Touch style | On Android the theme gets wider hit targets and spacing (`apply_touch_style`); egui turns touch events into pointer input, so taps, drags and long presses work everywhere the mouse does. The system's display density sets the UI scale. |
| File dialogs | rfd has no Android backend, so `musiccraft-file-dialog` builds against a stub that answers "cancelled" (`docs/spec/style/dialogs.md` keeps its desktop meaning). Move `.mcproj` files on and off the device with your file manager; open them through the browser panel's folders. |
| MIDI | `midir` speaks Android MIDI, so USB and Bluetooth keyboards show up in Settings once connected. |
| Audio | `cpal` opens the device through Android's audio stack; the engine runs in the callback exactly as on the desktop. |

Not available on Android (yet): CLAP and VST3 plugin hosting, Demucs stem
splitting (no ONNX Runtime), audio input recording, and the live-control
server. Everything else — channel rack, piano roll, playlist, mixer, the
built-in instruments and effects, export — is the desktop code.

## Logs

The app writes `musiccraft.log` (start-up steps, audio, panics) to, in order:

1. `Android/data/io.github.bhimio1.musiccraft/files/MusicCraft/logs/` on
   shared storage — open it with the phone's file manager (Android 10) or
   `adb pull` from a computer,
2. the app's private storage, same `MusicCraft/logs` layout,
3. `Download/MusicCraft/` — where the system allows it.

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

You need the SDK, the NDK and [cargo-apk](https://github.com/rust-mobile/cargo-apk):

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

## Notes for contributors

- The `unsafe_code = "forbid"` workspace rule has one deliberate exception:
  `musiccraft-android`'s `android_main` needs `#[unsafe(no_mangle)]` because
  that exact symbol is what android-activity's glue looks up. The crate holds
  nothing else, and does not inherit the workspace lints.
- Settings, autosaves and recorded takes live in the app's private storage
  (`internalDataPath`), so they survive across runs but are not user-visible
  files.
- The desktop build is untouched: `musiccraft-app` is now a library plus a
  thin `main.rs`, and every Android difference sits behind
  `#[cfg(target_os = "android")]`.
