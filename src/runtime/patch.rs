use mpatch::{parse_patches, patch_content_str, ApplyOptions};

#[derive(Debug)]
pub enum PatchError {
    ParseError(String),
    AmbiguousAnchor,
    AnchorNotFound,
    MultiplePatches,
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::ParseError(msg) => write!(f, "parse error: {msg}"),
            PatchError::AmbiguousAnchor => {
                write!(f, "ambiguous anchor: multiple candidate locations")
            }
            PatchError::AnchorNotFound => write!(f, "anchor not found in target content"),
            PatchError::MultiplePatches => {
                write!(f, "expected exactly one patch, found zero or multiple")
            }
        }
    }
}

pub fn apply_patch(original: &str, patch_text: &str) -> Result<String, PatchError> {
    let patches = parse_patches(patch_text).map_err(|e| PatchError::ParseError(e.to_string()))?;
    if patches.len() != 1 {
        return Err(PatchError::MultiplePatches);
    }

    patch_content_str(patch_text, Some(original), &ApplyOptions::default()).map_err(|e| {
        use mpatch::OneShotError;
        match e {
            OneShotError::Parse(pe) => PatchError::ParseError(pe.to_string()),
            OneShotError::NoPatchesFound => PatchError::MultiplePatches,
            OneShotError::MultiplePatchesFound(_) => PatchError::MultiplePatches,
            OneShotError::Apply(_) => PatchError::AnchorNotFound,
            _ => PatchError::AnchorNotFound,
        }
    })
}
