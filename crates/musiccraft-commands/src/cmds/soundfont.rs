//! Soundfonts: loading a `.sf2`/`.sf3` preset's zones into a zone sampler
//! (`docs/spec/channel-rack.md`, "Soundfonts"). The zones' samples are
//! `soundfont:` references the sample cache decodes out of the file, so the
//! project keeps working without the soundfont's samples as loose files.

use musiccraft_project::{Channel, ChannelId, Instrument, Project, SampleZone};
use musiccraft_soundfont::Soundfont;

use super::channel::{channel_arg, push_channel};
use super::zones::zones_mut;
use crate::args::Args;
use crate::command::cmd;
use crate::{Command, CommandError, Invocation, Outcome};

pub(super) fn commands() -> Vec<Box<dyn Command>> {
    vec![
        cmd(
            "channel.add_soundfont",
            "channel.add_soundfont <file.sf2|builtin> [preset] [name]",
            "Add a zone sampler playing a SoundFont preset (preset 1 is the first; `builtin` is the app's bundled GM soundfont).",
            add_soundfont,
        ),
        cmd(
            "channel.load_soundfont",
            "channel.load_soundfont <channel> <file.sf2|builtin> [preset]",
            "Replace a zone sampler's zones with a SoundFont preset's.",
            load_soundfont,
        ),
        cmd(
            "channel.set_soundfont_preset",
            "channel.set_soundfont_preset <channel> <file.sf2|builtin> <preset>",
            "Replace a channel's instrument with a SoundFont preset's zones (preset 1 is the first; `builtin` is the app's bundled GM soundfont).",
            set_soundfont_preset,
        ),
        cmd(
            "soundfont.presets",
            "soundfont.presets <file.sf2|builtin>",
            "List a SoundFont's presets as `index bank:program name`.",
            presets,
        ),
    ]
}

/// The file a soundfont command's path argument names: `builtin` is the
/// app's bundled General MIDI soundfont, anything else a path.
fn resolve_path(path: &str, args: &Args<'_>) -> Result<String, CommandError> {
    match path {
        "builtin" => musiccraft_paths::bundled_soundfont()
            .map(|p| p.display().to_string())
            .ok_or_else(|| args.invalid("the built-in soundfont is not installed this session")),
        _ => Ok(path.to_owned()),
    }
}

/// The soundfont file at `path`, read and parsed.
pub(super) fn parse_file(path: &str) -> Result<Soundfont, String> {
    let data = std::fs::read(path).map_err(|e| format!("cannot read {path}: {e}"))?;
    Soundfont::parse(&data).map_err(|e| format!("{path}: {e}"))
}

/// The preset at `index` (0-based) of the soundfont at `path`, as the
/// project's zones: `soundfont:` sample references with the soundfont's
/// key ranges, velocity ranges and tuning, plus the preset's own volume
/// envelope so the channel fades like the font's player would.
pub(super) fn preset_zones_at(
    font: &Soundfont,
    path: &str,
    index: usize,
) -> Result<(String, Vec<SampleZone>, bool, Envelope), String> {
    let chosen = font
        .presets()
        .get(index)
        .ok_or_else(|| format!("{path}: there is no preset {}", index + 1))?;
    let specs = font
        .preset_zones(index)
        .map_err(|e| format!("{path}: {e}"))?;
    let envelope = specs
        .first()
        .map(|zone| Envelope {
            attack: zone.attack,
            decay: zone.decay,
            sustain: zone.sustain,
            release: zone.release,
        })
        .unwrap_or_default();
    let zones: Vec<SampleZone> = specs
        .iter()
        .map(|zone| SampleZone {
            sample: format!("soundfont:{path}#{}", zone.sample),
            low: zone.low,
            high: zone.high,
            root: zone.root.round().clamp(0.0, 127.0) as u8,
            tune: ((zone.root - zone.root.round()) * 100.0)
                .round()
                .clamp(-50.0, 50.0) as i8,
            vel_low: zone.vel_low.max(1),
            vel_high: zone.vel_high.max(zone.vel_low.max(1)),
            loop_start: zone.loop_start,
            loop_end: zone.loop_end,
        })
        .collect();
    Ok((chosen.name.clone(), zones, chosen.bank >= 128, envelope))
}

/// A preset's volume envelope, in seconds and a 0..=1 sustain level.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Envelope {
    pub attack: f32,
    pub decay: f32,
    pub sustain: f32,
    pub release: f32,
}

impl Default for Envelope {
    fn default() -> Self {
        Self {
            attack: 0.0,
            decay: 0.0,
            sustain: 1.0,
            release: 0.0,
        }
    }
}

