use super::super::conversation::Conversation;
use super::super::investigation::tool_surface::ToolSurface;
use super::super::protocol::prompt;

pub(crate) fn estimate_generation_prompt_chars(
    conversation: &Conversation,
    tool_surface: ToolSurface,
    project_snapshot_hint: Option<&str>,
    test_coverage_hint: Option<&str>,
    dynamic_tool_names: &[&str],
) -> usize {
    let mut hint_tools: Vec<&str> = tool_surface
        .allowed_tool_names()
        .chain(tool_surface.mutation_tool_names().iter().copied())
        .collect();
    hint_tools.extend_from_slice(dynamic_tool_names);
    let hint = prompt::render_tool_surface_hint(tool_surface.as_str(), hint_tools);
    conversation
        .pruned_snapshot()
        .into_iter()
        .map(|message| message.content.len())
        .sum::<usize>()
        + hint.len()
        + project_snapshot_hint.map_or(0, str::len)
        + test_coverage_hint.map_or(0, str::len)
}

/// Caps tool result blocks in an accumulated results string to `max_lines` content lines each.
///
/// Only `=== tool_result: ... ===` blocks are affected. Error blocks, corrections, and other
/// injected messages pass through unchanged. Top-aligned truncation: the first `max_lines`
/// content lines are kept; a metadata note is appended when capping occurs.
pub(crate) fn cap_tool_result_blocks(text: &str, max_lines: usize) -> String {
    const HDR: &str = "=== tool_result:";
    const FTR: &str = "=== /tool_result ===";

    let mut out = String::with_capacity(text.len());
    let mut pos = 0;

    while pos < text.len() {
        match text[pos..].find(HDR) {
            None => {
                out.push_str(&text[pos..]);
                break;
            }
            Some(rel) => {
                let hdr_start = pos + rel;
                out.push_str(&text[pos..hdr_start]);

                let body_start = text[hdr_start..]
                    .find('\n')
                    .map(|i| hdr_start + i + 1)
                    .unwrap_or(text.len());
                out.push_str(&text[hdr_start..body_start]);

                match text[body_start..].find(FTR) {
                    None => {
                        out.push_str(&text[body_start..]);
                        pos = text.len();
                    }
                    Some(rel_ftr) => {
                        let ftr_start = body_start + rel_ftr;
                        let body = &text[body_start..ftr_start];
                        let body_line_count = body.lines().count();

                        if body_line_count > max_lines {
                            for line in body.lines().take(max_lines) {
                                out.push_str(line);
                                out.push('\n');
                            }
                            out.push_str(&format!(
                                "[capped at {max_lines} lines — original: {body_line_count} lines]\n"
                            ));
                        } else {
                            out.push_str(body);
                        }

                        let ftr_end = ftr_start + FTR.len();
                        let trailing = text[ftr_end..]
                            .find(|c: char| c != '\n')
                            .map(|i| ftr_end + i)
                            .unwrap_or(text.len());
                        out.push_str(&text[ftr_start..trailing]);
                        pos = trailing;
                    }
                }
            }
        }
    }

    out
}
