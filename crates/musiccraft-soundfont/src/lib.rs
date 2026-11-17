//! SoundFont 2 loading (`docs/spec/channel-rack.md`, "Soundfonts"): the
//! published SoundFont 2.04 file structure, an open standard, read from
//! scratch. A `.sf2` holds sample data and the tables that map keys and
//! velocities to it; a `.sf3` is the same structure with the samples
//! compressed as Ogg Vorbis. What a preset plays comes out as [`ZoneSpec`]s
//! (a sample index, its key and velocity ranges, its tuning), which the zone
//! sampler plays through `soundfont:` sample references.
//!
//! Runs on the main thread only (file bytes in, decoded samples out).

use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{CODEC_TYPE_VORBIS, DecoderOptions};
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// A loaded soundfont.
#[derive(Debug, Clone, Default)]
pub struct Soundfont {
    /// The `INAM` name, or the empty string when the file has none.
    name: String,
    presets: Vec<Preset>,
    /// Each preset's first bag index, parallel to `presets`.
    preset_first_bags: Vec<usize>,
    instruments: Vec<InstrumentHeader>,
    samples: Vec<SampleHeader>,
    /// The `smpl` chunk's bytes: 16-bit little-endian PCM, or an Ogg Vorbis
    /// bitstream per sample in a `.sf3`.
    sample_data: Vec<u8>,
    /// The preset bags: each entry's generator list starts here.
    preset_bags: Vec<usize>,
    preset_generators: Generators,
    /// The instrument bags: each entry's generator list starts here.
    instrument_bags: Vec<usize>,
    instrument_generators: Generators,
}

/// One preset (a program a channel can play).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preset {
    /// The preset's name.
    pub name: String,
    /// MIDI bank number.
    pub bank: u16,
    /// MIDI program number.
    pub program: u16,
}

/// One sample's place in the file and its native tuning.
#[derive(Debug, Clone)]
struct SampleHeader {
    start: usize,
    end: usize,
    /// The sustain loop, in frames from the sample's start (`0, 0` when the
    /// sample does not loop). The file stores the loop as offsets into the
    /// whole sample pool, so reading moves it back by the sample's start.
    loop_start: u32,
    loop_end: u32,
    sample_rate: u32,
    original_pitch: u8,
    pitch_correction: i8,
    /// The linked stereo partner (`stereoLeft` names `stereoRight` and the
    /// other way round); `None` for mono and ROM samples.
    link: Option<u16>,
    /// Whether the bytes are an Ogg Vorbis bitstream (a `.sf3` sample).
    compressed: bool,
}

#[derive(Debug, Clone)]
struct InstrumentHeader {
    first_bag: usize,
}

