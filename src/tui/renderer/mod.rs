mod buffer;
mod diff;
mod style;
mod symbols;
mod transcript;

use std::io::{self, Write};

use unicode_width::UnicodeWidthChar;

use self::buffer::{Cell, CellBuffer};
use self::diff::PatchWriter;
use self::style::{PackedStyle, Rgb, Theme};
use self::symbols::SymbolPool;

use super::state::{AppState, ApprovalRisk, DirtySections};

type StyledSpan = (String, PackedStyle);
pub(super) type StyledLine = (Vec<StyledSpan>, Option<usize>);

const CTX_LOW: Rgb = Rgb::new(80, 200, 80);
const CTX_MID: Rgb = Rgb::new(242, 179, 86);
const CTX_HIGH: Rgb = Rgb::new(237, 104, 109);

const SPINNER: [char; 4] = ['-', '\\', '|', '/'];

const MAX_INPUT_ROWS: usize = 6;

pub(crate) struct RenderStats {
    // deferred: render performance metrics
    #[allow(dead_code)]
    pub(crate) changed_cells: usize,
}

pub(crate) struct Renderer {
    symbols: SymbolPool,
    frames: [CellBuffer; 2],
    current: usize,
    width: u16,
    height: u16,
    theme: Theme,
    spin_tick: u32,
}

