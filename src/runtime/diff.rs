pub(crate) fn render_diff(original: &str, patched: &str) -> String {
    similar::TextDiff::from_lines(original, patched)
        .unified_diff()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::render_diff;

    #[test]
    fn render_diff_produces_unified_diff() {
        let original = "hello\nworld\n";
        let patched = "hello\nuniverse\n";
        let diff = render_diff(original, patched);
        assert!(!diff.is_empty(), "diff should not be empty");
        assert!(diff.contains('+'), "missing + line in diff");
        assert!(diff.contains('-'), "missing - line in diff");
    }
}