/// One sounding zone of a preset, flattened from the preset's and the
/// instruments' generator lists.
#[derive(Debug, Clone, PartialEq)]
pub struct ZoneSpec {
    /// The sample index the zone plays.
    pub sample: u16,
    /// Lowest and highest key that play it.
    pub low: u8,
    pub high: u8,
    /// Lowest and highest velocity (0–127) that play it.
    pub vel_low: u8,
    pub vel_high: u8,
    /// The key that plays the sample at its recorded pitch: the soundfont's
    /// root key with the zone's coarse and fine tuning folded in.
    pub root: f32,
    /// Whether the sample loops (the `sampleModes` loop bit).
    pub loops: bool,
    /// The sample's sustain loop, in frames; `loop_end` is exclusive and
    /// `0, 0` means no loop. Zones that loop hold their note for as long
    /// as it sounds, instead of running out of sample.
    pub loop_start: u32,
    pub loop_end: u32,
    /// The volume envelope the zone sounds through, in seconds and a
    /// 0..=1 sustain level: how long the note rises, falls to the sustain
    /// level and fades after the note-off. The file stores the times as
    /// timecents and the sustain as centibels of attenuation; a zone that
    /// sets nothing attacks and releases at once at full level.
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

/// A byte reader over a chunk's slice.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.data.len().saturating_sub(self.pos)
    }

    fn bytes(&mut self, count: usize) -> Result<&'a [u8], String> {
        let end = self.pos + count;
        if end > self.data.len() {
            return Err(format!(
                "the file ends {end} bytes into a {count}-byte field"
            ));
        }
        let slice = &self.data[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn skip(&mut self, count: usize) -> Result<(), String> {
        self.bytes(count).map(|_| ())
    }

    fn u16(&mut self) -> Result<u16, String> {
        let bytes = self.bytes(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, String> {
        let bytes = self.bytes(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }

    /// A fixed-width field's bytes, trailing NULs trimmed, lossy UTF-8.
    fn text(&mut self, width: usize) -> Result<String, String> {
        let bytes = self.bytes(width)?;
        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
        Ok(String::from_utf8_lossy(&bytes[..end]).into_owned())
    }
}

/// One `op amount` pair from a generator chunk.
type Generators = Vec<(u16, u16)>;

/// The generator operators a zone reads (`sfGenOper`, SoundFont 2.01 spec
/// §8.1.3, matching fluidsynth's `gen.h`).
const GEN_INSTRUMENT: u16 = 41;
const GEN_KEY_RANGE: u16 = 43;
const GEN_VEL_RANGE: u16 = 44;
const GEN_COARSE_TUNE: u16 = 51;
const GEN_FINE_TUNE: u16 = 52;
const GEN_SAMPLE_ID: u16 = 53;
const GEN_SAMPLE_MODES: u16 = 54;
const GEN_ROOT_KEY: u16 = 58;
/// The volume envelope's stages, in timecents (`1200·log2(seconds)`); the
/// sustain is an attenuation in centibels from full level. Players like
/// fluidsynth shape every note with these, so a preset's decay and release
/// are what make a guitar fade like a plucked string instead of ringing
/// until the note-off.
const GEN_ATTACK_VOL: u16 = 34;
const GEN_HOLD_VOL: u16 = 35;
const GEN_DECAY_VOL: u16 = 36;
const GEN_SUSTAIN_VOL: u16 = 37;
const GEN_RELEASE_VOL: u16 = 38;

/// A generator list entry's byte size.
const GEN_SIZE: usize = 4;
/// A bag entry's byte size (generator index, modulator index).
const BAG_SIZE: usize = 4;
/// A preset header's byte size: name, program, bank, bag index, three words.
const PRESET_SIZE: usize = 38;
/// An instrument header's byte size: name, bag index.
const INSTRUMENT_SIZE: usize = 22;
/// A sample header's byte size: name, two ranges, loop, rate, pitch, link,
/// type.
const SAMPLE_SIZE: usize = 46;
/// The `sampleType` bit that marks an Ogg Vorbis sample (`.sf3`).
const COMPRESSED: u16 = 0x10;
/// `sampleType` values for the two halves of a linked stereo pair.
const STEREO_LEFT: u16 = 1;
const STEREO_RIGHT: u16 = 2;

impl Soundfont {
    /// Parses `.sf2` or `.sf3` bytes.
    ///
    /// # Errors
    ///
    /// A string describing the first structural problem: not a `sfbk` file,
    /// a truncated chunk, or missing tables.
    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let mut reader = Reader::new(bytes);
        expect(&mut reader, b"RIFF")?;
        let _size = reader.u32()?;
        expect(&mut reader, b"sfbk")?;
        let mut font = Self::default();
        while reader.remaining() >= 8 {
            let mut id = [0u8; 4];
            id.copy_from_slice(reader.bytes(4)?);
            let size = reader.u32()? as usize;
            let body = reader.bytes(size)?;
            if &id == b"LIST" {
                font.list(body)?;
            }
        }
        if font.presets.is_empty() {
            return Err("the file has no preset table (pdta/phdr)".to_owned());
        }
        if font.samples.is_empty() {
            return Err("the file has no sample table (pdta/shdr)".to_owned());
        }
        Ok(font)
    }

    /// The `INAM` name, or the empty string when the file has none.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The presets, in file order (the terminal EOP record is left off).
    #[must_use]
    pub fn presets(&self) -> &[Preset] {
        &self.presets
    }

    /// The preset's name for `index` (0-based).
    #[must_use]
    pub fn preset_name(&self, index: usize) -> &str {
        self.presets
            .get(index)
            .map_or("", |preset| preset.name.as_str())
    }

    /// The sounding zones of preset `index` (0-based, [`presets`](Self::presets)
    /// order), flattened: the preset's generators under the instruments'.
    ///
    /// # Errors
    ///
    /// An unknown preset index or a broken generator table.
    pub fn preset_zones(&self, index: usize) -> Result<Vec<ZoneSpec>, String> {
        // The preset's bags run to the next header's, or to the table's end
        // (the terminal EOP record closes the last preset).
        let next_bag = self
            .preset_first_bags
            .get(index + 1)
            .copied()
            .unwrap_or(self.preset_bags.len());
        if index >= self.presets.len() {
            return Err(format!("there is no preset {index}"));
        }
        let first_bag = self.preset_first_bags[index];
        // The first bag without an instrument is the preset's global zone:
        // its generators back every later zone of the preset.
        let mut global: Option<Generators> = None;
        let mut zones = Vec::new();
        for bag in first_bag..next_bag.min(self.preset_bags.len()) {
            let generators = self.preset_zone_generators(bag, next_bag);
            let Some(instrument) = generator(&generators, GEN_INSTRUMENT) else {
                if global.is_none() {
                    global = Some(generators);
                }
                continue;
            };
            let instrument = instrument as usize;
            let Some(instrument_header) = self.instruments.get(instrument) else {
                return Err(format!(
                    "preset zone names instrument {instrument}, which does not exist"
                ));
            };
            let next_instrument_bag = self
                .instruments
                .get(instrument + 1)
                .map_or(self.instrument_bags.len(), |header| header.first_bag);
            let mut instrument_global: Option<Generators> = None;
            for bag in
                instrument_header.first_bag..next_instrument_bag.min(self.instrument_bags.len())
            {
                let local = self.instrument_zone_generators(bag, next_instrument_bag);
                let Some(sample) = generator(&local, GEN_SAMPLE_ID) else {
                    if instrument_global.is_none() {
                        instrument_global = Some(local);
                    }
                    continue;
                };
                let zone = self.zone(
                    global.as_ref(),
                    &generators,
                    instrument_global.as_ref(),
                    &local,
                    sample,
                );
                if let Some(zone) = zone {
                    zones.push(zone);
                }
            }
        }
        Ok(zones)
    }

    /// One sample as mono floating point at its native rate: 16-bit PCM from
    /// a `.sf2`, Ogg Vorbis decoded from a `.sf3`.
    ///
    /// # Errors
    ///
    /// An unknown sample index, bytes outside the sample data, or a bitstream
    /// that does not decode.
    pub fn sample_mono(&self, index: u16) -> Result<(Vec<f32>, u32), String> {
        let header = self
            .samples
            .get(index as usize)
            .ok_or_else(|| format!("there is no sample {index}"))?;
        // A PCM sample's start and end count 16-bit words in the sample
        // data; a compressed (`.sf3`) sample's count bytes.
        let bytes = if header.compressed {
            self.sample_data.get(header.start..header.end)
        } else {
            self.sample_data
                .get(header.start.saturating_mul(2)..header.end.saturating_mul(2))
        }
        .ok_or_else(|| format!("sample {index} points outside the sample data"))?;
        if header.compressed {
            decode_ogg(bytes)
        } else {
            let samples = bytes
                .chunks_exact(2)
                .map(|pair| i16::from_le_bytes([pair[0], pair[1]]) as f32 / f32::from(i16::MAX))
                .collect();
            Ok((samples, header.sample_rate))
        }
    }

    /// One sample as a stereo pair at its native rate. A linked stereo sample
    /// joins with its partner; anything else is duplicated to both channels.
    ///
    /// # Errors
    ///
    /// As [`sample_mono`](Self::sample_mono), or a stereo partner that does
    /// not exist or decodes to another rate.
    pub fn sample_stereo(&self, index: u16) -> Result<(Vec<f32>, Vec<f32>, u32), String> {
        let header = self
            .samples
            .get(index as usize)
            .ok_or_else(|| format!("there is no sample {index}"))?;
        if let Some(link) = header.link
            && !header.compressed
            && self
                .samples
                .get(link as usize)
                .is_some_and(|partner| partner.link == Some(index) && !partner.compressed)
        {
            let (left, left_rate) = self.sample_mono(index)?;
            let (right, right_rate) = self.sample_mono(link)?;
            if left_rate != right_rate {
                return Err(format!(
                    "stereo samples {index} and {link} run at {left_rate} and {right_rate} Hz"
                ));
            }
            return Ok((left, right, left_rate));
        }
        let (mono, rate) = self.sample_mono(index)?;
        Ok((mono.clone(), mono, rate))
    }

    /// One flattened zone: the preset's and the instrument's generators
    /// combined (local over global, instrument over preset), ranges
    /// intersected, tuning folded into the root key.
    fn zone(
        &self,
        preset_global: Option<&Generators>,
        preset_local: &Generators,
        instrument_global: Option<&Generators>,
        instrument_local: &Generators,
        sample: u16,
    ) -> Option<ZoneSpec> {
        // Later tables in the lookup override earlier ones, so the search
        // order is instrument local, instrument global, preset local,
        // preset global.
        let pick = |op: u16| {
            instrument_local
                .iter()
                .chain(instrument_global.into_iter().flatten())
                .chain(preset_local)
                .chain(preset_global.into_iter().flatten())
                .find(|(candidate, _)| *candidate == op)
                .map(|(_, amount)| *amount)
        };
        let header = self.samples.get(sample as usize)?;
        let (mut low, mut high) = range(pick(GEN_KEY_RANGE));
        let (mut vel_low, mut vel_high) = range(pick(GEN_VEL_RANGE));
        // Preset-level ranges narrow the instrument's: both have to match.
        for (op, low_out, high_out) in [
            (GEN_KEY_RANGE, &mut low, &mut high),
            (GEN_VEL_RANGE, &mut vel_low, &mut vel_high),
        ] {
            if let Some(amount) = preset_local
                .iter()
                .chain(preset_global.into_iter().flatten())
                .find(|(candidate, _)| *candidate == op)
                .map(|(_, amount)| *amount)
            {
                let (other_low, other_high) = range(Some(amount));
                *low_out = (*low_out).max(other_low);
                *high_out = (*high_out).min(other_high);
            }
        }
        if low > high || vel_low > vel_high {
            return None;
        }
        // The sounding pitch: the root key (or the sample's own), moved by
        // the coarse and fine tuning and the sample's correction.
        let root_key = pick(GEN_ROOT_KEY)
            .filter(|root| *root <= 127)
            .unwrap_or_else(|| u16::from(header.original_pitch));
        let tune = f32::from(pick(GEN_COARSE_TUNE).unwrap_or(0) as i16)
            + f32::from(pick(GEN_FINE_TUNE).unwrap_or(0) as i16) / 100.0
            + f32::from(header.pitch_correction) / 100.0;
        let loops = pick(GEN_SAMPLE_MODES).is_some_and(|modes| modes & 1 == 1);
        // The volume envelope: timecents are seconds (`2^(tc/1200)`, at or
        // below -12000 counts as instant), the sustain is centibels of
        // attenuation from full level. The hold stage (time at full before
        // the decay) folds into the decay, which every player's envelope
        // then spends falling to the sustain level.
        let seconds = |op: u16| {
            pick(op).map(|amount| {
                let timecents = amount as i16;
                if timecents <= -12_000 {
                    0.0
                } else {
                    2.0_f32.powf(f32::from(timecents) / 1200.0)
                }
            })
        };
        let attack = seconds(GEN_ATTACK_VOL).unwrap_or(0.0);
        let hold = seconds(GEN_HOLD_VOL).unwrap_or(0.0);
        let decay = seconds(GEN_DECAY_VOL).unwrap_or(0.0) + hold;
        let sustain = pick(GEN_SUSTAIN_VOL)
            .map(|centibels| 10.0_f32.powf(-f32::from(centibels as i16) / 200.0))
            .unwrap_or(1.0)
            .clamp(0.0, 1.0);
        let release = seconds(GEN_RELEASE_VOL).unwrap_or(0.0);
        Some(ZoneSpec {
            sample,
            low,
            high,
            vel_low,
            vel_high,
            root: f32::from(root_key as i8) - tune,
            loops,
            loop_start: if loops { header.loop_start } else { 0 },
            loop_end: if loops { header.loop_end } else { 0 },
            attack,
            decay,
            sustain,
            release,
        })
    }

    /// The generators of preset bag `bag`, which run to the next bag's start
    /// (or the table's end for the last one).
    fn preset_zone_generators(&self, bag: usize, next_bag: usize) -> Generators {
        zone_generators(
            &self.preset_generators,
            self.preset_bags.get(bag).copied(),
            self.preset_bags.get(bag + 1).copied().unwrap_or(next_bag),
        )
    }

    /// The generators of instrument bag `bag`.
    fn instrument_zone_generators(&self, bag: usize, next_bag: usize) -> Generators {
        zone_generators(
            &self.instrument_generators,
            self.instrument_bags.get(bag).copied(),
            self.instrument_bags
                .get(bag + 1)
                .copied()
                .unwrap_or(next_bag),
        )
    }

    /// A `LIST` chunk: `INFO`, `sdta` or `pdta`.
    fn list(&mut self, body: &[u8]) -> Result<(), String> {
        let Some((kind, body)) = body.split_at_checked(4) else {
            return Err("a LIST chunk ends early".to_owned());
        };
        let mut reader = Reader::new(body);
        while reader.remaining() >= 8 {
            let mut id = [0u8; 4];
            id.copy_from_slice(reader.bytes(4)?);
            let size = reader.u32()? as usize;
            let chunk = reader.bytes(size)?;
            match (kind, &id) {
                (b"INFO", b"INAM") => {
                    self.name = String::from_utf8_lossy(chunk)
                        .trim_end_matches('\0')
                        .to_owned();
                }
                (b"sdta", b"smpl") => self.sample_data = chunk.to_vec(),
                (b"pdta", b"phdr") => self.preset_headers(chunk)?,
                (b"pdta", b"pbag") => {
                    self.preset_bags = bag_table(chunk)?;
                }
                (b"pdta", b"pgen") => self.preset_generators = generator_table(chunk)?,
                (b"pdta", b"inst") => self.instrument_headers(chunk)?,
                (b"pdta", b"ibag") => {
                    self.instrument_bags = bag_table(chunk)?;
                }
                (b"pdta", b"igen") => self.instrument_generators = generator_table(chunk)?,
                (b"pdta", b"shdr") => self.sample_headers(chunk)?,
                _ => {}
            }
        }
        Ok(())
    }

    /// The `phdr` records; the terminal EOP record is left off.
    fn preset_headers(&mut self, chunk: &[u8]) -> Result<(), String> {
        let mut reader = Reader::new(chunk);
        let mut presets = Vec::new();
        while reader.remaining() >= PRESET_SIZE {
            let name = reader.text(20)?;
            let program = reader.u16()?;
            let bank = reader.u16()?;
            let first_bag = reader.u16()? as usize;
            reader.skip(12)?;
            if name == "EOP" {
                break;
            }
            presets.push(Preset {
                name,
                bank,
                program,
            });
            self.preset_first_bags.push(first_bag);
        }
        self.presets = presets;
        Ok(())
    }

    /// The `inst` records (the terminal record is harmless to keep: nothing
    /// names it).
    fn instrument_headers(&mut self, chunk: &[u8]) -> Result<(), String> {
        let mut reader = Reader::new(chunk);
        let mut headers = Vec::new();
        while reader.remaining() >= INSTRUMENT_SIZE {
            let _name = reader.text(20)?;
            let first_bag = reader.u16()? as usize;
            headers.push(InstrumentHeader { first_bag });
        }
        self.instruments = headers;
        Ok(())
    }

    /// The `shdr` records (the terminal record has zero ranges and no type).
    fn sample_headers(&mut self, chunk: &[u8]) -> Result<(), String> {
        let mut reader = Reader::new(chunk);
        let mut samples = Vec::new();
        while reader.remaining() >= SAMPLE_SIZE {
            let _name = reader.text(20)?;
            let start = reader.u32()? as usize;
            let end = reader.u32()? as usize;
            if start == 0 && end == 0 {
                // The terminal record: an empty sample nothing names.
                break;
            }
            let loop_start = reader.u32()?;
            let loop_end = reader.u32()?;
            let sample_rate = reader.u32()?;
            let length = end.saturating_sub(start) as u32;
            let first = start as u32;
            let loop_start = loop_start.saturating_sub(first).min(length);
            let loop_end = loop_end.saturating_sub(first).min(length);
            let original_pitch = reader.bytes(1)?[0];
            let pitch_correction = reader.bytes(1)?[0] as i8;
            let link = reader.u16()?;
            let kind = reader.u16()?;
            let base = kind & !COMPRESSED;
            samples.push(SampleHeader {
                start,
                end,
                loop_start,
                loop_end,
                sample_rate,
                original_pitch,
                pitch_correction,
                link: (base == STEREO_LEFT || base == STEREO_RIGHT).then_some(link),
                compressed: kind & COMPRESSED != 0,
            });
        }
        self.samples = samples;
        Ok(())
    }
}

/// Reads the four-byte tag `reader` is on.
fn expect(reader: &mut Reader<'_>, tag: &[u8; 4]) -> Result<(), String> {
    let found = reader.bytes(4)?;
    if found == tag {
        Ok(())
    } else {
        Err(format!(
            "expected {}, found {}",
            String::from_utf8_lossy(tag),
            String::from_utf8_lossy(found)
        ))
    }
}

/// A range generator's amount: a low and a high byte.
fn range(amount: Option<u16>) -> (u8, u8) {
    match amount {
        Some(amount) => (amount as u8, (amount >> 8) as u8),
        None => (0, 127),
    }
}

/// One bag's generator: `op`, or `None` when the bag does not set it.
fn generator(generators: &Generators, op: u16) -> Option<u16> {
    generators
        .iter()
        .find(|(candidate, _)| *candidate == op)
        .map(|(_, amount)| *amount)
}

/// The generators of one bag: from its start index to the next bag's.
fn zone_generators(table: &Generators, start: Option<usize>, end: usize) -> Generators {
    let Some(start) = start else {
        return Vec::new();
    };
    let end = end.min(table.len());
    if start >= end {
        return Vec::new();
    }
    table[start..end].to_vec()
}

/// A bag table: each record's generator index.
fn bag_table(chunk: &[u8]) -> Result<Vec<usize>, String> {
    let mut reader = Reader::new(chunk);
    let mut bags = Vec::new();
    while reader.remaining() >= BAG_SIZE {
        let generators = reader.u16()? as usize;
        reader.skip(2)?;
        bags.push(generators);
    }
    Ok(bags)
}

/// A generator chunk: `op amount` pairs.
fn generator_table(chunk: &[u8]) -> Result<Generators, String> {
    let mut reader = Reader::new(chunk);
    let mut generators = Vec::new();
    while reader.remaining() >= GEN_SIZE {
        let op = reader.u16()?;
        let amount = reader.u16()?;
        generators.push((op, amount));
    }
    Ok(generators)
}

/// An Ogg Vorbis bitstream as mono floating point at its rate (the channels
/// averaged).
fn decode_ogg(bytes: &[u8]) -> Result<(Vec<f32>, u32), String> {
    let source = MediaSourceStream::new(
        Box::new(std::io::Cursor::new(bytes.to_vec())),
        Default::default(),
    );
    let mut hint = Hint::new();
    hint.with_extension("ogg");
    let mut format = symphonia::default::get_probe()
        .format(
            &hint,
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("the sample is not decodable Ogg Vorbis: {e}"))?
        .format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec == CODEC_TYPE_VORBIS)
        .ok_or("the Ogg stream holds no Vorbis audio")?;
    let track_id = track.id;
    let rate = track
        .codec_params
        .sample_rate
        .ok_or("the Ogg stream names no sample rate")?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("the Vorbis decoder did not start: {e}"))?;
    let mut samples = Vec::new();
    while let Ok(packet) = format.next_packet() {
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = decoder
            .decode(&packet)
            .map_err(|e| format!("the Vorbis stream breaks: {e}"))?;
        let spec = *decoded.spec();
        let mut buffer = SampleBuffer::<f32>::new(decoded.capacity() as u64, spec);
        buffer.copy_interleaved_ref(decoded);
        let channels = spec.channels.count().max(1);
        for frame in buffer.samples().chunks(channels) {
            let sum: f32 = frame.iter().sum();
            samples.push(sum / channels as f32);
        }
    }
    Ok((samples, rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Puts one RIFF chunk (`id`, `body`) into `out`.
    fn chunk(out: &mut Vec<u8>, id: &[u8; 4], body: &[u8]) {
        out.extend_from_slice(id);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(body);
    }

    /// Puts one `LIST` chunk of sub-chunks into `out`.
    fn list(out: &mut Vec<u8>, kind: &[u8; 4], sub: &[u8]) {
        let mut body = kind.to_vec();
        body.extend_from_slice(sub);
        chunk(out, b"LIST", &body);
    }

    /// A preset header record.
    fn preset_record(name: &str, program: u16, bank: u16, bag: u16) -> Vec<u8> {
        let mut record = vec![0u8; 20];
        record[..name.len()].copy_from_slice(name.as_bytes());
        record.extend_from_slice(&program.to_le_bytes());
        record.extend_from_slice(&bank.to_le_bytes());
        record.extend_from_slice(&bag.to_le_bytes());
        record.extend_from_slice(&[0u8; 12]);
        record
    }

    /// An instrument header record.
    fn instrument_record(name: &str, bag: u16) -> Vec<u8> {
        let mut record = vec![0u8; 20];
        record[..name.len()].copy_from_slice(name.as_bytes());
        record.extend_from_slice(&bag.to_le_bytes());
        record
    }

    /// A sample header record.
    #[allow(clippy::too_many_arguments)]
    fn sample_record(
        name: &str,
        start: u32,
        end: u32,
        loop_start: u32,
        loop_end: u32,
        rate: u32,
        pitch: u8,
        correction: i8,
        link: u16,
        kind: u16,
    ) -> Vec<u8> {
        let mut record = vec![0u8; 20];
        record[..name.len()].copy_from_slice(name.as_bytes());
        for value in [start, end, loop_start, loop_end, rate] {
            record.extend_from_slice(&value.to_le_bytes());
        }
        record.push(pitch);
        record.push(correction.to_le_bytes()[0]);
        record.extend_from_slice(&link.to_le_bytes());
        record.extend_from_slice(&kind.to_le_bytes());
        assert_eq!(record.len(), SAMPLE_SIZE);
        record
    }

    /// A generator pair.
    fn generator_pair(op: u16, amount: u16) -> Vec<u8> {
        let mut bytes = op.to_le_bytes().to_vec();
        bytes.extend_from_slice(&amount.to_le_bytes());
        bytes
    }

    /// A bag pair (generator index, modulator index).
    fn bag(generators: u16) -> Vec<u8> {
        let mut bytes = generators.to_le_bytes().to_vec();
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes
    }

    /// A two-second sine at `rate`, 16-bit PCM.
    fn tone_pcm(rate: u32, hertz: f32) -> Vec<u8> {
        let frames = (rate as usize) / 100;
        (0..frames)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let value = (std::f32::consts::TAU * hertz * t).sin() * 0.5;
                (value * f32::from(i16::MAX)) as i16
            })
            .flat_map(|value| value.to_le_bytes())
            .collect()
    }

    /// A minimal mono `.sf2`: one preset, one instrument, one sample over
    /// keys 36–84, velocities 1–127, root key 60, tuned down an octave.
    fn mono_font(pcm: &[u8], compressed: bool) -> Vec<u8> {
        mono_font_with(pcm, compressed, None, &[])
    }

    /// The same font with a `sampleModes` generator and extra instrument
    /// generators, for the loop and envelope tests.
    fn mono_font_with(
        pcm: &[u8],
        compressed: bool,
        modes: Option<u16>,
        extra: &[(u16, u16)],
    ) -> Vec<u8> {
        let mut file = Vec::new();
        chunk(&mut file, b"RIFF", b"sfbk");
        let mut info = Vec::new();
        chunk(&mut info, b"ifil", &[2, 0, 4, 0]);
        chunk(&mut info, b"INAM", b"Test font\0");
        let mut sdta = Vec::new();
        chunk(&mut sdta, b"smpl", pcm);
        let kind = if compressed { 0x10 } else { 0 };
        let mut pdta = Vec::new();
        let mut phdr = preset_record("Grand", 0, 0, 0);
        phdr.extend(preset_record("EOP", 0, 0, 1));
        chunk(&mut pdta, b"phdr", &phdr);
        let mut pbag = bag(0);
        pbag.extend(bag(2));
        chunk(&mut pdta, b"pbag", &pbag);
        let mut pgen = generator_pair(GEN_INSTRUMENT, 0);
        pgen.extend(generator_pair(GEN_KEY_RANGE, 36 | (84 << 8)));
        chunk(&mut pdta, b"pgen", &pgen);
        let mut inst = instrument_record("Piano", 0);
        inst.extend(instrument_record("", 2));
        chunk(&mut pdta, b"inst", &inst);
        let mut ibag = bag(0);
        ibag.extend(bag(3 + u16::from(modes.is_some()) + extra.len() as u16));
        chunk(&mut pdta, b"ibag", &ibag);
        let mut igen = generator_pair(GEN_SAMPLE_ID, 0);
        igen.extend(generator_pair(GEN_ROOT_KEY, 60));
        igen.extend(generator_pair(GEN_COARSE_TUNE, (-12i16) as u16));
        if let Some(modes) = modes {
            igen.extend(generator_pair(GEN_SAMPLE_MODES, modes));
        }
        for (op, amount) in extra {
            igen.extend(generator_pair(*op, *amount));
        }
        chunk(&mut pdta, b"igen", &igen);
        let end = if compressed {
            pcm.len() as u32
        } else {
            pcm.len() as u32 / 2
        };
        let mut shdr = sample_record("Tone", 0, end, 0, end, 44_100, 60, 0, 0, kind);
        shdr.extend(sample_record("", 0, 0, 0, 0, 0, 0, 0, 0, 0));
        chunk(&mut pdta, b"shdr", &shdr);
        list(&mut file, b"INFO", &info);
        list(&mut file, b"sdta", &sdta);
        list(&mut file, b"pdta", &pdta);
        // The RIFF size covers everything after the size field.
        let size = (file.len() - 8) as u32;
        file[4..8].copy_from_slice(&size.to_le_bytes());
        file
    }

    #[test]
    fn a_minimal_font_parses_into_its_preset_and_zone() {
        let pcm = tone_pcm(44_100, 440.0);
        let font = Soundfont::parse(&mono_font(&pcm, false)).unwrap();
        assert_eq!(font.name(), "Test font");
        assert_eq!(
            font.presets(),
            &[Preset {
                name: "Grand".to_owned(),
                bank: 0,
                program: 0
            }]
        );
        let zones = font.preset_zones(0).unwrap();
        assert_eq!(zones.len(), 1);
        let zone = &zones[0];
        assert_eq!(zone.sample, 0);
        assert_eq!((zone.low, zone.high), (36, 84));
        assert_eq!((zone.vel_low, zone.vel_high), (0, 127));
        assert!(
            (zone.root - 72.0).abs() < 1e-4,
            "root 60 tuned down an octave plays from 72"
        );
        assert!(!zone.loops);
        assert_eq!((zone.loop_start, zone.loop_end), (0, 0));
    }

    #[test]
    fn a_looping_zone_carries_the_samples_loop() {
        let pcm = tone_pcm(44_100, 440.0);
        let frames = pcm.len() as u32 / 2;
        let font = Soundfont::parse(&mono_font_with(&pcm, false, Some(1), &[])).unwrap();
        let zones = font.preset_zones(0).unwrap();
        assert_eq!(zones.len(), 1);
        let zone = &zones[0];
        assert!(zone.loops);
        assert_eq!((zone.loop_start, zone.loop_end), (0, frames));
        // A loop switch-off leaves no loop behind.
        let off = Soundfont::parse(&mono_font_with(&pcm, false, Some(0), &[])).unwrap();
        let zone = &off.preset_zones(0).unwrap()[0];
        assert!(!zone.loops);
        assert_eq!((zone.loop_start, zone.loop_end), (0, 0));
    }

    #[test]
    fn a_zone_reads_the_preset_volume_envelope() {
        let pcm = tone_pcm(44_100, 440.0);
        // A plucked-string envelope: a half-second attack, a hold then a
        // decay to near silence (-200 cB) and a short release, stored as
        // timecents and centibels.
        let font = Soundfont::parse(&mono_font_with(
            &pcm,
            false,
            Some(1),
            &[
                (GEN_ATTACK_VOL, (-1_200i16) as u16),
                (GEN_HOLD_VOL, (-1_200i16) as u16),
                (GEN_DECAY_VOL, (-2_400i16) as u16),
                (GEN_SUSTAIN_VOL, 200),
                (GEN_RELEASE_VOL, (-3_600i16) as u16),
            ],
        ))
        .unwrap();
        let zone = &font.preset_zones(0).unwrap()[0];
        assert!((zone.attack - 0.5).abs() < 1e-4);
        assert!(
            (zone.decay - 0.75).abs() < 1e-4,
            "hold folds into the decay"
        );
        assert!((zone.sustain - 0.1).abs() < 1e-4);
        assert!((zone.release - 0.125).abs() < 1e-4);
        // Times at or below -12000 timecents count as instant, and a font
        // that sets nothing attacks and releases at once at full level.
        let instant = Soundfont::parse(&mono_font_with(
            &pcm,
            false,
            Some(1),
            &[(GEN_ATTACK_VOL, (-24_000i16) as u16)],
        ))
        .unwrap();
        let zone = &instant.preset_zones(0).unwrap()[0];
        assert_eq!(zone.attack, 0.0);
        assert_eq!((zone.decay, zone.sustain, zone.release), (0.0, 1.0, 0.0));
    }

    #[test]
    fn a_loop_is_relative_to_the_samples_start() {
        // The file stores the loop as offsets into the whole sample pool;
        // a font whose sample sits deep in the pool (like TimGM6mb's) must
        // still read its loop from its own start.
        let pcm = tone_pcm(44_100, 440.0);
        let frames = pcm.len() as u32 / 2;
        let offset = 1_000_000;
        let mut file = Vec::new();
        chunk(&mut file, b"RIFF", b"sfbk");
        let mut info = Vec::new();
        chunk(&mut info, b"ifil", &[2, 0, 4, 0]);
        chunk(&mut info, b"INAM", b"Pool font\0");
        let mut sdta = Vec::new();
        let mut smpl = vec![0u8; usize::try_from(offset).unwrap() * 2];
        smpl.extend_from_slice(&pcm);
        chunk(&mut sdta, b"smpl", &smpl);
        let mut pdta = Vec::new();
        let mut phdr = preset_record("Grand", 0, 0, 0);
        phdr.extend(preset_record("EOP", 0, 0, 1));
        chunk(&mut pdta, b"phdr", &phdr);
        let mut pbag = bag(0);
        pbag.extend(bag(2));
        chunk(&mut pdta, b"pbag", &pbag);
        let mut pgen = generator_pair(GEN_INSTRUMENT, 0);
        pgen.extend(generator_pair(GEN_KEY_RANGE, 36 | (84 << 8)));
        chunk(&mut pdta, b"pgen", &pgen);
        let mut inst = instrument_record("Piano", 0);
        inst.extend(instrument_record("", 2));
        chunk(&mut pdta, b"inst", &inst);
        let mut ibag = bag(0);
        ibag.extend(bag(4));
        chunk(&mut pdta, b"ibag", &ibag);
        let mut igen = generator_pair(GEN_SAMPLE_ID, 0);
        igen.extend(generator_pair(GEN_ROOT_KEY, 60));
        igen.extend(generator_pair(GEN_SAMPLE_MODES, 1));
        chunk(&mut pdta, b"igen", &igen);
        let mut shdr = sample_record(
            "Tone",
            offset,
            offset + frames,
            offset + frames / 4,
            offset + frames / 2,
            44_100,
            60,
            0,
            0,
            0,
        );
        shdr.extend(sample_record("", 0, 0, 0, 0, 0, 0, 0, 0, 0));
        chunk(&mut pdta, b"shdr", &shdr);
        list(&mut file, b"INFO", &info);
        list(&mut file, b"sdta", &sdta);
        list(&mut file, b"pdta", &pdta);
        let size = (file.len() - 8) as u32;
        file[4..8].copy_from_slice(&size.to_le_bytes());

        let font = Soundfont::parse(&file).unwrap();
        let zone = &font.preset_zones(0).unwrap()[0];
        assert!(zone.loops);
        assert_eq!(
            (zone.loop_start, zone.loop_end),
            (frames / 4, frames / 2),
            "the loop counts from the sample's own start"
        );
    }

    #[test]
    fn samples_decode_as_pcm_and_ogg() {
        let pcm = tone_pcm(44_100, 440.0);
        let plain = Soundfont::parse(&mono_font(&pcm, false)).unwrap();
        let (samples, rate) = plain.sample_mono(0).unwrap();
        assert_eq!(rate, 44_100);
        assert_eq!(samples.len(), pcm.len() / 2);
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!((0.4..0.6).contains(&peak), "peak {peak}");
        let (left, right, rate) = plain.sample_stereo(0).unwrap();
        assert_eq!((left, right, rate), (samples.clone(), samples, 44_100));

        let (encoded, ogg_rate) = encode_ogg_tone(44_100, 440.0);
        let sf3 = Soundfont::parse(&mono_font(&encoded, true)).unwrap();
        let (samples, rate) = sf3.sample_mono(0).unwrap();
        assert_eq!(rate, ogg_rate);
        assert!(
            samples.len() > ogg_rate as usize / 4,
            "{} frames",
            samples.len()
        );
        let peak = samples.iter().fold(0.0f32, |p, s| p.max(s.abs()));
        assert!((0.2..0.8).contains(&peak), "peak {peak}");
    }

    /// A one-second tone as Ogg Vorbis bytes and its rate.
    fn encode_ogg_tone(rate: u32, hertz: f32) -> (Vec<u8>, u32) {
        use std::num::{NonZeroU8, NonZeroU32};
        use vorbis_rs::{VorbisBitrateManagementStrategy, VorbisEncoderBuilder};
        let samples: Vec<f32> = (0..rate as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (std::f32::consts::TAU * hertz * t).sin() * 0.5
            })
            .collect();
        let mut encoder = VorbisEncoderBuilder::new_with_serial(
            NonZeroU32::new(rate).unwrap(),
            NonZeroU8::new(1).unwrap(),
            Vec::new(),
            1,
        );
        encoder.bitrate_management_strategy(VorbisBitrateManagementStrategy::QualityVbr {
            target_quality: 0.6,
        });
        let mut encoder = encoder.build().unwrap();
        encoder.encode_audio_block([&samples]).unwrap();
        (encoder.finish().unwrap(), rate)
    }

    #[test]
    fn broken_files_are_refused_with_a_reason() {
        assert!(Soundfont::parse(b"RIFF").is_err());
        let mut file = mono_font(&tone_pcm(44_100, 440.0), false);
        file[8..12].copy_from_slice(b"JUNK");
        assert!(Soundfont::parse(&file).is_err());
    }

    #[test]
    fn unknown_preset_and_sample_indices_are_errors() {
        let pcm = tone_pcm(44_100, 440.0);
        let font = Soundfont::parse(&mono_font(&pcm, false)).unwrap();
        assert!(font.preset_zones(1).is_err());
        assert!(font.sample_mono(1).is_err());
        assert_eq!(font.preset_name(5), "");
    }
}
