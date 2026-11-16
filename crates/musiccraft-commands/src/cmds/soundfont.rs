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
            "channel.add_soundfont <file.sf2> [preset] [name]",
            "Add a zone sampler playing a SoundFont preset (preset 1 is the first).",
            add_soundfont,
        ),
        cmd(
            "channel.load_soundfont",
            "channel.load_soundfont <channel> <file.sf2> [preset]",
            "Replace a zone sampler's zones with a SoundFont preset's.",
            load_soundfont,
        ),
        cmd(
            "soundfont.presets",
            "soundfont.presets <file.sf2>",
            "List a SoundFont's presets as `index bank:program name`.",
            presets,
        ),
    ]
}

/// The preset `args` names (1-based, default 1) of the soundfont at `path`,
/// as the project's zones: `soundfont:` sample references with the soundfont's
/// key ranges, velocity ranges and tuning.
fn preset_zones(
    path: &str,
    args: &Args<'_>,
    at: usize,
) -> Result<(String, Vec<SampleZone>), CommandError> {
    let data =
        std::fs::read(path).map_err(|e| CommandError::Io(format!("cannot read {path}: {e}")))?;
    let font = Soundfont::parse(&data).map_err(|e| args.invalid(format!("{path}: {e}")))?;
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
    let chosen = &presets[number - 1];
    let specs = font
        .preset_zones(number - 1)
        .map_err(|e| args.invalid(format!("{path}: {e}")))?;
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

fn add_soundfont(project: &mut Project, args: &Args<'_>) -> Result<Outcome, CommandError> {
    args.expect(1, 3)?;
    let path = args.str(0)?;
    let (preset, zones) = preset_zones(path, args, 1)?;
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
    let path = args.str(1)?;
    let (preset, zones) = preset_zones(path, args, 2)?;
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
    let path = args.str(0)?;
    let data =
        std::fs::read(path).map_err(|e| CommandError::Io(format!("cannot read {path}: {e}")))?;
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
