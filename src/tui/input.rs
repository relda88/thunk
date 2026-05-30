use super::state::{AppState, DirtySections};

/// Defines methods for modifying the input buffer and cursor position in the app state
impl AppState {
    /// Inserts a character at the current cursor position and moves the cursor forward
    pub fn insert_char(&mut self, c: char) {
        self.input.insert(self.cursor, c);
        self.cursor += c.len_utf8();
        self.history_cursor = None;
        self.history_draft = None;
        self.exit_reverse_search();
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Inserts a string at the current cursor position and moves the cursor forward
    pub fn insert_str(&mut self, s: &str) {
        self.input.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.history_cursor = None;
        self.history_draft = None;
        self.exit_reverse_search();
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
        self.history_cursor = None;
        self.history_draft = None;
        self.exit_reverse_search();
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
        self.history_cursor = None;
        self.history_draft = None;
        self.exit_reverse_search();
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
        self.history_cursor = None;
        self.history_draft = None;
        self.exit_reverse_search();
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

    pub fn recall_previous_input(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        let next_index = match self.history_cursor {
            Some(current) if current > 0 => current - 1,
            Some(current) => current,
            None => {
                self.history_draft = Some(self.input.clone());
                self.input_history.len() - 1
            }
        };
        self.history_cursor = Some(next_index);
        let text = self.input_history[next_index].clone();
        self.set_input_text(text);
    }

    pub fn recall_next_input(&mut self) {
        let Some(current) = self.history_cursor else {
            return;
        };
        if current + 1 < self.input_history.len() {
            self.history_cursor = Some(current + 1);
            let text = self.input_history[current + 1].clone();
            self.set_input_text(text);
        } else {
            let draft = self.history_draft.take().unwrap_or_default();
            self.history_cursor = None;
            self.set_input_text(draft);
        }
    }

    pub fn is_reverse_search_active(&self) -> bool {
        self.reverse_search_active
    }

    pub fn activate_reverse_search(&mut self) {
        if self.input_history.is_empty() {
            return;
        }
        if !self.reverse_search_active {
            self.reverse_search_active = true;
            self.reverse_search_query.clear();
            self.reverse_search_selection = 0;
            self.reverse_search_draft = Some(self.input.clone());
        }
        self.apply_reverse_search_match();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub fn reverse_search_push_char(&mut self, c: char) {
        if !self.reverse_search_active {
            return;
        }
        self.reverse_search_query.push(c);
        self.reverse_search_selection = 0;
        self.apply_reverse_search_match();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub fn reverse_search_backspace(&mut self) {
        if !self.reverse_search_active {
            return;
        }
        self.reverse_search_query.pop();
        self.reverse_search_selection = 0;
        self.apply_reverse_search_match();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub fn reverse_search_cycle(&mut self) {
        if !self.reverse_search_active {
            self.activate_reverse_search();
            return;
        }
        let matches = self.reverse_search_matches();
        if matches.is_empty() {
            return;
        }
        self.reverse_search_selection = (self.reverse_search_selection + 1) % matches.len();
        let text = self.input_history[matches[self.reverse_search_selection]].clone();
        self.set_input_text(text);
    }

    pub fn accept_reverse_search(&mut self) {
        self.exit_reverse_search();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub fn cancel_reverse_search(&mut self) {
        if !self.reverse_search_active {
            return;
        }
        let draft = self.reverse_search_draft.clone().unwrap_or_default();
        self.exit_reverse_search();
        self.set_input_text(draft);
    }

    pub fn reverse_search_view(&self) -> Option<(String, String)> {
        if !self.reverse_search_active {
            return None;
        }
        Some((self.reverse_search_query.clone(), self.input.clone()))
    }

    fn set_input_text(&mut self, text: String) {
        self.input = text;
        self.cursor = self.input.len();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub(crate) fn exit_reverse_search(&mut self) {
        self.reverse_search_active = false;
        self.reverse_search_query.clear();
        self.reverse_search_selection = 0;
        self.reverse_search_draft = None;
    }

    fn reverse_search_matches(&self) -> Vec<usize> {
        let query = self.reverse_search_query.to_lowercase();
        self.input_history
            .iter()
            .enumerate()
            .rev()
            .filter_map(|(i, entry)| {
                if query.is_empty() || entry.to_lowercase().contains(&query) {
                    Some(i)
                } else {
                    None
                }
            })
            .collect()
    }

    fn apply_reverse_search_match(&mut self) {
        let matches = self.reverse_search_matches();
        if matches.is_empty() {
            return;
        }
        self.reverse_search_selection = self
            .reverse_search_selection
            .min(matches.len().saturating_sub(1));
        let text = self.input_history[matches[self.reverse_search_selection]].clone();
        self.set_input_text(text);
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::app::paths::AppPaths;
    use crate::core::config::Config;
    use crate::tui::state::AppState;

    fn make_state() -> AppState {
        let config = Config::default();
        let paths = AppPaths {
            root_dir: PathBuf::from("/tmp"),
            project_root: PathBuf::from("/tmp"),
            config_file: PathBuf::from("/tmp/config.toml"),
            data_dir: PathBuf::from("/tmp/data"),
            logs_dir: PathBuf::from("/tmp/logs"),
            session_db: PathBuf::from("/tmp/data/sessions.db"),
        };
        AppState::new(&config, &paths)
    }

    #[test]
    fn history_pushed_on_submit_not_for_slash_commands() {
        let mut state = make_state();
        state.input = "hello world".into();
        state.cursor = state.input.len();
        let _ = state.submit_input();
        assert_eq!(state.input_history, vec!["hello world"]);

        state.input = "/approve".into();
        state.cursor = state.input.len();
        let _ = state.submit_input();
        assert_eq!(
            state.input_history,
            vec!["hello world"],
            "/approve must not push to history"
        );

        state.input = "/reject".into();
        state.cursor = state.input.len();
        let _ = state.submit_input();
        assert_eq!(
            state.input_history,
            vec!["hello world"],
            "/reject must not push to history"
        );
    }

    #[test]
    fn history_draft_stash_and_restore() {
        let mut state = make_state();
        state.input_history = vec!["first".into(), "second".into()];
        state.input = "draft".into();
        state.cursor = state.input.len();

        state.recall_previous_input();
        assert_eq!(state.input, "second");
        assert_eq!(state.history_cursor, Some(1));
        assert_eq!(state.history_draft, Some("draft".into()));

        state.recall_previous_input();
        assert_eq!(state.input, "first");
        assert_eq!(state.history_cursor, Some(0));

        state.recall_next_input();
        assert_eq!(state.input, "second");
        assert_eq!(state.history_cursor, Some(1));

        state.recall_next_input();
        assert_eq!(state.input, "draft", "draft must be restored");
        assert_eq!(state.history_cursor, None, "cursor must reset to present");
    }

    #[test]
    fn cancel_reverse_search_restores_draft() {
        let mut state = make_state();
        state.input_history = vec!["old prompt".into()];
        state.input = "my draft".into();
        state.cursor = state.input.len();

        state.activate_reverse_search();
        assert!(state.reverse_search_active);
        assert_eq!(state.reverse_search_draft, Some("my draft".into()));

        state.cancel_reverse_search();
        assert!(!state.reverse_search_active);
        assert_eq!(
            state.input, "my draft",
            "original draft must be restored exactly"
        );
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
