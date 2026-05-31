mod buffer;
mod diff;
mod style;
mod symbols;

use std::io::{self, Write};

use self::buffer::{Cell, CellBuffer};
use self::diff::PatchWriter;
use self::style::{PackedStyle, Rgb};
use self::symbols::SymbolPool;

use super::state::{AppState, DirtySections, MessageKind, Role};

const BG: Rgb = Rgb::new(0, 0, 0);
const FG: Rgb = Rgb::new(220, 220, 220);
const FG_DIM: Rgb = Rgb::new(120, 120, 120);
const FG_ALERT: Rgb = Rgb::new(242, 179, 86);
const FG_ERROR: Rgb = Rgb::new(220, 80, 80);
const FG_GREEN: Rgb = Rgb::new(80, 200, 80);
const FG_YELLOW: Rgb = Rgb::new(220, 180, 80);
const FG_RED: Rgb = Rgb::new(220, 80, 80);

const MAX_INPUT_ROWS: usize = 6;

pub(crate) struct RenderStats {
    pub(crate) changed_cells: usize,
}

pub(crate) struct Renderer {
    symbols: SymbolPool,
    frames: [CellBuffer; 2],
    current: usize,
    width: u16,
    height: u16,
}

impl Renderer {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        let mut symbols = SymbolPool::new();
        let blank_id = symbols.blank_id();
        let blank = Cell {
            symbol_id: blank_id,
            style: PackedStyle::new(FG, BG),
        };
        let mut this = Self {
            symbols,
            frames: [
                CellBuffer::new(width, height, blank),
                CellBuffer::new(width, height, blank),
            ],
            current: 0,
            width,
            height,
        };
        this.invalidate();
        this
    }

    pub(crate) fn resize(&mut self, width: u16, height: u16) {
        self.width = width;
        self.height = height;
        self.frames[0].resize(width, height);
        self.frames[1].resize(width, height);
        self.invalidate();
    }

    pub(crate) fn invalidate(&mut self) {
        let prev = 1 - self.current;
        let sid = self.symbols.intern("~");
        let sentinel = Cell {
            symbol_id: sid,
            style: PackedStyle::new(Rgb::new(1, 0, 0), Rgb::new(0, 0, 1)),
        };
        self.frames[prev].fill(sentinel);
    }

    pub(crate) fn render<W: Write>(
        &mut self,
        state: &mut AppState,
        out: &mut W,
        _dirty: DirtySections,
    ) -> io::Result<RenderStats> {
        let w = self.width;
        let h = self.height;
        let cur = self.current;

        let base = PackedStyle::new(FG, BG);
        let bold = base.with_bold();
        let dim = PackedStyle::new(FG_DIM, BG);
        let alert = PackedStyle::new(FG_ALERT, BG).with_bold();
        let error_style = PackedStyle::new(FG_ERROR, BG);

        let blank_id = self.symbols.blank_id();
        self.frames[cur].fill(Cell {
            symbol_id: blank_id,
            style: base,
        });

        // Row 0: header
        if h > 0 {
            let title = format!(" {}  |  Ctrl+Q quit  |  Enter send ", state.app_name);
            self.paint(cur, 0, 0, &title, w, bold);
        }

        // Row 1: horizontal rule
        if h > 1 {
            let rule = "─".repeat(w as usize);
            self.paint(cur, 0, 1, &rule, w, base);
        }

        let input_rows = state
            .input_content_rows(w as usize)
            .max(1)
            .min(MAX_INPUT_ROWS) as u16;
        let overlay_rows: u16 = if state.reverse_search_view().is_some() {
            1
        } else {
            0
        };
        let effective_rows = input_rows + overlay_rows;

        // Rows 2..h-effective_rows-2: transcript
        if h > effective_rows + 3 {
            let transcript_height = h.saturating_sub(effective_rows + 3) as usize;
            let avail_w = w.saturating_sub(1) as usize;

            // Each entry: (display_text, kind, source_message_index).
            let mut lines: Vec<(String, MessageKind, Option<usize>)> = Vec::new();
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

                if msg.is_collapsible && state.collapsed_message_indices.contains(&i) {
                    // Collapsed: emit one summary line with a toggle affordance.
                    let summary: String = msg.content.chars().take(60).collect();
                    let ellipsis = if msg.content.chars().count() > 60 {
                        "…"
                    } else {
                        ""
                    };
                    let focused = state
                        .focused_collapsible_idx
                        .and_then(|fi| state.collapsible_message_indices.get(fi).copied())
                        == Some(i);
                    let indicator = if focused { "▶[+] " } else { " [+] " };
                    lines.push((format!("{indicator}{summary}{ellipsis}"), msg.kind, Some(i)));
                    lines.push((String::new(), msg.kind, Some(i)));
                    continue;
                }

                let prefix = if is_expanded {
                    ""
                } else {
                    match msg.role {
                        Role::System => "system: ",
                        Role::User => "you: ",
                        Role::Assistant => "assistant: ",
                    }
                };

                // Two-char prefix reserved for all collapsible messages so wrap
                // geometry is stable when focus moves. Focused = "▶ ", unfocused = "  ".
                let focus_prefix = if msg.is_collapsible {
                    let focused = state
                        .focused_collapsible_idx
                        .and_then(|fi| state.collapsible_message_indices.get(fi).copied())
                        == Some(i);
                    if focused {
                        "▶ "
                    } else {
                        "  "
                    }
                } else {
                    ""
                };

                let text = format!("{focus_prefix}{prefix}{}", msg.content);
                for line in wrap_text(&text, avail_w.max(8)) {
                    lines.push((line, msg.kind, Some(i)));
                }
                lines.push((String::new(), msg.kind, Some(i)));
            }

            let max_scroll = lines.len().saturating_sub(transcript_height);

            // Scroll the newly focused collapsible into the upper third of the
            // viewport. Consumed once per focus-cycle key press.
            if let Some(msg_idx) = state.scroll_to_message_idx.take() {
                if let Some(target_line) =
                    lines.iter().position(|(_, _, src)| *src == Some(msg_idx))
                {
                    let upper_third = transcript_height / 3;
                    // desired_start is where we want the viewport to begin.
                    let desired_start = target_line.saturating_sub(upper_third);
                    // offset counts lines from the bottom; invert desired_start.
                    state.scroll_offset = max_scroll.saturating_sub(desired_start).min(max_scroll);
                }
            }

            let offset = state.scroll_offset.min(max_scroll);
            let end = lines.len().saturating_sub(offset);
            let start = end.saturating_sub(transcript_height);
            let visible = &lines[start..end];
            let cap = h.saturating_sub(effective_rows + 1);

            for (idx, (line, kind, _msg_idx)) in visible.iter().enumerate() {
                let row = 2 + idx as u16;
                if row >= cap {
                    break;
                }
                let style = match kind {
                    MessageKind::Dimmed => dim,
                    MessageKind::Alert => alert,
                    MessageKind::Error => error_style,
                    MessageKind::Normal => base,
                };
                self.paint(cur, 0, row, line, w, style);
            }

            if offset > 0 && !visible.is_empty() {
                let indicator = format!("↑ {} lines", offset);
                let ind_len = indicator.chars().count() as u16;
                if w > ind_len {
                    let col = w.saturating_sub(ind_len);
                    let row = 2 + visible.len().saturating_sub(1) as u16;
                    if row < cap {
                        self.paint(cur, col, row, &indicator, ind_len, base);
                    }
                }
            }
        }

        // Row h-effective_rows-2: horizontal rule before input
        if h > effective_rows + 2 {
            let row = h.saturating_sub(effective_rows + 2);
            let rule = "─".repeat(w as usize);
            self.paint(cur, 0, row, &rule, w, base);
        }

        // Rows h-effective_rows-1..h-overlay_rows-2: input area
        if h > effective_rows + 1 {
            let first_row = h.saturating_sub(effective_rows + 1);
            let prefix = "> ";
            let prefix_w = prefix.len() as u16;
            let avail = w.saturating_sub(prefix_w) as usize;
            let (visible_lines, _, _) = state.input_display_lines(avail.max(1), MAX_INPUT_ROWS);
            for (i, line) in visible_lines.iter().enumerate() {
                let row = first_row + i as u16;
                if i == 0 {
                    self.paint(cur, 0, row, prefix, prefix_w, bold);
                } else {
                    self.paint(cur, 0, row, "  ", prefix_w, bold);
                }
                self.paint(cur, prefix_w, row, line, w.saturating_sub(prefix_w), base);
            }
        }

        // Reverse-search overlay row
        if overlay_rows > 0 {
            if let Some((query, matched)) = state.reverse_search_view() {
                let row = h.saturating_sub(overlay_rows + 1);
                let text = format!("bkwd-search: {}  {}", query, matched);
                let display: String = text.chars().take(w as usize).collect();
                self.paint(cur, 0, row, &display, w, base);
            }
        }

        // Row h-1: status bar
        if h > 1 {
            let row = h.saturating_sub(1);
            let text = if state.show_activity {
                format!("  {}  ", state.status)
            } else {
                " ".to_string()
            };
            self.paint(cur, 0, row, &text, w, base);

            if let Some(pct) = state.context_pct {
                let indicator = format!(" ctx: {pct}% ");
                let ind_len = indicator.chars().count() as u16;
                if w > ind_len {
                    let col = w.saturating_sub(ind_len);
                    let color = if pct < 50 {
                        FG_GREEN
                    } else if pct <= 75 {
                        FG_YELLOW
                    } else {
                        FG_RED
                    };
                    self.paint(
                        cur,
                        col,
                        row,
                        &indicator,
                        ind_len,
                        PackedStyle::new(color, BG),
                    );
                }
            }
        }

        // Input cursor position
        let (cx, cy) = if h > effective_rows + 1 {
            let prefix_len = 2usize;
            let avail = w.saturating_sub(prefix_len as u16) as usize;
            let (_, cursor_row, cursor_col) =
                state.input_display_lines(avail.max(1), MAX_INPUT_ROWS);
            let x = (prefix_len + cursor_col).min(w as usize) as u16;
            let y = h.saturating_sub(effective_rows + 1) + cursor_row as u16;
            (x, y)
        } else {
            (0, 0)
        };

        let prev = 1 - cur;
        let mut pw = PatchWriter::new();
        let stats = pw.write_diff(
            out,
            &self.frames[prev],
            &self.frames[cur],
            &self.symbols,
            (cx, cy),
        )?;
        self.current = 1 - self.current;

        Ok(RenderStats {
            changed_cells: stats.changed_cells,
        })
    }

    fn paint(
        &mut self,
        cur: usize,
        x: u16,
        y: u16,
        text: &str,
        max_width: u16,
        style: PackedStyle,
    ) {
        self.frames[cur].write_text_clipped(x, y, text, max_width, style, &mut self.symbols);
    }
}

fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(current);
            current = String::new();
            continue;
        }
        current.push(ch);
        if current.chars().count() >= width {
            lines.push(current);
            current = String::new();
        }
    }
    if current.is_empty() {
        if lines.is_empty() {
            lines.push(String::new());
        }
    } else {
        lines.push(current);
    }
    lines
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::TempDir;

    use crate::app::config::Config;
    use crate::app::paths::AppPaths;
    use crate::tui::state::{AppState, DirtySections};

    use super::Renderer;

    fn make_state() -> (TempDir, AppState) {
        let dir = TempDir::new().unwrap();
        fs::create_dir_all(dir.path().join("data")).unwrap();
        fs::create_dir_all(dir.path().join("logs")).unwrap();
        let paths = AppPaths {
            root_dir: dir.path().to_path_buf(),
            project_root: dir.path().to_path_buf(),
            config_file: dir.path().join("config.toml"),
            data_dir: dir.path().join("data"),
            logs_dir: dir.path().join("logs"),
            session_db: dir.path().join("data").join("sessions.db"),
        };
        let state = AppState::new(&Config::default(), &paths);
        (dir, state)
    }

    #[test]
    fn second_render_of_unchanged_state_writes_zero_cells() {
        let (_dir, mut state) = make_state();
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();
        out.clear();
        let stats = renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();
        assert_eq!(
            stats.changed_cells, 0,
            "unchanged state must produce zero changed cells"
        );
    }
}
