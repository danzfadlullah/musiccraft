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
/// key ranges, velocity ranges and tuning.
pub(super) fn preset_zones_at(
    font: &Soundfont,
    path: &str,
    index: usize,
) -> Result<(String, Vec<SampleZone>), String> {
    let chosen = font
        .presets()
        .get(index)
        .ok_or_else(|| format!("{path}: there is no preset {}", index + 1))?;
    let specs = font
        .preset_zones(index)
        .map_err(|e| format!("{path}: {e}"))?;
    let zones: Vec<SampleZone> = specs
        .iter()
        .map(|zone| SampleZone {
            sample: format!("soundfont:{path}#{}", zone.sample),
            low: zone.low,
            high: zone.high,
            root: zone.root.round().clamp(0.0, 127.0) as u8,
            vel_low: zone.vel_low.max(1),
            vel_high: zone.vel_high.max(zone.vel_low.max(1)),
        })
        .collect();
    Ok((chosen.name.clone(), zones))
}

/// The preset `args` names (1-based, default 1) of the soundfont at `path`,
/// as the project's zones: `soundfont:` sample references with the soundfont's
/// key ranges, velocity ranges and tuning.
fn preset_zones(
    path: &str,
    args: &Args<'_>,
    at: usize,
) -> Result<(String, Vec<SampleZone>), CommandError> {
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
/// session.
pub(super) fn default_font() -> Option<(String, Soundfont)> {
    let path = musiccraft_paths::bundled_soundfont()?;
    let font = parse_file(&path.display().to_string()).ok()?;
    Some((font.name().to_owned(), font))
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
) -> Option<(String, Vec<SampleZone>)> {
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
    let (preset, zones) = preset_zones(&path, args, 1)?;
    let name = if args.len() > 2 {
        args.rest(2)?
    } else {
        preset
    };
    let id = ChannelId(project.alloc_id());
    Ok(push_channel(
        project,
        Channel::new(id, name, Instrument::Zones { zones }),
    ))
}

fn load_soundfont(project: &mut Project, args: &Args<'_>) -> Result<Outcome, CommandError> {
    args.expect(2, 3)?;
    let index = channel_arg(project, args, 0)?;
    let path = resolve_path(args.str(1)?, args)?;
    let (preset, zones) = preset_zones(&path, args, 2)?;
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
