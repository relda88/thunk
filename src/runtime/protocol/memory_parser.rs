#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryProposal {
    pub text: String,
    pub category: String,
}

/// Parse model output for memory fact candidates.
/// Format: [REMEMBER: text | category] — one tag per line, rest of line ignored.
/// Permissive: skips non-matching lines, never errors.
/// Category defaults to "general" when the pipe separator is absent.
pub fn parse_memory_proposals(text: &str) -> Vec<MemoryProposal> {
    let mut proposals = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        let Some(inner) = line
            .strip_prefix("[REMEMBER:")
            .and_then(|s| s.strip_suffix(']'))
        else {
            continue;
        };
        let inner = inner.trim();
        if inner.is_empty() {
            continue;
        }
        let (text, category) = if let Some((t, c)) = inner.split_once('|') {
            let t = t.trim().to_string();
            let c = c.trim().to_string();
            if t.is_empty() {
                continue;
            }
            (
                t,
                if c.is_empty() {
                    "general".to_string()
                } else {
                    c
                },
            )
        } else {
            (inner.to_string(), "general".to_string())
        };
        proposals.push(MemoryProposal { text, category });
    }
    proposals
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_valid_tag_with_category() {
        let text = "[REMEMBER: I prefer tabs over spaces | preference]";
        let proposals = parse_memory_proposals(text);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].text, "I prefer tabs over spaces");
        assert_eq!(proposals[0].category, "preference");
    }

    #[test]
    fn parse_tag_without_category_defaults_to_general() {
        let text = "[REMEMBER: I use Rust for backend work]";
        let proposals = parse_memory_proposals(text);
        assert_eq!(proposals.len(), 1);
        assert_eq!(proposals[0].text, "I use Rust for backend work");
        assert_eq!(proposals[0].category, "general");
    }

    #[test]
    fn parse_skips_non_matching_lines() {
        let text = "Here are some facts I found:\n\
                    [REMEMBER: prefers dark mode | preference]\n\
                    This line is ignored.\n\
                    [REMEMBER: uses Neovim | workflow]";
        let proposals = parse_memory_proposals(text);
        assert_eq!(proposals.len(), 2);
        assert_eq!(proposals[0].text, "prefers dark mode");
        assert_eq!(proposals[1].text, "uses Neovim");
    }

    #[test]
    fn parse_empty_input_returns_empty() {
        assert!(parse_memory_proposals("").is_empty());
    }

    #[test]
    fn parse_skips_empty_text_inside_tag() {
        let text = "[REMEMBER: | preference]";
        assert!(parse_memory_proposals(text).is_empty());
    }

    #[test]
    fn parse_multiple_valid_tags() {
        let text = "[REMEMBER: fact one | identity]\n[REMEMBER: fact two | project]";
        let proposals = parse_memory_proposals(text);
        assert_eq!(proposals.len(), 2);
        assert_eq!(proposals[0].category, "identity");
        assert_eq!(proposals[1].category, "project");
    }
}