impl Envelope {
    /// Writes the envelope into a channel's instrument params, clamped to
    /// half a minute so a broken file cannot hold a note for years.
    pub fn apply(&self, params: &mut musiccraft_project::InstrumentParams) {
        let clamp_time = |seconds: f32| seconds.clamp(0.0, 30.0);
        params.0[musiccraft_project::params::inst::ATTACK] = clamp_time(self.attack);
        params.0[musiccraft_project::params::inst::DECAY] = clamp_time(self.decay);
        params.0[musiccraft_project::params::inst::SUSTAIN] = self.sustain.clamp(0.0, 1.0);
        params.0[musiccraft_project::params::inst::RELEASE] = clamp_time(self.release);
    }
}

/// The preset `args` names (1-based, default 1) of the soundfont at `path`,
/// as the project's zones: `soundfont:` sample references with the soundfont's
/// key ranges, velocity ranges and tuning.
fn preset_zones(
    path: &str,
    args: &Args<'_>,
    at: usize,
) -> Result<(String, Vec<SampleZone>, bool, Envelope), CommandError> {
    let font = parse_file(path).map_err(|e| args.invalid(e))?;
    let presets = font.presets();
    if presets.is_empty() {
        return Err(args.invalid(format!("{path}: the soundfont has no presets")));
    }
    let number = if args.len() > at {
        args.number(at, "preset")?
    } else {
        1
    };
    if number > presets.len() {
        return Err(args.invalid(format!(
            "there is no preset {number}; {} has {} (soundfont.presets lists them)",
            path,
            presets.len()
        )));
    }
    preset_zones_at(&font, path, number - 1).map_err(|e| args.invalid(e))
}

/// Writes a preset's volume envelope into a channel: the soundfont's own
/// attack, decay, sustain and release, so a plucked preset fades like its
/// player's instead of holding the sustain level until the note-off.
pub(super) fn set_envelope(channel: &mut musiccraft_project::Channel, envelope: &Envelope) {
    envelope.apply(&mut channel.params);
}

/// The soundfont path a zone sampler's sample plays:
/// `soundfont:<path>#<sample>`.
fn soundfont_path(sample: &str) -> Option<&str> {
    let (path, _) = sample.strip_prefix("soundfont:")?.rsplit_once('#')?;
    (!path.is_empty()).then_some(path)
}

/// The soundfont the project already plays, if any: the first zone sampler
/// whose samples come from a soundfont file, parsed once for MIDI import.
pub(super) fn loaded_font(project: &Project) -> Option<(String, Soundfont)> {
    let path = project
        .channels
        .iter()
        .find_map(|channel| match &channel.instrument {
            Instrument::Zones { zones } => {
                zones.first().and_then(|zone| soundfont_path(&zone.sample))
            }
            _ => None,
        })?;
    let path = path.to_owned();
    parse_file(&path).ok().map(|font| (path, font))
}

/// The app's bundled General MIDI soundfont, when it is installed this
/// session: the installed file's path, which the zones' `soundfont:`
/// samples must reference (the soundfont's own name is a display name,
/// not a path — `TimGM6mb1.sf2` exists nowhere on disk).
pub(super) fn default_font() -> Option<(String, Soundfont)> {
    let path = musiccraft_paths::bundled_soundfont()?;
    let path = path.display().to_string();
    let font = parse_file(&path).ok()?;
    Some((path, font))
}

/// A soundfont preset a channel can switch to: the 1-based preset number
/// `channel.set_soundfont_preset` takes, the bank and program it plays, and
/// its name for the menu.
#[derive(Debug, Clone)]
pub struct PresetChoice {
    pub preset: usize,
    pub bank: u16,
    pub program: u16,
    pub name: String,
}

/// One soundfont the menus offer, with what it carries: the presets when
/// the file reads, or why it does not — a soundfont the project still
/// names (a file manager pick whose cache copy is gone, say) shows up as
/// unreadable instead of silently vanishing.
#[derive(Debug, Clone)]
pub struct FontEntry {
    /// The path the soundfont commands accept.
    pub path: String,
    /// The file's name alone, for the menu.
    pub file_name: String,
    /// The soundfont's own name, from the file.
    pub name: String,
    /// Presets in bank 0 (melodic instruments).
    pub melodic: usize,
    /// Presets in a percussion bank (128 and up).
    pub drums: usize,
    /// The presets, or `None` when the file cannot be read or parsed.
    pub presets: Option<Vec<PresetChoice>>,
    /// Why [`Self::presets`] is `None`.
    pub error: Option<String>,
}