impl Renderer {
    pub(crate) fn new(width: u16, height: u16) -> Self {
        let theme = Theme::default();
        let mut symbols = SymbolPool::new();
        let blank_id = symbols.blank_id();
        let blank = Cell {
            symbol_id: blank_id,
            style: theme.base(),
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
            theme,
            spin_tick: 0,
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

        let base = self.theme.base();

        let blank_id = self.symbols.blank_id();
        self.frames[cur].fill(Cell {
            symbol_id: blank_id,
            style: base,
        });

        // Row 0: header
        if h > 0 {
            self.paint_header(state, cur, w);
        }

        // Row 1: horizontal rule
        if h > 1 {
            let rule = "─".repeat(w as usize);
            self.paint(cur, 0, 1, &rule, w, self.theme.border());
        }

        let input_rows = state
            .input_content_rows(w as usize)
            .max(1)
            .min(MAX_INPUT_ROWS) as u16;
        let overlay_rows: u16 = if state.is_autocomplete_active() {
            state.autocomplete_preview_items(4).len() as u16
        } else if state.reverse_search_view().is_some() {
            1
        } else if state.is_launcher_active() {
            state
                .launcher_view(5)
                .map(|(_, e)| e.len() + 1)
                .unwrap_or(0) as u16
        } else {
            0
        };
        let approval_rows: u16 = state.pending_approval.as_ref().map_or(0, |a| {
            1 + if a.irreversible { 1 } else { 0 }
                + a.transaction_files.len().min(6) as u16
                + a.evidence.len().min(4) as u16
                + a.preview.len().min(4) as u16
                + if a.impact.is_empty() { 0 } else { 1 }
                + if a.reason.is_none() { 0 } else { 1 }
                + 1
        });
        let plan_rows: u16 = state
            .pending_plan_approval
            .as_ref()
            .map_or(0, |p| 1 + p.steps.len() as u16 + 1);
        // 1 fact line + 1 category line + 1 optional scope line + 1 controls line = 4 rows max.
        // Use 3 rows when scope is absent (fact + category + controls).
        let memory_rows: u16 =
            state.pending_memory_proposal.as_ref().map_or(
                0,
                |m| {
                    if m.scope.is_some() {
                        4
                    } else {
                        3
                    }
                },
            );
        let input_base_rows = input_rows + overlay_rows;
        let effective_rows = input_base_rows + approval_rows + plan_rows + memory_rows;

        // Rows 2..h-effective_rows-2: transcript
        if h > effective_rows + 3 {
            self.paint_transcript(state, cur, w, h, effective_rows);
        }

        // Row h-effective_rows-2: horizontal rule before input
        if h > effective_rows + 2 {
            let row = h.saturating_sub(effective_rows + 2);
            let rule = "─".repeat(w as usize);
            self.paint(cur, 0, row, &rule, w, self.theme.border());
        }

        // Memory proposal widget (above plan widget)
        if memory_rows > 0 {
            let first_row = h.saturating_sub(effective_rows - approval_rows - plan_rows + 1);
            self.paint_memory_proposal_widget(state, first_row, w);
        }

        // Plan approval widget (above the tool approval widget)
        if plan_rows > 0 {
            let first_row = h.saturating_sub(effective_rows - approval_rows + 1);
            self.paint_plan_approval_widget(state, first_row, w);
        }

        // Approval widget: rows above the input area (between separator and input)
        if approval_rows > 0 {
            let first_row = h.saturating_sub(approval_rows + input_base_rows + 1);
            self.paint_approval_widget(state, first_row, w);
        }

        // Rows above overlay: input area
        if h > input_base_rows + 1 {
            self.paint_input(state, cur, w, h, input_base_rows);
        }

        // Overlay rows: autocomplete dropdown, reverse-search bar, or launcher (mutually exclusive).
        if overlay_rows > 0 {
            if state.is_autocomplete_active() {
                self.paint_autocomplete_overlay(state, cur, w, h, overlay_rows);
            } else if let Some((query, matched)) = state.reverse_search_view() {
                let row = h.saturating_sub(overlay_rows + 1);
                let text = format!("search: {} → {}", query, matched);
                let display: String = text.chars().take(w as usize).collect();
                self.paint(cur, 0, row, &display, w, base);
            } else if let Some((query, entries)) = state.launcher_view(5) {
                self.paint_launcher_overlay(cur, w, h, overlay_rows, &query, &entries);
            }
        }

        // Row h-1: status bar
        if state.is_busy {
            self.spin_tick = self.spin_tick.wrapping_add(1);
        }
        if h > 1 {
            self.paint_status_bar(state, cur, w, h);
        }

        // Input cursor position
        let (cx, cy) = if h > input_base_rows + 1 {
            let prefix_len = 2usize;
            let avail = w.saturating_sub(prefix_len as u16) as usize;
            let (_, cursor_row, cursor_col) =
                state.input_display_lines(avail.max(1), MAX_INPUT_ROWS);
            let x = (prefix_len + cursor_col).min(w as usize) as u16;
            let y = h.saturating_sub(input_base_rows + 1) + cursor_row as u16;
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

    #[cfg(test)]
    fn rendered_cell_style(&self, x: u16, y: u16) -> PackedStyle {
        let rendered = 1 - self.current;
        self.frames[rendered].get(x, y).style
    }

    #[cfg(test)]
    fn rendered_cell_text(&self, x: u16, y: u16) -> &str {
        let rendered = 1 - self.current;
        let cell = self.frames[rendered].get(x, y);
        self.symbols.get(cell.symbol_id)
    }

    fn paint_header(&mut self, state: &AppState, cur: usize, w: u16) {
        let name = format!(" {} ", state.app_name);
        let sep = " | ";
        let hints = "Ctrl+Q quit | Enter send ";

        let name_len = name.chars().count() as u16;
        let sep1_len = sep.chars().count() as u16;
        let hints_len = hints.chars().count() as u16;

        self.paint(cur, 0, 0, &name, name_len.min(w), self.theme.chip_accent());
        if w > name_len {
            self.paint(
                cur,
                name_len,
                0,
                sep,
                sep1_len.min(w - name_len),
                self.theme.border(),
            );
        }
        let hints_col = name_len + sep1_len;
        if w > hints_col {
            self.paint(
                cur,
                hints_col,
                0,
                hints,
                hints_len.min(w - hints_col),
                self.theme.dim(),
            );
        }

        let (label, label_style) = if state.pending_approval.is_some() {
            ("● awaiting approval", self.theme.chip_warning())
        } else if state.status == "error" {
            ("● error", self.theme.chip_danger())
        } else if state.status == "ready" {
            ("● ready", self.theme.dim())
        } else {
            ("● generating", self.theme.chip_accent())
        };
        let label_len = label.chars().count() as u16;
        let left_used = name_len + sep1_len + hints_len;
        if w > label_len && w.saturating_sub(label_len) > left_used {
            let col = w.saturating_sub(label_len);
            self.paint(cur, col, 0, label, label_len, label_style);
        }
    }

    fn paint_status_bar(&mut self, state: &AppState, cur: usize, w: u16, h: u16) {
        let row = h.saturating_sub(1);

        if state.show_activity {
            let (prefix, prefix_style, text_style) = if state.pending_approval.is_some() {
                ("! ", self.theme.chip_warning(), self.theme.muted())
            } else if state.is_busy {
                let frame = SPINNER[self.spin_tick as usize / 8 % SPINNER.len()];
                let s: &'static str = match frame {
                    '-' => "- ",
                    '\\' => "\\ ",
                    '|' => "| ",
                    '/' => "/ ",
                    _ => "  ",
                };
                (s, self.theme.chip_accent(), self.theme.muted())
            } else {
                ("", self.theme.dim(), self.theme.dim())
            };

            let prefix_len = prefix.chars().count() as u16;
            let status_text = format!(" {}", state.status);
            let text_len = status_text.chars().count() as u16;

            if prefix_len > 0 && w > 1 {
                self.paint(cur, 1, row, prefix, prefix_len.min(w - 1), prefix_style);
            }
            let text_col = 1 + prefix_len;
            if w > text_col {
                self.paint(
                    cur,
                    text_col,
                    row,
                    &status_text,
                    text_len.min(w - text_col),
                    text_style,
                );
            }
        }

        {
            let label = match state.project_label {
                Some(ref name) => format!(" project: {name} "),
                None => " no project ".to_string(),
            };
            let label_len = label.chars().count() as u16;
            if w > label_len + 20 {
                let col = (w / 2).saturating_sub(label_len / 2);
                self.paint(cur, col, row, &label, label_len, self.theme.muted());
            }
        }

        if let Some(pct) = state.context_pct {
            let indicator = format!(" ctx: {pct}% ");
            let ind_len = indicator.chars().count() as u16;
            if w > ind_len {
                let col = w.saturating_sub(ind_len);
                let color = if pct < 50 {
                    CTX_LOW
                } else if pct <= 75 {
                    CTX_MID
                } else {
                    CTX_HIGH
                };
                self.paint(
                    cur,
                    col,
                    row,
                    &indicator,
                    ind_len,
                    PackedStyle::new(color, self.theme.background),
                );
            }
        }
    }

    // paint_transcript mutates state.max_scroll and
    // state.visible_collapsible_ids as a render side effect.
    // This coupling is load-bearing: collapsible viewport focus
    // navigation depends on visible_collapsible_ids being populated
    // during render. Justified exception to the renderer-reads-only
    // rule — documented here intentionally.
    fn paint_transcript(
        &mut self,
        state: &mut AppState,
        cur: usize,
        w: u16,
        h: u16,
        effective_rows: u16,
    ) {
        let transcript_height = h.saturating_sub(effective_rows + 3) as usize;

        let dim = self.theme.dim();
        let base = self.theme.base();

        if state.messages.is_empty() {
            self.paint(cur, 0, 2, "  type a message, or / for commands.", w, dim);
            return;
        }

        let lines = self.build_transcript_lines(state, w);

        let max_scroll = lines.len().saturating_sub(transcript_height);
        state.max_scroll = max_scroll;

        if let Some(msg_idx) = state.scroll_to_message_idx.take() {
            if let Some(target_line) = lines.iter().position(|(_, src)| *src == Some(msg_idx)) {
                let upper_third = transcript_height / 3;
                let desired_start = target_line.saturating_sub(upper_third);
                state.scroll_offset = max_scroll.saturating_sub(desired_start).min(max_scroll);
            }
        }

        let offset = state.scroll_offset.min(max_scroll);
        let end = lines.len().saturating_sub(offset);
        let start = end.saturating_sub(transcript_height);
        let visible = &lines[start..end];

        {
            let mut seen = std::collections::HashSet::new();
            let mut ids: Vec<usize> = visible
                .iter()
                .filter_map(|(_, idx)| *idx)
                .filter(|&idx| {
                    state
                        .messages
                        .get(idx)
                        .map(|m| m.is_collapsible)
                        .unwrap_or(false)
                        && seen.insert(idx)
                })
                .collect();
            ids.sort_unstable();
            state.visible_collapsible_ids = ids;
        }

        let cap = h.saturating_sub(effective_rows + 1);

        for (idx, (spans, _msg_idx)) in visible.iter().enumerate() {
            let row = 2 + idx as u16;
            if row >= cap {
                break;
            }
            let mut col: u16 = 0;
            for (text, style) in spans {
                if col >= w {
                    break;
                }
                let avail = w.saturating_sub(col);
                self.paint(cur, col, row, text, avail, *style);
                let text_w = text
                    .chars()
                    .map(|c| UnicodeWidthChar::width(c).unwrap_or(1))
                    .sum::<usize>() as u16;
                col = col.saturating_add(text_w.min(avail));
            }
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

    fn paint_launcher_overlay(
        &mut self,
        cur: usize,
        w: u16,
        h: u16,
        overlay_rows: u16,
        query: &str,
        entries: &[(&crate::tui::commands::LauncherCommand, bool)],
    ) {
        let accent = self.theme.chip_accent();
        let dim = self.theme.dim();
        let mut row_offset: u16 = 0;
        {
            let row = h.saturating_sub(overlay_rows - row_offset + 1);
            let text = format!("/ {}", query);
            let display: String = text.chars().take(w as usize).collect();
            self.paint(cur, 0, row, &display, w, dim);
            row_offset += 1;
        }
        let name_col: usize = 14;
        for (cmd, selected) in entries {
            let row = h.saturating_sub(overlay_rows - row_offset + 1);
            let marker = if *selected { "→ " } else { "  " };
            let style = if *selected { accent } else { dim };
            let name: String = cmd.name.chars().take(name_col).collect();
            let pad = name_col.saturating_sub(name.chars().count());
            let desc_w = (w as usize).saturating_sub(name_col + 4);
            let desc: String = cmd.description.chars().take(desc_w).collect();
            let text = format!("{}{}{}  {}", marker, name, " ".repeat(pad), desc);
            let display: String = text.chars().take(w as usize).collect();
            self.paint(cur, 0, row, &display, w, style);
            row_offset += 1;
        }
    }

    fn paint_input(&mut self, state: &AppState, cur: usize, w: u16, h: u16, input_base_rows: u16) {
        let first_row = h.saturating_sub(input_base_rows + 1);
        let base = self.theme.base();
        let is_generating =
            state.status != "ready" && state.status != "error" && state.pending_approval.is_none();
        let prefix_style = if is_generating {
            self.theme.chip_accent()
        } else {
            self.theme.muted()
        };
        let prefix = if state.is_launcher_active() {
            ": "
        } else {
            "> "
        };
        let prefix_w = prefix.len() as u16;
        let avail = w.saturating_sub(prefix_w) as usize;
        let (visible_lines, _, _) = state.input_display_lines(avail.max(1), MAX_INPUT_ROWS);
        for (i, line) in visible_lines.iter().enumerate() {
            let row = first_row + i as u16;
            if i == 0 {
                self.paint(cur, 0, row, prefix, prefix_w, prefix_style);
            } else {
                self.paint(cur, 0, row, "  ", prefix_w, prefix_style);
            }
            self.paint(cur, prefix_w, row, line, w.saturating_sub(prefix_w), base);
        }
    }

    fn approval_kind_label(tool_name: &str) -> &'static str {
        match tool_name {
            "edit_file" => "Edit File",
            "write_file" => "Write File",
            "shell" => "Shell Command",
            _ => "Tool Action",
        }
    }

    fn paint_approval_widget(&mut self, state: &AppState, first_row: u16, w: u16) {
        let Some(ref approval) = state.pending_approval else {
            return;
        };
        let cur = self.current;
        let dim = self.theme.dim();
        let label_style = match approval.risk {
            ApprovalRisk::High => self.theme.chip_danger(),
            ApprovalRisk::Medium => self.theme.chip_warning(),
            ApprovalRisk::Low => self.theme.chip_accent(),
        };
        let kind_label = Self::approval_kind_label(approval.tool_name.as_str());
        let label = format!("! {}  {}", kind_label, approval.summary);
        self.paint(cur, 0, first_row, &label, w, label_style);

        let mut offset: u16 = 1;

        // Stronger confirmation for actions that cannot be undone.
        if approval.irreversible {
            let warning = "⚠ This action cannot be undone";
            self.paint(
                cur,
                0,
                first_row + offset,
                warning,
                w,
                self.theme.chip_danger(),
            );
            offset += 1;
        }

        // Transaction file list (capped at 6).
        let tx_count = approval.transaction_files.len().min(6);
        for (i, file) in approval.transaction_files.iter().take(6).enumerate() {
            let display: String = format!("  · {}", file).chars().take(w as usize).collect();
            self.paint(cur, 0, first_row + offset + i as u16, &display, w, dim);
        }
        offset += tx_count as u16;

        let actual_preview = approval.preview.len().min(4);
        for (i, line) in approval.preview.iter().take(4).enumerate() {
            let display: String = format!("  › {}", line).chars().take(w as usize).collect();
            self.paint(cur, 0, first_row + offset + i as u16, &display, w, dim);
        }
        offset += actual_preview as u16;

        let evidence_count = approval.evidence.len().min(4);
        for (i, ev) in approval.evidence.iter().take(4).enumerate() {
            let ev_text = format!("  › {}", ev);
            let display: String = ev_text.chars().take(w as usize).collect();
            self.paint(cur, 0, first_row + offset + i as u16, &display, w, dim);
        }
        offset += evidence_count as u16;

        if !approval.impact.is_empty() {
            let joined = approval.impact.join(", ");
            let display: String = format!("  ↳ affects: {joined}")
                .chars()
                .take(w as usize)
                .collect();
            self.paint(cur, 0, first_row + offset, &display, w, dim);
            offset += 1;
        }

        if let Some(ref reason) = approval.reason {
            let display: String = format!("  ↳ {reason}").chars().take(w as usize).collect();
            self.paint(cur, 0, first_row + offset, &display, w, dim);
            offset += 1;
        }

        self.paint(
            cur,
            0,
            first_row + offset,
            "  ^Y approve   ^N reject",
            w,
            dim,
        );
    }

    fn paint_plan_approval_widget(&mut self, state: &AppState, first_row: u16, w: u16) {
        let Some(ref plan) = state.pending_plan_approval else {
            return;
        };
        let cur = self.current;
        let dim = self.theme.dim();
        let label_style = self.theme.chip_warning();
        let label = format!("  Plan: {}", plan.goal);
        self.paint(cur, 0, first_row, &label, w, label_style);
        for (i, (title, desc)) in plan.steps.iter().enumerate() {
            let row_text = format!("  {}. {}: {}", i + 1, title, desc);
            let display: String = row_text.chars().take(w as usize).collect();
            self.paint(cur, 0, first_row + 1 + i as u16, &display, w, dim);
        }
        let control_row = first_row + 1 + plan.steps.len() as u16;
        self.paint(cur, 0, control_row, "  ^Y approve   ^N abandon", w, dim);
    }

    fn paint_memory_proposal_widget(&mut self, state: &AppState, first_row: u16, w: u16) {
        let Some(ref proposal) = state.pending_memory_proposal else {
            return;
        };
        let cur = self.current;
        let dim = self.theme.dim();
        let label_style = self.theme.chip_warning();
        let header = if proposal.delete {
            format!("  Forget this fact: {}", proposal.fact)
        } else {
            format!("  Propose memory: {}", proposal.fact)
        };
        let display: String = header.chars().take(w as usize).collect();
        self.paint(cur, 0, first_row, &display, w, label_style);
        let cat_text = format!("  Category: {}", proposal.category);
        let cat_display: String = cat_text.chars().take(w as usize).collect();
        self.paint(cur, 0, first_row + 1, &cat_display, w, dim);
        let control_row = if let Some(ref scope) = proposal.scope {
            let scope_text = format!("  Scope: {}", scope);
            let scope_display: String = scope_text.chars().take(w as usize).collect();
            self.paint(cur, 0, first_row + 2, &scope_display, w, dim);
            first_row + 3
        } else {
            first_row + 2
        };
        let controls = if proposal.delete {
            "  ^Y confirm   ^N cancel"
        } else {
            "  ^Y remember   ^N discard"
        };
        self.paint(cur, 0, control_row, controls, w, dim);
    }

    fn paint_autocomplete_overlay(
        &mut self,
        state: &AppState,
        cur: usize,
        w: u16,
        h: u16,
        overlay_rows: u16,
    ) {
        let accent = self.theme.chip_accent();
        let dim = self.theme.dim();
        let items = state.autocomplete_preview_items(4);
        for (i, (item, selected)) in items.iter().enumerate() {
            let row = h.saturating_sub(overlay_rows - i as u16 + 1);
            let marker = if *selected { "→ " } else { "  " };
            let style = if *selected { accent } else { dim };
            let text = format!("{}{}", marker, item);
            let display: String = text.chars().take(w as usize).collect();
            self.paint(cur, 0, row, &display, w, style);
        }
    }
}

pub(super) fn wrap_text(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return vec![String::new()];
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut col = 0usize;
    for ch in text.chars() {
        if ch == '\n' {
            lines.push(current);
            current = String::new();
            col = 0;
            continue;
        }
        let cw = UnicodeWidthChar::width(ch).unwrap_or(1);
        current.push(ch);
        col += cw;
        if col >= width {
            lines.push(current);
            current = String::new();
            col = 0;
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
            project_label: None,
            thunk_dir: dir.path().join(".thunk"),
            config_file: dir.path().join("config.toml"),
            data_dir: dir.path().join("data"),
            logs_dir: dir.path().join("logs"),
            session_db: dir.path().join("data").join("sessions.db"),
            home_mcp_config: None,
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

    #[test]
    fn user_message_first_line_has_badge() {
        let (_dir, mut state) = make_state();
        state.messages.clear();
        state.add_user_message("hello world");
        let renderer = Renderer::new(80, 24);
        let lines = renderer.build_transcript_lines(&state, 80);
        let first = lines.iter().find(|(spans, _)| !spans.is_empty()).unwrap();
        assert_eq!(first.0[0].0, "│ ");
        assert_eq!(first.0[1].0, "you");
    }

    #[test]
    fn assistant_message_first_line_has_badge() {
        let (_dir, mut state) = make_state();
        state.messages.clear();
        state.add_assistant_message("hello world");
        let renderer = Renderer::new(80, 24);
        let lines = renderer.build_transcript_lines(&state, 80);
        let first = lines.iter().find(|(spans, _)| !spans.is_empty()).unwrap();
        assert_eq!(first.0[0].0, "│ ");
        assert_eq!(first.0[1].0, "assistant");
    }

    #[test]
    fn continuation_lines_have_badge_indent() {
        let (_dir, mut state) = make_state();
        state.messages.clear();
        // With w=30: body_w = max(30 - (2+9+2), 8) = 17; 35 chars wraps into 3 lines.
        state.add_assistant_message("a".repeat(35));
        let renderer = Renderer::new(30, 24);
        let lines = renderer.build_transcript_lines(&state, 30);
        let content: Vec<_> = lines
            .iter()
            .filter(|(spans, _)| !spans.is_empty())
            .collect();
        assert!(content.len() > 1, "message should produce multiple lines");
        let second = &content[1].0;
        assert_eq!(second[0].0, "│ ");
        assert_eq!(second[1].0, " ".repeat(11)); // "assistant"(9) + "  "(2)
    }

    #[test]
    fn collapsed_message_renders_as_summary() {
        let (_dir, mut state) = make_state();
        state.messages.clear();
        state.add_collapsible_tool_message("this is a tool result");
        let msg_idx = state.messages.len() - 1;
        state.collapsed_message_indices.insert(msg_idx);
        let renderer = Renderer::new(80, 24);
        let lines = renderer.build_transcript_lines(&state, 80);
        let summary = lines.iter().find(|(spans, _)| !spans.is_empty()).unwrap();
        assert!(summary.0.iter().any(|(text, _)| text.contains('›')));
    }

    #[test]
    fn generation_cursor_appended_when_busy() {
        let (_dir, mut state) = make_state();
        state.messages.clear();
        state.add_assistant_message("hello");
        state.is_busy = true;
        let renderer = Renderer::new(80, 24);
        let lines = renderer.build_transcript_lines(&state, 80);
        let last_content = lines
            .iter()
            .filter(|(spans, _)| !spans.is_empty())
            .last()
            .unwrap();
        let last_span = last_content.0.last().unwrap();
        assert_eq!(last_span.0, "▍");
    }

    #[test]
    fn generation_cursor_not_shown_on_completed_response_before_stream_starts() {
        // Simulates the pre-stream phase: is_busy=true but AssistantMessageStarted
        // has not fired yet — last message is the user prompt, not an assistant.
        let (_dir, mut state) = make_state();
        state.messages.clear();
        state.add_assistant_message("previous response");
        state.add_user_message("new question");
        state.is_busy = true;
        let renderer = Renderer::new(80, 24);
        let lines = renderer.build_transcript_lines(&state, 80);
        for (spans, _) in &lines {
            if let Some(last) = spans.last() {
                assert_ne!(last.0, "▍", "cursor must not appear on completed message");
            }
        }
    }

    #[test]
    fn paint_input_prefix_is_muted_when_ready() {
        let (_dir, mut state) = make_state();
        // status starts as "ready" and pending_approval is None — is_generating = false
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();
        // input row: h - input_base_rows - 1 = 24 - 1 - 1 = 22; prefix at col 0
        let cell_style = renderer.rendered_cell_style(0, 22);
        assert_eq!(
            cell_style,
            renderer.theme.muted(),
            "prefix must be muted when status is ready"
        );
    }

    #[test]
    fn paint_input_prefix_is_accent_when_generating() {
        let (_dir, mut state) = make_state();
        state.set_status("generating...");
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();
        let cell_style = renderer.rendered_cell_style(0, 22);
        assert_eq!(
            cell_style,
            renderer.theme.chip_accent(),
            "prefix must be accent when actively generating"
        );
    }

    #[test]
    fn paint_input_prefix_is_muted_during_approval_wait() {
        use crate::tui::state::{ApprovalRisk, PendingApprovalState};
        let (_dir, mut state) = make_state();
        state.set_status("awaiting approval");
        state.pending_approval = Some(PendingApprovalState {
            tool_name: "shell".to_string(),
            summary: "run cargo test".to_string(),
            risk: ApprovalRisk::Low,
            irreversible: false,
            evidence: vec![],
            preview: vec![],
            transaction_files: vec![],
            impact: vec![],
            reason: None,
        });
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();
        let cell_style = renderer.rendered_cell_style(0, 22);
        assert_eq!(
            cell_style,
            renderer.theme.muted(),
            "prefix must be muted while awaiting approval"
        );
    }

    #[test]
    fn approval_kind_label_maps_all_variants() {
        assert_eq!(Renderer::approval_kind_label("edit_file"), "Edit File");
        assert_eq!(Renderer::approval_kind_label("write_file"), "Write File");
        assert_eq!(Renderer::approval_kind_label("shell"), "Shell Command");
        assert_eq!(Renderer::approval_kind_label("unknown_tool"), "Tool Action");
    }

    #[test]
    fn approval_widget_evidence_has_chevron_gutter() {
        // 80×24: approval_rows = 1 + 1 + 0 + 1 = 3; effective_rows = 4
        // first_row = 24 - 4 - 1 = 19; evidence row = 19 + 1 + 0 + 0 = 20
        use crate::tui::state::{ApprovalRisk, PendingApprovalState};
        let (_dir, mut state) = make_state();
        state.pending_approval = Some(PendingApprovalState {
            tool_name: "shell".to_string(),
            summary: "run".to_string(),
            risk: ApprovalRisk::Low,
            irreversible: false,
            evidence: vec!["some evidence".to_string()],
            preview: vec![],
            transaction_files: vec![],
            impact: vec![],
            reason: None,
        });
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();
        // col 2 = the › character in "  › some evidence"
        assert_eq!(
            renderer.rendered_cell_text(2, 20),
            "›",
            "evidence row must start with › gutter at col 2"
        );
        assert_eq!(
            renderer.rendered_cell_style(2, 20),
            renderer.theme.dim(),
            "evidence row must be dim"
        );
    }

    #[test]
    fn approval_widget_empty_evidence_skips_evidence_rows() {
        // 80×24 with no evidence: approval_rows = 2; first_row = 24 - 3 - 1 = 20
        // With 1 evidence entry:  approval_rows = 3; first_row = 24 - 4 - 1 = 19
        // Row 19 must be label-style when evidence present, plain when absent.
        use crate::tui::state::{ApprovalRisk, PendingApprovalState};
        let (_dir, mut state) = make_state();
        state.pending_approval = Some(PendingApprovalState {
            tool_name: "shell".to_string(),
            summary: "run".to_string(),
            risk: ApprovalRisk::Low,
            irreversible: false,
            evidence: vec![],
            preview: vec![],
            transaction_files: vec![],
            impact: vec![],
            reason: None,
        });
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();
        // Row 19 must NOT be the label (chip_accent): label is at row 20
        assert_ne!(
            renderer.rendered_cell_style(0, 19),
            renderer.theme.chip_accent(),
            "row 19 must not be the label row when evidence is empty"
        );
        // Row 20 must be the label (chip_accent for Low risk)
        assert_eq!(
            renderer.rendered_cell_style(0, 20),
            renderer.theme.chip_accent(),
            "label must be at row 20 when evidence is empty"
        );
    }

    #[test]
    fn approval_widget_renders_reason_row_when_present() {
        // 80×24 with a reason and no evidence/impact: approval_rows = 1+0+0+0+0+0+1+1 = 3
        // first_row = 24 - (3+1+1) = 19; reason row = 19 + 1 (offset after label) = 20.
        use crate::tui::state::{ApprovalRisk, PendingApprovalState};
        let (_dir, mut state) = make_state();
        let reason_text = "proposing because src/foo.rs was modified";
        state.pending_approval = Some(PendingApprovalState {
            tool_name: "shell".to_string(),
            summary: "run".to_string(),
            risk: ApprovalRisk::Low,
            irreversible: false,
            evidence: vec![],
            preview: vec![],
            transaction_files: vec![],
            impact: vec![],
            reason: Some(reason_text.to_string()),
        });
        let mut renderer = Renderer::new(80, 24);
        let mut out = Vec::<u8>::new();
        renderer
            .render(&mut state, &mut out, DirtySections::ALL)
            .unwrap();

        let expected = format!("  ↳ {reason_text}");
        let rendered: String = (0..expected.chars().count())
            .map(|i| {
                renderer
                    .rendered_cell_text(i as u16, 20)
                    .chars()
                    .next()
                    .unwrap_or(' ')
            })
            .collect();
        assert_eq!(
            rendered, expected,
            "reason row must render the reason text at row 20"
        );
    }

    #[test]
    fn approval_widget_absent_reason_skips_reason_row() {
        // No reason: approval_rows = 2 (label + controls) -> label at row 20.
        // With reason: approval_rows = 3 -> label shifts to row 19, no blank gap.
        use crate::tui::state::{ApprovalRisk, PendingApprovalState};

        let render_with_reason = |reason: Option<String>| {
            let (_dir, mut state) = make_state();
            state.pending_approval = Some(PendingApprovalState {
                tool_name: "shell".to_string(),
                summary: "run".to_string(),
                risk: ApprovalRisk::Low,
                irreversible: false,
                evidence: vec![],
                preview: vec![],
                transaction_files: vec![],
                impact: vec![],
                reason,
            });
            let mut renderer = Renderer::new(80, 24);
            let mut out = Vec::<u8>::new();
            renderer
                .render(&mut state, &mut out, DirtySections::ALL)
                .unwrap();
            renderer
        };

        let without = render_with_reason(None);
        let with = render_with_reason(Some(
            "proposing because src/foo.rs was modified".to_string(),
        ));

        assert_eq!(
            without.rendered_cell_style(0, 20),
            without.theme.chip_accent(),
            "label must be at row 20 when reason is absent"
        );
        assert_eq!(
            with.rendered_cell_style(0, 19),
            with.theme.chip_accent(),
            "label must shift to row 19 when reason is present (one extra row, no gap)"
        );
    }

    #[test]
    fn approval_rows_accounts_for_evidence_count() {
        // 2 evidence entries → approval_rows = 4 → separator at row 17
        // 0 evidence entries → approval_rows = 2 → separator at row 19
        use crate::tui::state::{ApprovalRisk, PendingApprovalState};

        let render_with_evidence = |count: usize| {
            let (_dir, mut state) = make_state();
            state.pending_approval = Some(PendingApprovalState {
                tool_name: "shell".to_string(),
                summary: "run".to_string(),
                risk: ApprovalRisk::Low,
                irreversible: false,
                evidence: (0..count).map(|i| format!("ev{}", i)).collect(),
                preview: vec![],
                transaction_files: vec![],
                impact: vec![],
                reason: None,
            });
            let mut renderer = Renderer::new(80, 24);
            let mut out = Vec::<u8>::new();
            renderer
                .render(&mut state, &mut out, DirtySections::ALL)
                .unwrap();
            renderer
        };

        let r2 = render_with_evidence(2);
        // separator (border style) at row 17 when 2 evidence entries
        assert_eq!(
            r2.rendered_cell_style(0, 17),
            r2.theme.border(),
            "separator must be at row 17 with 2 evidence entries"
        );

        let r0 = render_with_evidence(0);
        // separator at row 19 when no evidence entries
        assert_eq!(
            r0.rendered_cell_style(0, 19),
            r0.theme.border(),
            "separator must be at row 19 with no evidence"
        );
    }
}
