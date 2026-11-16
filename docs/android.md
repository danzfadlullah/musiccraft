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

## Build

The GitHub Actions workflow `.github/workflows/android.yml` builds one APK
for both ARM ABIs (`arm64-v8a` and `armeabi-v7a`) on every push that touches
the Android pieces; grab it from the run's artifacts.

To build locally you need the SDK, the NDK and [cargo-apk](https://github.com/rust-mobile/cargo-apk):

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

Release builds sign with the keystore named in
`[package.metadata.android.signing.release]`. The CI workflow generates a
gitignored throwaway keystore on the runner, so its artifact APKs install
right away but must never be published to a store. To make a real release,
create and keep your own key, and point the metadata at it:

```sh
keytool -genkeypair -v -keystore musiccraft.keystore -alias musiccraft \
  -keyalg RSA -keysize 2048 -validity 10000 \
  -dname "CN=MusicCraft, O=MusicCraft"
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
