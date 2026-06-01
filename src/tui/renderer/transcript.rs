use crate::tui::collapsible::classify_collapsible;
use crate::tui::state::{AppState, MessageKind, Role};

use super::{Renderer, StyledLine};

impl Renderer {
    pub(super) fn build_transcript_lines(&self, state: &AppState, w: u16) -> Vec<StyledLine> {
        let base = self.theme.base();
        let dim = self.theme.dim();
        let alert = self.theme.chip_warning();
        let error_style = self.theme.chip_danger();
        let border = self.theme.border();

        let collapsible_ids = state.collapsible_indices();
        let mut lines: Vec<StyledLine> = Vec::new();

        for (i, msg) in state.messages.iter().enumerate() {
            if !state.expanded_file_read {
                if let Some(idx) = state.last_file_read_index {
                    if i == idx && msg.role == Role::Assistant {
                        continue;
                    }
                }
            }
            let is_expanded = state.expanded_file_read
                && state.last_file_read_index.map_or(false, |idx| i == idx)
                && msg.role == Role::Assistant;

            let body_style = match msg.kind {
                MessageKind::Normal => base,
                MessageKind::Dimmed => dim,
                MessageKind::Alert => alert,
                MessageKind::Error => error_style,
            };

            let is_focused_collapsible = msg.is_collapsible
                && state
                    .focused_collapsible_idx
                    .and_then(|fi| collapsible_ids.get(fi).copied())
                    == Some(i);

            if msg.is_collapsible && state.collapsed_message_indices.contains(&i) {
                let classified = classify_collapsible(&msg.content);
                let indicator = if is_focused_collapsible { "▶ " } else { "  " };
                let indicator_style = if is_focused_collapsible {
                    self.theme.border_active()
                } else {
                    dim
                };
                let hint = if is_focused_collapsible {
                    "  alt+o"
                } else {
                    ""
                };
                lines.push((
                    vec![
                        (indicator.to_string(), indicator_style),
                        ("›".to_string(), self.theme.border()),
                        (" ".to_string(), dim),
                        (classified.summary, dim),
                        (hint.to_string(), dim),
                    ],
                    Some(i),
                ));
                for preview_line in classified.preview_lines.iter().take(2) {
                    lines.push((
                        vec![("  ".to_string(), dim), (preview_line.clone(), dim)],
                        Some(i),
                    ));
                }
                lines.push((vec![], Some(i)));
                continue;
            }

            if is_expanded {
                let body_w = (w as usize).saturating_sub(2).max(8);
                let body_lines = super::wrap_text(&msg.content, body_w);
                for (li, body_line) in body_lines.into_iter().enumerate() {
                    let border_span = if li == 0 && is_focused_collapsible {
                        ("▶ ".to_string(), self.theme.border_active())
                    } else {
                        ("│ ".to_string(), border)
                    };
                    lines.push((vec![border_span, (body_line, body_style)], Some(i)));
                }
                lines.push((vec![], Some(i)));
                continue;
            }

            let (badge_text, badge_style) = match msg.role {
                Role::User => ("you", self.theme.badge_user()),
                Role::Assistant => ("assistant", self.theme.badge_assistant()),
                Role::System => ("system", self.theme.dim()),
            };
            let badge_len = badge_text.chars().count();
            let prefix_w = 2 + badge_len + 2;
            let body_w = (w as usize).saturating_sub(prefix_w).max(8);
            let body_lines = super::wrap_text(&msg.content, body_w);

            for (li, body_line) in body_lines.into_iter().enumerate() {
                if li == 0 {
                    let border_span = if is_focused_collapsible {
                        ("▶ ".to_string(), self.theme.border_active())
                    } else {
                        ("│ ".to_string(), border)
                    };
                    lines.push((
                        vec![
                            border_span,
                            (badge_text.to_string(), badge_style),
                            ("  ".to_string(), base),
                            (body_line, body_style),
                        ],
                        Some(i),
                    ));
                } else {
                    let indent = " ".repeat(badge_len + 2);
                    lines.push((
                        vec![
                            ("│ ".to_string(), border),
                            (indent, base),
                            (body_line, body_style),
                        ],
                        Some(i),
                    ));
                }
            }
            lines.push((vec![], Some(i)));
        }

        if state.is_busy && state.pending_approval.is_none() && !state.messages.is_empty() {
            if let Some(ast_idx) = state
                .messages
                .iter()
                .enumerate()
                .rev()
                .find(|(_, m)| m.role == Role::Assistant)
                .map(|(i, _)| i)
            {
                // Only cursor the message that is actively streaming: the last
                // assistant message must also be the last message in the vec.
                // Before AssistantMessageStarted fires the last message is the
                // user prompt, so ast_idx + 1 < messages.len() and no cursor
                // appears on the previous completed response.
                if ast_idx + 1 == state.messages.len() {
                    let cursor_style = if self.spin_tick % 12 < 6 {
                        self.theme.badge_assistant()
                    } else {
                        self.theme.chip_accent()
                    };
                    if let Some(target) = lines
                        .iter()
                        .rposition(|(spans, src)| *src == Some(ast_idx) && !spans.is_empty())
                    {
                        lines[target].0.push(("▍".to_string(), cursor_style));
                    }
                }
            }
        }

        lines
    }
}
