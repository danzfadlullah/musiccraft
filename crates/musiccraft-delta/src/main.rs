//! The patch tool the release workflow drives: `make` builds a patch
//! between two APKs, `apply` rebuilds the new one and proves the round
//! trip before the release ships the patch.

use std::process::ExitCode;

fn usage() -> String {
    "usage: musiccraft-delta make <old> <new> <out>\n       musiccraft-delta apply <old> <patch> <out>"
        .to_owned()
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (verb, old_path, patch_path, out_path) = match args.as_slice() {
        [verb, old, new_or_patch, out] => (verb.as_str(), old, new_or_patch, out),
        _ => return Err(usage()),
    };
    let old = std::fs::read(old_path).map_err(|e| format!("cannot read {old_path}: {e}"))?;
    let patch = match verb {
        "make" => musiccraft_delta::make(
            &old,
            &std::fs::read(patch_path).map_err(|e| format!("cannot read {patch_path}: {e}"))?,
        )?,
        "apply" => {
            let patch =
                std::fs::read(patch_path).map_err(|e| format!("cannot read {patch_path}: {e}"))?;
            musiccraft_delta::apply(&old, &patch)?
        }
        _ => return Err(usage()),
    };
    std::fs::write(out_path, patch).map_err(|e| format!("cannot write {out_path}: {e}"))?;
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
