use mpatch::{patch_content_str, ApplyOptions};

#[derive(Debug)]
pub enum PatchError {
    ParseError(String),
    AnchorNotFound,
    MultiplePatches,
}

impl std::fmt::Display for PatchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PatchError::ParseError(msg) => write!(f, "parse error: {msg}"),
            PatchError::AnchorNotFound => write!(f, "anchor not found in target content"),
            PatchError::MultiplePatches => {
                write!(f, "expected exactly one patch, found zero or multiple")
            }
        }
    }
}

/// Applies a single-hunk patch to `original` using mpatch's auto-detection.
/// `patch_text` may be a unified diff, a markdown-fenced diff, or a conflict-marker
/// block (`<<<<<<< SEARCH … ======= … >>>>>>> REPLACE`).
/// `patch_content_str` validates "exactly one patch" internally via `parse_auto`.
pub fn apply_patch(original: &str, patch_text: &str) -> Result<String, PatchError> {
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
