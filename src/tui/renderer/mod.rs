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
        state: &AppState,
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

        // Rows 2..h-3: transcript
        if h > 4 {
            let transcript_height = h.saturating_sub(4) as usize;
            let avail_w = w.saturating_sub(1) as usize;

            let mut lines: Vec<(String, MessageKind)> = Vec::new();
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
                let prefix = if is_expanded {
                    ""
                } else {
                    match msg.role {
                        Role::System => "system: ",
                        Role::User => "you: ",
                        Role::Assistant => "assistant: ",
                    }
                };
                let text = format!("{prefix}{}", msg.content);
                for line in wrap_text(&text, avail_w.max(8)) {
                    lines.push((line, msg.kind));
                }
                lines.push((String::new(), msg.kind));
            }

            let max_scroll = lines.len().saturating_sub(transcript_height);
            let offset = state.scroll_offset.min(max_scroll);
            let end = lines.len().saturating_sub(offset);
            let start = end.saturating_sub(transcript_height);
            let visible = &lines[start..end];
            let cap = h.saturating_sub(2);

            for (idx, (line, kind)) in visible.iter().enumerate() {
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

        // Row h-3: horizontal rule before input
        if h > 3 {
            let row = h.saturating_sub(3);
            let rule = "─".repeat(w as usize);
            self.paint(cur, 0, row, &rule, w, base);
        }

        // Row h-2: input line
        if h > 2 {
            let row = h.saturating_sub(2);
            let prefix = "> ";
            let prefix_w = prefix.len() as u16;
            let avail = w.saturating_sub(prefix_w) as usize;
            let vis = visible_input_slice(&state.input, state.cursor, avail.max(1));
            self.paint(cur, 0, row, prefix, prefix_w, bold);
            self.paint(cur, prefix_w, row, &vis, w.saturating_sub(prefix_w), base);
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
        let (cx, cy) = if h > 2 {
            let prefix_len = 2usize;
            let avail = w.saturating_sub(prefix_len as u16) as usize;
            let cursor_chars = state.input[..state.cursor].chars().count();
            let vis = visible_input_slice(&state.input, state.cursor, avail.max(1));
            let vis_chars = vis.chars().count();
            let start = cursor_chars.saturating_sub(avail.saturating_sub(1));
            let rel = cursor_chars.saturating_sub(start).min(vis_chars);
            let x = (prefix_len + rel).min(w as usize) as u16;
            (x, h.saturating_sub(2))
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

fn visible_input_slice(input: &str, cursor: usize, width: usize) -> String {
    let chars: Vec<char> = input.chars().collect();
    if chars.len() <= width {
        return input.to_string();
    }
    let cursor_chars = input[..cursor].chars().count();
    let start = cursor_chars.saturating_sub(width.saturating_sub(1));
    chars[start..(start + width).min(chars.len())]
        .iter()
        .collect()
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
        let (_dir, state) = make_state();
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&state, &mut out, DirtySections::ALL)
            .unwrap();
        out.clear();
        let stats = renderer
            .render(&state, &mut out, DirtySections::ALL)
            .unwrap();
        assert_eq!(
            stats.changed_cells, 0,
            "unchanged state must produce zero changed cells"
        );
    }
}
