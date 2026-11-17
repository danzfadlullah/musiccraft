//! The app's bundled General MIDI soundfont: TimGM6mb (`assets/TimGM6mb.sf2`,
//! from the Free Soundfonts SF2 collection on archive.org), 128 melodic
//! presets in bank 0 and eight percussion kits in bank 128. MIDI import plays
//! a file's own instruments through it when the project has no soundfont yet
//! (`docs/spec/piano-roll.md`, "MIDI file import"), and the soundfont
//! commands accept `builtin` as its file name
//! (`docs/spec/channel-rack.md`, "Soundfonts").
//!
//! The soundfont ships inside the binary and is installed into the app's
//! folders at startup, because the sample cache plays `soundfont:` samples
//! from a real file path.

use std::path::{Path, PathBuf};

/// The installed file's name.
pub const FILE_NAME: &str = "TimGM6mb.sf2";

/// The soundfont's bytes, embedded in the binary.
#[must_use]
pub fn bytes() -> &'static [u8] {
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/TimGM6mb.sf2"))
}

/// Writes the soundfont into `dir` (creating it) unless a file of the same
/// size is already there, and returns its path. `None` when the folder
/// cannot be created or the file cannot be written.
pub fn install(dir: &Path) -> Option<PathBuf> {
    let path = dir.join(FILE_NAME);
    let fresh = std::fs::metadata(&path).is_ok_and(|meta| meta.len() == bytes().len() as u64);
    if !fresh {
        std::fs::create_dir_all(dir).ok()?;
        std::fs::write(&path, bytes()).ok()?;
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_writes_the_soundfont_once() {
        let dir =
            std::env::temp_dir().join(format!("musiccraft-default-sf-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = install(&dir).expect("install");
        assert_eq!(path, dir.join(FILE_NAME));
        assert_eq!(std::fs::read(&path).unwrap().len(), bytes().len());
        assert_eq!(bytes().len(), 5_969_788);
        // A second install finds the file of the same size and keeps it.
        let again = install(&dir).expect("install again");
        assert_eq!(again, path);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn install_fails_in_an_unwritable_folder() {
        let dir =
            std::env::temp_dir().join(format!("musiccraft-default-sf-bad-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        // A file where the folder should be blocks `create_dir_all`.
        let blocker = dir.join(FILE_NAME);
        std::fs::write(&blocker, b"not a folder").unwrap();
        let path = install(&blocker);
        assert!(path.is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_bundled_soundfont_is_a_full_general_midi_set() {
        let font = musiccraft_soundfont::Soundfont::parse(bytes()).expect("parse");
        let presets = font.presets();
        assert_eq!(presets.len(), 136);
        assert!(presets.iter().any(|p| p.bank == 0 && p.program == 0));
        assert!(presets.iter().any(|p| p.bank == 128 && p.program == 0));
    }
}
