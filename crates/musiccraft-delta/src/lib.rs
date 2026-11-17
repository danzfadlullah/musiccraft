//! Binary delta patches for the Android updater. A release ships a small
//! patch per recent base version instead of a whole APK; the updater
//! rebuilds the new APK from the installed one and checks it against the
//! release's own SHA-256 before installing. The format is bsdiff 4.x, so
//! the patches stay verifiable with the standard tools.

/// Creates the patch turning `old` into `new`.
///
/// ```
/// let patch = musiccraft_delta::make(b"same same same", b"same new same")?;
/// let new = musiccraft_delta::apply(b"same same same", &patch)?;
/// assert_eq!(new, b"same new same");
/// # Ok::<(), String>(())
/// ```
pub fn make(old: &[u8], new: &[u8]) -> Result<Vec<u8>, String> {
    let mut patch = Vec::new();
    qbsdiff::Bsdiff::new(old, new)
        .compare(&mut patch)
        .map_err(|e| format!("creating the patch failed: {e}"))?;
    Ok(patch)
}

/// Applies a [`make`] patch to `old`, rebuilding `new`.
pub fn apply(old: &[u8], patch: &[u8]) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    qbsdiff::Bspatch::new(patch)
        .map_err(|e| format!("the patch is broken: {e}"))?
        .apply(old, &mut out)
        .map_err(|e| format!("applying the patch failed: {e}"))?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_patch_rebuilds_the_new_apk_from_the_old() {
        // Two builds that share almost everything, like two releases of the
        // same app: the patch must carry only what changed.
        let old = vec![7u8; 100_000];
        let mut new = old.clone();
        new[500..600].copy_from_slice(&[9u8; 100]);
        let patch = make(&old, &new).unwrap();
        assert!(patch.len() < new.len() / 10, "patch {}", patch.len());
        assert_eq!(apply(&old, &patch).unwrap(), new);
    }

    #[test]
    fn a_broken_patch_is_an_error_not_a_crash() {
        let old = vec![7u8; 1_000];
        let patch = make(&old, &old).unwrap();
        let mut broken = patch.clone();
        broken[10] ^= 0xff;
        assert!(apply(&old, &broken).is_err());
        assert!(apply(&old, &patch[..20]).is_err());
    }
}
