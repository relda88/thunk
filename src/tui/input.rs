use super::state::{AppState, DirtySections};

/// Defines methods for modifying the input buffer and cursor position in the app state
impl AppState {
    /// Inserts a character at the current cursor position and moves the cursor forward
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Inserts a string at the current cursor position and moves the cursor forward
    pub fn insert_str(&mut self, s: &str) {
        self.input.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Deletes the character before the current cursor position and moves the cursor back
    pub fn delete_char_before(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let mut prev = self.cursor - 1;
        while !self.input.is_char_boundary(prev) {
            prev -= 1;
        }

        self.input.remove(prev);
        self.cursor = prev;
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Moves the cursor left, ensuring it stays on valid character boundaries
    pub fn cursor_left(&mut self) {
        if self.cursor == 0 {
            return;
        }

        let mut prev = self.cursor - 1;
        while !self.input.is_char_boundary(prev) {
            prev -= 1;
        }
        self.cursor = prev;
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Moves the cursor right, ensuring it stays on valid character boundaries
    pub fn cursor_right(&mut self) {
        if self.cursor >= self.input.len() {
            return;
        }

        let mut next = self.cursor + 1;
        while next < self.input.len() && !self.input.is_char_boundary(next) {
            next += 1;
        }
        self.cursor = next.min(self.input.len());
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Moves the cursor to the start of the current logical line
    pub fn cursor_home(&mut self) {
        self.cursor = self.current_line_start();
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Moves the cursor to the end of the current logical line
    pub fn cursor_end(&mut self) {
        self.cursor = self.current_line_end();
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Clears the input buffer and resets the cursor position
    pub fn clear_input(&mut self) {
        self.input.clear();
        self.cursor = 0;
        self.mark_dirty(DirtySections::INPUT);
    }

    pub fn insert_newline(&mut self) {
        self.insert_char('\n');
    }

    pub fn delete_word_before(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let before = &self.input[..self.cursor];
        let trim_end = before.trim_end_matches(' ').len();
        let word_start = before[..trim_end].rfind(' ').map(|i| i + 1).unwrap_or(0);
        self.input.drain(word_start..self.cursor);
        self.cursor = word_start;
        self.mark_dirty(DirtySections::INPUT);
    }

    pub fn normalized_paste(text: &str) -> String {
        text.replace("\r\n", "\n").replace('\r', "\n")
    }

    pub fn input_content_rows(&self, width: usize) -> usize {
        wrap_input_for_display(&self.input, width).len().max(1)
    }

    pub fn input_display_lines(
        &self,
        width: usize,
        max_visible_rows: usize,
    ) -> (Vec<String>, usize, usize) {
        let wrapped = wrap_input_for_display(&self.input, width);
        let cursor = cursor_visual_position(&self.input, self.cursor, width);
        let total_rows = wrapped.len().max(1);
        let start_row = if total_rows <= max_visible_rows {
            0
        } else {
            cursor
                .0
                .saturating_add(1)
                .saturating_sub(max_visible_rows)
                .min(total_rows.saturating_sub(max_visible_rows))
        };
        let end_row = (start_row + max_visible_rows).min(total_rows);
        let visible = wrapped[start_row..end_row].to_vec();
        (visible, cursor.0.saturating_sub(start_row), cursor.1)
    }

    fn current_line_start(&self) -> usize {
        self.input[..self.cursor]
            .rfind('\n')
            .map(|idx| idx + 1)
            .unwrap_or(0)
    }

    fn current_line_end(&self) -> usize {
        self.input[self.cursor..]
            .find('\n')
            .map(|offset| self.cursor + offset)
            .unwrap_or(self.input.len())
    }
}

fn wrap_input_for_display(input: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut lines = Vec::new();

    if input.is_empty() {
        return vec![String::new()];
    }

    for raw_line in input.split('\n') {
        let wrapped = wrap_preserving_empty_line(raw_line, width);
        lines.extend(wrapped);
    }

    if input.ends_with('\n') {
        lines.push(String::new());
    }

    if lines.is_empty() {
        vec![String::new()]
    } else {
        lines
    }
}

fn wrap_preserving_empty_line(line: &str, width: usize) -> Vec<String> {
    if line.is_empty() {
        return vec![String::new()];
    }

    let chars: Vec<char> = line.chars().collect();
    let mut wrapped = Vec::new();
    let mut start = 0usize;
    while start < chars.len() {
        let end = (start + width).min(chars.len());
        wrapped.push(chars[start..end].iter().collect());
        start = end;
    }
    wrapped
}

fn cursor_visual_position(input: &str, cursor: usize, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let safe_cursor = cursor.min(input.len());
    let before = &input[..safe_cursor];
    let mut row = 0usize;
    let mut col = 0usize;

    for ch in before.chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
            continue;
        }
        col += 1;
        if col >= width {
            row += 1;
            col = 0;
        }
    }

    (row, col)
}
