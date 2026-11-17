# MusicCraft for Android

MusicCraft is a clean-room, pure-Rust digital audio workstation with a
pattern-based workflow: channel rack, piano roll, playlist and mixer. This
fork — [danzfadlullah/musiccraft](https://github.com/danzfadlullah/musiccraft),
forked from [bhimio1/musiccraft](https://github.com/bhimio1/musiccraft) —
exists to run it on **Android phones and tablets**: the same engine, the same
interface and the same project files, rebuilt for touch, ARM and Android's
audio and MIDI stacks.

Everything else about MusicCraft — desktop builds for Windows, macOS and
Linux, the plugin exports, the headless CLI and the MCP server — lives in the
**[upstream repository](https://github.com/bhimio1/musiccraft)** and its
[releases](https://github.com/bhimio1/musiccraft/releases). This fork only
adds and maintains the Android port.

![Playlist arrangement with markers and automation, and the mixer window](docs/screenshots/playlist-mixer.png)

<table>
  <tr>
    <td><img src="docs/screenshots/channel-rack.png" alt="Channel rack step sequencer"></td>
    <td><img src="docs/screenshots/piano-roll.png" alt="Piano roll with a melody over chords and the velocity lane"></td>
  </tr>
  <tr>
    <td align="center">Channel rack</td>
    <td align="center">Piano roll</td>
  </tr>
  <tr>
    <td colspan="2"><img src="docs/screenshots/synth-and-eq.png" alt="Built-in synth editor with presets, and the equaliser window with its response curve"></td>
  </tr>
  <tr>
    <td colspan="2" align="center">Synth editor and an effect window</td>
  </tr>
</table>

The screenshots show [`examples/demo-song.mcs`](examples/demo-song.mcs).

## Download

Every build of `main` refreshes the rolling
**[`android` pre-release](https://github.com/danzfadlullah/musiccraft/releases/tag/android)**
on the Releases tab — that address always holds the newest APK:

- one APK for both **arm64-v8a** (modern phones) and **armeabi-v7a** (older
  32-bit phones), **Android 10 or newer**;
- marked as a pre-release: for testing, not for a store;
- signed with the project's persistent test key, so a new APK **installs
  straight over an older one** — no uninstall in between;
- first install needs *install unknown apps* allowed for your browser or file
  manager.

## Updating from the app

The app checks for updates by itself (once at start-up, and any time from
**Settings → Updates**, which also shows the installed version). When a
newer build is on the Releases tab, a pop-up offers it — with the
changelog — and one tap, **Download and install**, streams the APK down
with a progress bar, checks its SHA-256 fingerprint against the release,
and opens Android's installer to confirm the swap. No
browser, no manual download; once the update has installed, the spent APK
is swept away (a switch in **Settings → General** keeps it instead). If the
installer cannot open, the tab says where the APK copy is. See
[`docs/android.md`](docs/android.md).

## What works on Android

- **Fullscreen, landscape only**: the status bar is hidden and the layout
  stays wide, the way a DAW wants it.
- **Touch everywhere**: taps, drags and long presses do what click, drag and
  right-click do on the desktop, and hit targets are widened for fingers.
- **The full pattern workflow**: channel rack, piano roll (slides, chords,
  per-note events), playlist with automation and audio clips, mixer with the
  47 built-in effects, the built-in instruments (synth, FM, keys, additive,
  granular, drum synth, 16-pad kit, plucked, layer, sampler, Antiserum) and
  the browser.
- **Soundfonts**: the app ships with a General MIDI soundfont (TimGM6mb),
  so imported MIDI files play the file's own instruments out of the box:
  every part opens on its own channel playing its General MIDI preset (the
  drum channel the percussion kit). **Options → Soundfonts** shows the
  soundfont files in use and picks a `.sf2`/`.sf3` to add as a zone
  sampler channel; a soundfont loaded this way takes over the MIDI
  import's instruments.
- **Audio and MIDI**: output through AAudio, MIDI keyboards in and gear out
  through AMidi (USB and Bluetooth).
- **Projects and export**: open and save `.mcproj`, import MIDI files into
  the selected channel (**File → Import MIDI file…**, like FL Studio),
  export WAV (16/24/32f) and FLAC (16/24), one stem per mixer insert. A
  typical start: import the MIDI file — every part opens on its own channel
  playing its own instrument through the bundled General MIDI soundfont.

Not on Android (yet): CLAP/VST3 plugin hosting, Demucs stem splitting, audio
input recording, and folder picks in the file panels. Opening and saving
projects works through the system's file picker instead, filtered to the
kinds of file each dialog asks for.

## Build from source

```sh
rustup target add aarch64-linux-android armv7-linux-androideabi
cargo install cargo-apk --locked
export ANDROID_HOME=~/Android/Sdk
export NDK_HOME="$ANDROID_HOME"/ndk/27.2.12479018
cargo apk build -p musiccraft-android --release    # target/release/apk/MusicCraft.apk
```

The [`android.yml`](.github/workflows/android.yml) workflow builds the APK on
every push that touches the Android pieces and publishes it as the rolling
pre-release above. SDK/NDK setup, signing and the update machinery are
described in [`docs/android.md`](docs/android.md).

## Development

```sh
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

- `CLAUDE.md` — working rules; `ARCHITECTURE.md` — crates and threading;
  `docs/spec/` — behavior specs; `docs/spec/style/` — interface style guide;
  `docs/clean-room.md` — provenance rules; `docs/android.md` — the Android
  port.

MusicCraft is an independent project and is not affiliated with any other DAW.

## License

MusicCraft is free software, licensed under the GNU General Public License,
version 3 or (at your option) any later version. See `LICENSE`. The `.flp`
importer is a Rust port of [PyFLP](https://github.com/demberto/PyFLP),
© 2022 demberto, also GPL-3.0-or-later. The Serum preset importer is a Rust
port of [serum2vital](https://github.com/btesser/serum2vital), © 2026 Ben
Tesser, also GPL-3.0-or-later. Serum is a trademark of Xfer Records;
MusicCraft is not affiliated with Xfer Records and ships none of Serum's
wavetables or presets.