impl FontEntry {
    /// The menu's one-line summary: what it carries, or why it is silent.
    pub fn summary(&self) -> String {
        match &self.presets {
            Some(_) => format!(
                "{} — melodic {}, drums {}",
                self.file_name, self.melodic, self.drums
            ),
            None => format!(
                "{} — unreadable ({})",
                self.file_name,
                self.error.as_deref().unwrap_or("unknown reason")
            ),
        }
    }
}

/// The soundfont files the project's zone samplers play, first use first.
pub fn project_font_paths(project: &Project) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    for channel in &project.channels {
        let Instrument::Zones { zones } = &channel.instrument else {
            continue;
        };
        for zone in zones {
            let Some(path) = soundfont_path(&zone.sample) else {
                continue;
            };
            if !found.iter().any(|known| known == path) {
                found.push(path.to_owned());
            }
        }
    }
    found
}

/// The soundfont paths a channel's instrument menu offers, best first: the
/// channel's own, then the project's other soundfonts in use, then the
/// bundled GM soundfont. Cheap (no file is read) — the menus key their
/// caches on it; [`font_entries`] does the reading.
pub fn font_paths(project: &Project, index: usize) -> Vec<String> {
    let own = project
        .channels
        .get(index)
        .and_then(|channel| match &channel.instrument {
            Instrument::Zones { zones } => {
                zones.first().and_then(|zone| soundfont_path(&zone.sample))
            }
            _ => None,
        });
    let mut paths: Vec<String> = Vec::new();
    if let Some(path) = own {
        paths.push(path.to_owned());
    }
    for path in project_font_paths(project) {
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    if let Some(bundled) = musiccraft_paths::bundled_soundfont() {
        let path = bundled.display().to_string();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths
}

/// The soundfonts a channel's instrument menu offers, best first: the
/// channel's own, then the project's other soundfonts in use, then the
/// bundled GM soundfont. Unreadable files come along as unreadable entries
/// — the menu says so instead of quietly showing nothing.
pub fn font_entries(project: &Project, index: usize) -> Vec<FontEntry> {
    font_paths(project, index)
        .iter()
        .map(|path| font_entry(path))
        .collect()
}

/// The soundfonts the project plays, first use first — the Options menu's
/// soundfont list. The bundled font joins when installed, so it shows even
/// before anything plays it.
pub fn project_fonts(project: &Project) -> Vec<FontEntry> {
    let mut paths = project_font_paths(project);
    if let Some(bundled) = musiccraft_paths::bundled_soundfont() {
        let path = bundled.display().to_string();
        if !paths.contains(&path) {
            paths.push(path);
        }
    }
    paths.iter().map(|path| font_entry(path)).collect()
}

/// Reads and summarizes the soundfont at `path`.
fn font_entry(path: &str) -> FontEntry {
    let file_name = std::path::Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned());
    match parse_file(path) {
        Ok(font) => {
            let presets: Vec<PresetChoice> = font
                .presets()
                .iter()
                .enumerate()
                .map(|(index, preset)| PresetChoice {
                    preset: index + 1,
                    bank: preset.bank,
                    program: preset.program,
                    name: preset.name.clone(),
                })
                .collect();
            FontEntry {
                path: path.to_owned(),
                file_name,
                name: font.name().to_owned(),
                melodic: presets.iter().filter(|p| p.bank < 128).count(),
                drums: presets.iter().filter(|p| p.bank >= 128).count(),
                presets: (!presets.is_empty()).then_some(presets),
                error: None,
            }
        }
        Err(err) => FontEntry {
            path: path.to_owned(),
            file_name,
            name: String::new(),
            melodic: 0,
            drums: 0,
            presets: None,
            error: Some(err),
        },
    }
}

/// The preset a MIDI channel plays in a General MIDI soundfont: melodic
/// channels play bank 0 program N (the file's program change, default 0),
/// the drum channel 9 plays the bank 128 kit, falling back to the first
/// percussion bank, then the soundfont's default preset.
pub(super) fn gm_preset(
    font: &Soundfont,
    path: &str,
    midi_channel: u8,
    program: Option<u8>,
) -> Option<(String, Vec<SampleZone>, bool, Envelope)> {
    let presets = font.presets();
    let index = match midi_channel {
        9 => presets
            .iter()
            .position(|p| p.bank == 128 && p.program == 0)
            .or_else(|| presets.iter().position(|p| p.bank >= 128))
            .or_else(|| presets.iter().position(|p| p.bank == 0 && p.program == 0))?,
        _ => {
            let program = u16::from(program.unwrap_or(0));
            presets
                .iter()
                .position(|p| p.bank == 0 && p.program == program)
                .or_else(|| presets.iter().position(|p| p.bank == 0 && p.program == 0))?
        }
    };
    preset_zones_at(font, path, index).ok()
}

fn add_soundfont(project: &mut Project, args: &Args<'_>) -> Result<Outcome, CommandError> {
    args.expect(1, 3)?;
    let path = resolve_path(args.str(0)?, args)?;
    let (preset, zones, drums, envelope) = preset_zones(&path, args, 1)?;
    let name = if args.len() > 2 {
        args.rest(2)?
    } else {
        preset
    };
    let id = ChannelId(project.alloc_id());
    let mut channel = Channel::new(id, name, Instrument::Zones { zones });
    set_one_shot(&mut channel, drums);
    set_envelope(&mut channel, &envelope);
    Ok(push_channel(project, channel))
}

/// A melodic soundfont preset releases its note at note-off, like a GM
/// player; a drum kit plays through (its samples are short, and note-offs
/// there mean nothing). Without this, every note rings to the sample's end,
/// piling up voices that sound like reverb and steal each other's slots.
pub(super) fn set_one_shot(channel: &mut musiccraft_project::Channel, drums: bool) {
    channel.params.0[musiccraft_project::params::inst::ONE_SHOT] = u8::from(drums) as f32;
}

/// Replaces any channel's instrument with a soundfont preset's zones —
/// `channel.load_soundfont` for channels that are not zone samplers yet.
/// The undo is `channel.set_instrument` with the old instrument's snapshot.
fn set_soundfont_preset(project: &mut Project, args: &Args<'_>) -> Result<Outcome, CommandError> {
    args.expect(3, 3)?;
    let index = channel_arg(project, args, 0)?;
    let path = resolve_path(args.str(1)?, args)?;
    let (preset, zones, drums, envelope) = preset_zones(&path, args, 2)?;
    let number = index + 1;
    let old = super::channel::snapshot_json(&project.channels[index])
        .map_err(|e| args.invalid(e.to_string()))?;
    let channel = &mut project.channels[index];
    channel.instrument = Instrument::Zones { zones };
    channel.params = musiccraft_project::InstrumentParams::default();
    set_one_shot(channel, drums);
    set_envelope(channel, &envelope);
    Ok(Outcome::undoable(
        format!("channel {number}: {preset}"),
        Invocation::new("channel.set_instrument", [number.to_string(), old]),
    ))
}

fn load_soundfont(project: &mut Project, args: &Args<'_>) -> Result<Outcome, CommandError> {
    args.expect(2, 3)?;
    let index = channel_arg(project, args, 0)?;
    let path = resolve_path(args.str(1)?, args)?;
    let (preset, zones, _, _) = preset_zones(&path, args, 2)?;
    let old = zones_mut(project, args, index)?.clone();
    let number = index + 1;
    // Undo: the new zones out (newest first), the old ones back in place.
    let mut inverses = Vec::new();
    for zone in (1..=zones.len()).rev() {
        inverses.push(
            Invocation::new(
                "channel.remove_zone",
                [number.to_string(), zone.to_string()],
            )
            .to_string(),
        );
    }
    for (position, zone) in old.iter().enumerate() {
        let json = serde_json::to_string(zone).map_err(|e| args.invalid(e.to_string()))?;
        inverses.push(
            Invocation::new(
                "channel.insert_zone",
                [number.to_string(), (position + 1).to_string(), json],
            )
            .to_string(),
        );
    }
    let inverse = match inverses.len() {
        0 => {
            return Ok(Outcome::done(format!(
                "channel {number}: {preset} (no zones)"
            )));
        }
        1 => Invocation::parse(&inverses[0]).map_err(|e| args.invalid(e.to_string()))?,
        _ => Invocation::new(crate::BATCH, inverses),
    };
    match &mut project.channels[index].instrument {
        Instrument::Zones { zones: target } => *target = zones,
        _ => {
            return Err(args.invalid(format!("channel {number} is not a zone sampler")));
        }
    }
    Ok(Outcome::undoable(
        format!("channel {number}: {preset}"),
        inverse,
    ))
}

fn presets(project: &mut Project, args: &Args<'_>) -> Result<Outcome, CommandError> {
    let _ = project;
    args.expect(1, 1)?;
    let path = resolve_path(args.str(0)?, args)?;
    let data =
        std::fs::read(&path).map_err(|e| CommandError::Io(format!("cannot read {path}: {e}")))?;
    let font = Soundfont::parse(&data).map_err(|e| args.invalid(format!("{path}: {e}")))?;
    let listed: Vec<String> = font
        .presets()
        .iter()
        .enumerate()
        .map(|(index, preset)| {
            format!(
                "{} {}:{} {}",
                index + 1,
                preset.bank,
                preset.program,
                preset.name
            )
        })
        .collect();
    Ok(Outcome::done(listed.join("\n")))
}
