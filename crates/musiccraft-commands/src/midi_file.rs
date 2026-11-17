//! Reader for Standard MIDI Files (SMF formats 0 and 1), enough to import notes.
//! Follows the public MIDI 1.0 specification's file format.

use std::collections::BTreeMap;

/// A note read from a MIDI file, timed in the file's own ticks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FileNote {
    pub start: u64,
    pub length: u64,
    pub key: u8,
    pub velocity: u8,
    /// The MIDI channel the note sounded on (0-15).
    pub channel: u8,
}

/// What a MIDI channel said about itself outside its notes: the track name
/// it first sounded in, its last program change, and its last channel
/// volume (`CC7`) and pan (`CC10`).
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct PartInfo {
    pub name: Option<String>,
    pub program: Option<u8>,
    pub volume: Option<u8>,
    pub pan: Option<u8>,
}

/// Notes and the file's ticks per quarter note.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct MidiFile {
    pub ticks_per_quarter: u16,
    pub notes: Vec<FileNote>,
    /// The earliest time signature: numerator and denominator.
    pub time_signature: Option<(u32, u32)>,
    /// The first tempo event, in beats per minute.
    pub tempo_bpm: Option<f64>,
    /// Per MIDI channel (0-15) what [`PartInfo`] records.
    pub parts: BTreeMap<u8, PartInfo>,
    /// Per MIDI channel (0-15), the channel's pitch bends as (tick,
    /// semitones), sorted by tick with repeats collapsed. Each channel's bend
    /// range is its own RPN 0 setting (the General MIDI default ±2 semitones
    /// when the file sets none). A bend moves every sounding note of its
    /// channel, like a MIDI player's.
    pub bends: BTreeMap<u8, Vec<(u64, f32)>>,
}

/// Most bend points one channel keeps; denser curves are thinned.
const MAX_BEND_POINTS: usize = 20_000;

/// A 14-bit pitch-bend value as semitones, over the channel's bend range.
fn bend_semitones(value: u16, range: f32) -> f32 {
    (f32::from(value) - 8192.0) / 8192.0 * range
}

/// A channel's RPN state while a track plays: the selected RPN's MSB and LSB,
/// its bend range in semitones (the General MIDI default ±2), and the coarse
/// data-entry value waiting for the fine one.
#[derive(Clone, Copy)]
struct Rpn {
    selected: u16,
    range: f32,
    coarse: u8,
}

impl Default for Rpn {
    fn default() -> Self {
        Self {
            selected: u16::MAX,
            range: 2.0,
            coarse: 0,
        }
    }
}

impl Rpn {
    /// Applies one controller, updating the bend range on RPN 0 (pitch-bend
    /// range): data-entry coarse sets whole semitones, fine adds cents.
    fn control(&mut self, controller: u8, value: u8) {
        match controller {
            101 => self.selected = (self.selected & 0xff) | (u16::from(value) << 8),
            100 => self.selected = (self.selected & 0xff00) | u16::from(value),
            6 => {
                self.coarse = value;
                if self.selected == 0 {
                    self.range = f32::from(value);
                }
            }
            38 if self.selected == 0 => {
                self.range = f32::from(self.coarse) + f32::from(value) / 100.0;
            }
            _ => {}
        }
    }
}

/// Most notes accepted from one file.
const MAX_NOTES: usize = 100_000;

struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn byte(&mut self) -> Result<u8, String> {
        let b = *self.data.get(self.pos).ok_or("unexpected end of file")?;
        self.pos += 1;
        Ok(b)
    }

    fn bytes(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.pos.checked_add(n).ok_or("length overflow")?;
        let slice = self
            .data
            .get(self.pos..end)
            .ok_or("unexpected end of file")?;
        self.pos = end;
        Ok(slice)
    }

    fn u16(&mut self) -> Result<u16, String> {
        let b = self.bytes(2)?;
        Ok(u16::from_be_bytes([b[0], b[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let b = self.bytes(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }

    /// Variable-length quantity (at most four bytes).
    fn vlq(&mut self) -> Result<u64, String> {
        let mut value = 0_u64;
        for _ in 0..4 {
            let b = self.byte()?;
            value = (value << 7) | u64::from(b & 0x7f);
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
        Err("variable-length number is too long".to_owned())
    }
}

/// The earliest time signature seen: (time, numerator, denominator).
type FirstSig = Option<(u64, u32, u32)>;

fn read_track(
    data: &[u8],
    notes: &mut Vec<FileNote>,
    sig: &mut FirstSig,
    tempo: &mut Option<f64>,
    parts: &mut BTreeMap<u8, PartInfo>,
    bends: &mut BTreeMap<u8, Vec<(u64, f32)>>,
) -> Result<(), String> {
    let mut r = Reader { data, pos: 0 };
    let mut time = 0_u64;
    let mut running: Option<u8> = None;
    let mut rpns = [Rpn::default(); 16];
    // Sounding notes: (channel, key) → (start, velocity), oldest first.
    let mut open: Vec<(u8, u8, u64, u8)> = Vec::new();
    // This track's name (meta 0x03), kept for the channels that sound in it.
    let mut track_name: Option<String> = None;
    while r.pos < data.len() {
        time = time.saturating_add(r.vlq()?);
        let first = r.byte()?;
        let status = if first & 0x80 != 0 {
            first
        } else {
            // Running status: `first` is already the first data byte.
            r.pos -= 1;
            running.ok_or("data byte without a status")?
        };
        match status {
            0xff => {
                running = None;
                let kind = r.byte()?;
                let len = usize::try_from(r.vlq()?).map_err(|e| e.to_string())?;
                let body = r.bytes(len)?;
                if let (0x58, [num, power, ..]) = (kind, body)
                    && sig.is_none_or(|(at, ..)| time < at)
                {
                    let den = 1_u32.checked_shl(u32::from(*power)).unwrap_or(0);
                    *sig = Some((time, u32::from(*num), den));
                }
                if kind == 0x51
                    && let [high, middle, low] = body
                {
                    let us = (u32::from(*high) << 16) | (u32::from(*middle) << 8) | u32::from(*low);
                    if us > 0 && tempo.is_none() {
                        *tempo = Some(60_000_000.0 / f64::from(us));
                    }
                }
                if kind == 0x03 {
                    let name = String::from_utf8_lossy(body).trim().to_owned();
                    track_name = (!name.is_empty()).then_some(name);
                }
                if kind == 0x2f {
                    break;
                }
            }
            0xf0 | 0xf7 => {
                running = None;
                let len = usize::try_from(r.vlq()?).map_err(|e| e.to_string())?;
                r.bytes(len)?;
            }
            0xf1..=0xfe => return Err(format!("unexpected system message {status:#x}")),
            _ => {
                running = Some(status);
                let channel = status & 0x0f;
                match status & 0xf0 {
                    0x80 | 0x90 => {
                        let key = r.byte()? & 0x7f;
                        let velocity = r.byte()? & 0x7f;
                        if status & 0xf0 == 0x90 && velocity > 0 {
                            open.push((channel, key, time, velocity));
                            let part = parts.entry(channel).or_default();
                            if part.name.is_none() {
                                part.name = track_name.clone();
                            }
                        } else if let Some(i) = open
                            .iter()
                            .position(|(c, k, ..)| *c == channel && *k == key)
                        {
                            let (_, key, start, velocity) = open.remove(i);
                            notes.push(FileNote {
                                start,
                                length: (time - start).max(1),
                                key,
                                velocity,
                                channel,
                            });
                        }
                    }
                    0xb0 => {
                        let controller = r.byte()?;
                        let value = r.byte()?;
                        let part = parts.entry(channel).or_default();
                        match controller {
                            7 => part.volume = Some(value),
                            10 => part.pan = Some(value),
                            _ => {}
                        }
                        rpns[usize::from(channel)].control(controller, value);
                    }
                    0xc0 => {
                        let program = r.byte()?;
                        parts.entry(channel).or_default().program = Some(program);
                    }
                    0xa0 => {
                        r.bytes(2)?;
                    }
                    0xe0 => {
                        let lsb = r.byte()?;
                        let msb = r.byte()?;
                        let value = (u16::from(msb & 0x7f) << 7) | u16::from(lsb & 0x7f);
                        let range = rpns[usize::from(channel)].range;
                        bends
                            .entry(channel)
                            .or_default()
                            .push((time, bend_semitones(value, range)));
                    }
                    _ => {
                        r.bytes(1)?;
                    }
                }
            }
        }
        if notes.len() > MAX_NOTES {
            return Err(format!("more than {MAX_NOTES} notes"));
        }
    }
    // Notes still sounding at the end of the track stop there.
    for (midi_channel, key, start, velocity) in open {
        notes.push(FileNote {
            start,
            length: (time - start).max(1),
            key,
            velocity,
            channel: midi_channel,
        });
    }
    Ok(())
}

/// Parses a Standard MIDI File.
pub(crate) fn parse(data: &[u8]) -> Result<MidiFile, String> {
    let mut r = Reader { data, pos: 0 };
    if r.bytes(4)? != b"MThd" {
        return Err("not a MIDI file".to_owned());
    }
    let header_len = r.u32()? as usize;
    let format = r.u16()?;
    let tracks = r.u16()?;
    let division = r.u16()?;
    r.bytes(header_len.saturating_sub(6))?;
    if format > 1 {
        return Err(format!("MIDI file format {format} is not supported"));
    }
    if division & 0x8000 != 0 || division == 0 {
        return Err("SMPTE-timed MIDI files are not supported".to_owned());
    }
    let mut notes = Vec::new();
    let mut sig = None;
    let mut tempo = None;
    let mut parts = BTreeMap::new();
    let mut bends: BTreeMap<u8, Vec<(u64, f32)>> = BTreeMap::new();
    for _ in 0..tracks {
        if r.pos >= data.len() {
            break;
        }
        let kind = r.bytes(4)?;
        let len = r.u32()? as usize;
        let chunk = r.bytes(len)?;
        if kind == b"MTrk" {
            read_track(
                chunk, &mut notes, &mut sig, &mut tempo, &mut parts, &mut bends,
            )?;
        }
    }
    notes.sort_by_key(|n| (n.start, n.key));
    for points in bends.values_mut() {
        points.sort_by_key(|(tick, _)| *tick);
        points.dedup_by(|a, b| a.0 == b.0 || a.1 == b.1);
        if points.len() > MAX_BEND_POINTS {
            let stride = points.len().div_ceil(MAX_BEND_POINTS);
            let last = *points.last().expect("the bend has points");
            *points = points.iter().step_by(stride).copied().collect();
            if points.last() != Some(&last) {
                points.push(last);
            }
        }
    }
    Ok(MidiFile {
        ticks_per_quarter: division,
        notes,
        time_signature: sig.map(|(_, num, den)| (num, den)),
        tempo_bpm: tempo,
        parts,
        bends,
    })
}

/// The General MIDI program's instrument name (program change 0-127).
#[must_use]
pub(crate) fn gm_name(program: u8) -> &'static str {
    const NAMES: [&str; 128] = [
        "Grand Piano",
        "Bright Grand Piano",
        "Electric Grand",
        "Honky-tonk Piano",
        "Electric Piano 1",
        "Electric Piano 2",
        "Harpsichord",
        "Clavinet",
        "Celesta",
        "Glockenspiel",
        "Music Box",
        "Vibraphone",
        "Marimba",
        "Xylophone",
        "Tubular Bells",
        "Dulcimer",
        "Drawbar Organ",
        "Percussive Organ",
        "Rock Organ",
        "Church Organ",
        "Reed Organ",
        "Accordion",
        "Harmonica",
        "Tango Accordion",
        "Nylon Guitar",
        "Steel Guitar",
        "Jazz Guitar",
        "Electric Guitar",
        "Muted Guitar",
        "Overdriven Guitar",
        "Distortion Guitar",
        "Guitar Harmonics",
        "Acoustic Bass",
        "Finger Bass",
        "Pick Bass",
        "Fretless Bass",
        "Slap Bass 1",
        "Slap Bass 2",
        "Synth Bass 1",
        "Synth Bass 2",
        "Violin",
        "Viola",
        "Cello",
        "Contrabass",
        "Tremolo Strings",
        "Pizzicato Strings",
        "Orchestral Harp",
        "Timpani",
        "Strings",
        "Slow Strings",
        "Synth Strings 1",
        "Synth Strings 2",
        "Choir Aahs",
        "Voice Oohs",
        "Synth Choir",
        "Orchestra Hit",
        "Trumpet",
        "Trombone",
        "Tuba",
        "Muted Trumpet",
        "French Horn",
        "Brass Section",
        "Synth Brass 1",
        "Synth Brass 2",
        "Soprano Sax",
        "Alto Sax",
        "Tenor Sax",
        "Baritone Sax",
        "Oboe",
        "English Horn",
        "Bassoon",
        "Clarinet",
        "Piccolo",
        "Flute",
        "Recorder",
        "Pan Flute",
        "Bottle Blow",
        "Shakuhachi",
        "Whistle",
        "Ocarina",
        "Square Lead",
        "Saw Lead",
        "Calliope Lead",
        "Chiff Lead",
        "Charang Lead",
        "Voice Lead",
        "Fifths Lead",
        "Bass Lead",
        "New Age Pad",
        "Warm Pad",
        "Polysynth Pad",
        "Choir Pad",
        "Bowed Pad",
        "Metallic Pad",
        "Halo Pad",
        "Sweep Pad",
        "Ice Rain",
        "Soundtrack",
        "Crystal",
        "Atmosphere",
        "Brightness",
        "Goblins",
        "Echoes",
        "Sci-Fi",
        "Sitar",
        "Banjo",
        "Shamisen",
        "Koto",
        "Kalimba",
        "Bagpipe",
        "Fiddle",
        "Shanai",
        "Tinkle Bell",
        "Agogo",
        "Steel Drums",
        "Woodblock",
        "Taiko Drum",
        "Melodic Tom",
        "Synth Drum",
        "Reverse Cymbal",
        "Guitar Fret Noise",
        "Breath Noise",
        "Seashore",
        "Bird Tweet",
        "Telephone Ring",
        "Helicopter",
        "Applause",
        "Gunshot",
    ];
    NAMES[usize::from(program.min(127))]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Builds a format-0 file from raw track bytes.
    pub fn smf(division: u16, track: &[u8]) -> Vec<u8> {
        let mut out = b"MThd".to_vec();
        out.extend(6_u32.to_be_bytes());
        out.extend(0_u16.to_be_bytes());
        out.extend(1_u16.to_be_bytes());
        out.extend(division.to_be_bytes());
        out.extend(b"MTrk");
        out.extend((track.len() as u32).to_be_bytes());
        out.extend(track);
        out
    }

    /// A note on channel 1 and one on the drum channel 10, in one track.
    pub fn two_channel_track() -> Vec<u8> {
        vec![
            0x00, 0x90, 60, 100, // ch 1: C on
            0x81, 0x40, 0x80, 60, 0, // after 192 ticks: C off
            0x00, 0x99, 38, 100, // ch 10: drum on
            0x81, 0x40, 0x89, 38, 0, // after 192 ticks: drum off
            0x00, 0xff, 0x2f, 0x00, // end of track
        ]
    }

    /// A drum note on the unnamed part, then a track that says its name,
    /// its instrument, its volume and pan, and the file's tempo, and plays
    /// one note. Two MIDI channels, so the import opens both.
    pub fn named_track() -> Vec<u8> {
        let name = b"Melody";
        vec![
            0x00,
            0x99,
            38,
            100, // ch 10: drum on (before the track name)
            0x81,
            0x40,
            0x89,
            38,
            0, // after 192 ticks: drum off
            0x00,
            0xff,
            0x51,
            0x03,
            0x07,
            0xa1,
            0x20, // tempo 500000 µs → 120 BPM
            0x00,
            0xff,
            0x03,
            name.len() as u8,
            name[0],
            name[1],
            name[2],
            name[3],
            name[4],
            name[5], // track name "Melody"
            0x00,
            0xc0,
            40, // program 40: Violin
            0x00,
            0xb0,
            7,
            127, // channel volume full
            0x00,
            0xb0,
            10,
            127, // pan hard right
            0x00,
            0x90,
            60,
            100, // C on
            0x81,
            0x40,
            0x80,
            60,
            0, // after 192 ticks: C off
            0x00,
            0xff,
            0x2f,
            0x00, // end of track
        ]
    }

    /// C4 for one beat, then E4 and G4 together using running status and
    /// velocity-0 note-offs; a tempo meta event; and a sysex message.
    pub fn sample_track() -> Vec<u8> {
        vec![
            0x00, 0xff, 0x51, 0x03, 0x07, 0xa1, 0x20, // tempo
            0x00, 0x90, 60, 100, // C on
            0x81, 0x40, 0x80, 60, 0, // after 192 ticks: C off
            0x00, 0x90, 64, 90, // E on
            0x00, 67, 80, // running status: G on
            0x00, 0xf0, 0x02, 0x7e, 0xf7, // sysex
            0x60, 0x90, 64, 0, // after 96: E off (velocity 0)
            0x00, 67, 0, // G off
            0x00, 0xff, 0x2f, 0x00, // end of track
        ]
    }

    #[test]
    fn reads_notes_with_running_status_and_meta_events() {
        let file = parse(&smf(192, &sample_track())).unwrap();
        assert_eq!(file.ticks_per_quarter, 192);
        assert_eq!(
            file.notes,
            [
                FileNote {
                    start: 0,
                    length: 192,
                    key: 60,
                    velocity: 100,
                    channel: 0,
                },
                FileNote {
                    start: 192,
                    length: 96,
                    key: 64,
                    velocity: 90,
                    channel: 0,
                },
                FileNote {
                    start: 192,
                    length: 96,
                    key: 67,
                    velocity: 80,
                    channel: 0,
                },
            ]
        );
    }

    #[test]
    fn rejects_bad_files() {
        assert!(parse(b"RIFF....").is_err());
        assert!(parse(&smf(192, &[0x00, 0x90, 60])).is_err(), "truncated");
        assert!(parse(&smf(0xe728, &[])).is_err(), "SMPTE");
        assert!(
            parse(&smf(96, &[0x00, 60, 100])).is_err(),
            "data without status"
        );
    }

    #[test]
    fn unterminated_notes_end_with_the_track() {
        let file = parse(&smf(96, &[0x00, 0x90, 50, 64, 0x60, 0xff, 0x2f, 0x00])).unwrap();
        assert_eq!(file.notes[0].length, 96);
    }

    #[test]
    fn reads_pitch_bends_as_semitones_and_collapses_repeats() {
        let track = vec![
            0x00, 0x90, 60, 100, // C on
            0x00, 0xe0, 0x00, 0x60, // bend up a semitone (12288 of 16383)
            0x30, 0xe0, 0x00, 0x40, // after 48 ticks: back home
            0x30, 0xe0, 0x00, 0x40, // still home: a repeat, dropped
            0x00, 0xff, 0x2f, 0x00, // end of track
        ];
        let file = parse(&smf(96, &track)).unwrap();
        let bends = &file.bends[&0];
        assert_eq!(bends.len(), 2);
        assert_eq!(bends[0], (0, 1.0));
        assert_eq!(bends[1], (48, 0.0));
    }

    #[test]
    fn reads_the_channels_rpn_bend_range() {
        // RPN 0 (pitch-bend range) set to 12 semitones, then a full-up bend:
        // without the RPN the same bend is the General MIDI default ±2.
        let full_up = |range: f32| (16383.0 - 8192.0) / 8192.0 * range;
        let track = vec![
            0x00, 0xb0, 101, 0, // RPN MSB 0
            0x00, 0xb0, 100, 0, // RPN LSB 0
            0x00, 0xb0, 6, 12, // data entry coarse: ±12 semitones
            0x00, 0xb0, 38, 50, // data entry fine: half a semitone
            0x00, 0xe0, 0x7f, 0x7f, // bend full up
            0x00, 0xff, 0x2f, 0x00, // end of track
        ];
        let file = parse(&smf(96, &track)).unwrap();
        let (tick, semitones) = file.bends[&0][0];
        assert_eq!(tick, 0);
        assert!((semitones - full_up(12.5)).abs() < 1e-4, "{semitones}");

        // A different channel keeps the default range.
        let track = vec![
            0x00, 0xb1, 101, 0, // channel 1 selects RPN 0
            0x00, 0xb1, 100, 0, 0x00, 0xb1, 6, 7, 0x00, 0xe0, 0x7f,
            0x7f, // channel 0 bends full up: still ±2
            0x00, 0xe1, 0x7f, 0x7f, // channel 1 bends full up: ±7
            0x00, 0xff, 0x2f, 0x00,
        ];
        let file = parse(&smf(96, &track)).unwrap();
        let (_, home) = file.bends[&0][0];
        assert!((home - full_up(2.0)).abs() < 1e-4, "{home}");
        let (_, ranged) = file.bends[&1][0];
        assert!((ranged - full_up(7.0)).abs() < 1e-4, "{ranged}");
    }
}
