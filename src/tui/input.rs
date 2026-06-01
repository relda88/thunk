use super::commands::{launcher_commands, LauncherCommand};
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
        self.clear_autocomplete();
        self.mark_dirty(DirtySections::INPUT);
    }

    /// Inserts a string at the current cursor position and moves the cursor forward
    pub fn insert_str(&mut self, s: &str) {
        self.input.insert_str(self.cursor, s);
        self.cursor += s.len();
        self.history_cursor = None;
        self.history_draft = None;
        self.exit_reverse_search();
        self.clear_autocomplete();
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
        self.clear_autocomplete();
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
        self.clear_autocomplete();
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
        self.clear_autocomplete();
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
        self.clear_autocomplete();
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
        self.clear_autocomplete();
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
        self.clear_autocomplete();
        self.exit_launcher();
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

    pub(crate) fn is_launcher_active(&self) -> bool {
        self.launcher_active
    }

    pub(crate) fn activate_launcher(&mut self) {
        self.exit_reverse_search();
        self.clear_autocomplete();
        self.launcher_active = true;
        self.launcher_query.clear();
        self.launcher_index = 0;
        self.apply_launcher_filter();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub(crate) fn cancel_launcher(&mut self) {
        self.exit_launcher();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub(crate) fn accept_launcher(&mut self) {
        if self.launcher_filtered.is_empty() || self.launcher_index >= self.launcher_filtered.len()
        {
            self.cancel_launcher();
            return;
        }
        let name = self.launcher_filtered[self.launcher_index].name;
        let text = format!("{} ", name);
        self.input = text;
        self.cursor = self.input.len();
        self.exit_launcher();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub(crate) fn launcher_push_char(&mut self, c: char) {
        self.launcher_query.push(c);
        self.launcher_index = 0;
        self.apply_launcher_filter();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub(crate) fn launcher_backspace(&mut self) {
        self.launcher_query.pop();
        self.launcher_index = 0;
        self.apply_launcher_filter();
        self.mark_dirty(DirtySections::INPUT);
    }

    pub(crate) fn launcher_cycle(&mut self, reverse: bool) {
        if self.launcher_filtered.is_empty() {
            return;
        }
        let len = self.launcher_filtered.len();
        if reverse {
            self.launcher_index = if self.launcher_index == 0 {
                len - 1
            } else {
                self.launcher_index - 1
            };
        } else {
            self.launcher_index = (self.launcher_index + 1) % len;
        }
        self.mark_dirty(DirtySections::INPUT);
    }

    pub(crate) fn launcher_view(
        &self,
        max: usize,
    ) -> Option<(String, Vec<(&'static LauncherCommand, bool)>)> {
        if !self.launcher_active {
            return None;
        }
        let view_start = self
            .launcher_index
            .saturating_sub(max / 2)
            .min(self.launcher_filtered.len().saturating_sub(max));
        let items = self
            .launcher_filtered
            .iter()
            .enumerate()
            .skip(view_start)
            .take(max)
            .map(|(idx, cmd)| (*cmd, idx == self.launcher_index))
            .collect();
        Some((self.launcher_query.clone(), items))
    }

    fn apply_launcher_filter(&mut self) {
        let query = self.launcher_query.to_lowercase();
        self.launcher_filtered = if query.is_empty() {
            launcher_commands().iter().collect()
        } else {
            launcher_commands()
                .iter()
                .filter(|cmd| {
                    cmd.name.to_lowercase().contains(&query)
                        || cmd.description.to_lowercase().contains(&query)
                })
                .collect()
        };
        if self.launcher_index >= self.launcher_filtered.len() {
            self.launcher_index = self.launcher_filtered.len().saturating_sub(1);
        }
    }

    pub(crate) fn exit_launcher(&mut self) {
        self.launcher_active = false;
        self.launcher_query.clear();
        self.launcher_filtered.clear();
        self.launcher_index = 0;
    }

    // Returns (start=0, end=command_end, prefix=&input[..command_end]).
    // Returns None if input does not start with '/' or cursor is past the first space.
    fn slash_prefix_range(&self) -> Option<(usize, usize, &str)> {
        if !self.input.starts_with('/') {
            return None;
        }
        let safe_cursor = self.cursor.min(self.input.len());
        let active = &self.input[..safe_cursor];
        let command_end = active.find(' ').unwrap_or(active.len());
        if command_end == 0 || safe_cursor > command_end {
            return None;
        }
        Some((0, command_end, &self.input[..command_end]))
    }

    pub(crate) fn clear_autocomplete(&mut self) {
        self.autocomplete_matches.clear();
        self.autocomplete_index = 0;
        self.autocomplete_prefix = None;
    }

    pub(crate) fn is_autocomplete_active(&self) -> bool {
        !self.autocomplete_matches.is_empty()
    }

    pub(crate) fn autocomplete_preview_items(&self, max: usize) -> Vec<(String, bool)> {
        self.autocomplete_matches
            .iter()
            .take(max)
            .enumerate()
            .map(|(idx, value)| (value.clone(), idx == self.autocomplete_index))
            .collect()
    }

    pub(crate) fn autocomplete_command(&mut self, names: &[&str], reverse: bool) -> bool {
        self.exit_reverse_search();

        let (start, end, typed_prefix) = match self.slash_prefix_range() {
            Some(range) => range,
            None => {
                self.clear_autocomplete();
                return false;
            }
        };
        let typed_prefix = typed_prefix.to_string();

        // When already cycling, preserve the original prefix so cycling doesn't narrow.
        let prefix = if !self.autocomplete_matches.is_empty()
            && self.autocomplete_index < self.autocomplete_matches.len()
            && self.autocomplete_matches[self.autocomplete_index] == self.input[..end]
        {
            self.autocomplete_prefix.clone().unwrap_or(typed_prefix)
        } else {
            typed_prefix
        };

        let matches: Vec<String> = names
            .iter()
            .filter(|cmd| cmd.starts_with(prefix.as_str()))
            .map(|cmd| cmd.to_string())
            .collect();

        if matches.is_empty() {
            self.clear_autocomplete();
            return false;
        }

        let same_cycle = self
            .autocomplete_prefix
            .as_ref()
            .map(|existing| existing == &prefix)
            .unwrap_or(false)
            && self.autocomplete_matches == matches;

        if same_cycle {
            if reverse {
                if self.autocomplete_index == 0 {
                    self.autocomplete_index = self.autocomplete_matches.len() - 1;
                } else {
                    self.autocomplete_index -= 1;
                }
            } else {
                self.autocomplete_index =
                    (self.autocomplete_index + 1) % self.autocomplete_matches.len();
            }
        } else {
            self.autocomplete_matches = matches;
            self.autocomplete_prefix = Some(prefix);
            self.autocomplete_index = if reverse {
                self.autocomplete_matches.len() - 1
            } else {
                0
            };
        }

        let selected = self.autocomplete_matches[self.autocomplete_index].clone();
        self.input.replace_range(start..end, &selected);
        self.cursor = start + selected.len();

        // Unique match: append a trailing space so the user can type the subcommand immediately.
        if self.autocomplete_matches.len() == 1 && self.input[self.cursor..].is_empty() {
            self.input.push(' ');
            self.cursor += 1;
        }

        self.mark_dirty(DirtySections::INPUT);
        true
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

    #[test]
    fn autocomplete_command_cycles_forward_through_matches() {
        let mut state = make_state();
        state.input = "/d".to_string();
        state.cursor = 2;

        let names = &["/def", "/diag", "/debug-log"];
        assert!(state.autocomplete_command(names, false));
        assert_eq!(state.input, "/def");

        assert!(state.autocomplete_command(names, false));
        assert_eq!(state.input, "/diag");

        assert!(state.autocomplete_command(names, false));
        assert_eq!(state.input, "/debug-log");

        // Wraps back to first.
        assert!(state.autocomplete_command(names, false));
        assert_eq!(state.input, "/def");
    }

    #[test]
    fn autocomplete_command_cycles_backward_through_matches() {
        let mut state = make_state();
        state.input = "/d".to_string();
        state.cursor = 2;

        let names = &["/def", "/diag", "/debug-log"];
        assert!(state.autocomplete_command(names, true));
        assert_eq!(state.input, "/debug-log");

        assert!(state.autocomplete_command(names, true));
        assert_eq!(state.input, "/diag");
    }

    #[test]
    fn autocomplete_command_unique_match_appends_space() {
        let mut state = make_state();
        state.input = "/reject".to_string();
        state.cursor = state.input.len();

        assert!(state.autocomplete_command(&["/reject"], false));
        assert_eq!(state.input, "/reject ");
        assert_eq!(state.cursor, "/reject ".len());
    }

    #[test]
    fn insert_char_dismisses_autocomplete() {
        let mut state = make_state();
        state.input = "/h".to_string();
        state.cursor = 2;
        state.autocomplete_command(&["/help", "/history"], false);
        assert!(state.is_autocomplete_active());

        state.insert_char('x');
        assert!(!state.is_autocomplete_active());
    }

    #[test]
    fn slash_prefix_range_returns_none_when_cursor_past_first_space() {
        let mut state = make_state();
        state.input = "/help foo".to_string();
        state.cursor = 9; // past the space
        assert!(state.slash_prefix_range().is_none());
    }

    #[test]
    fn slash_prefix_range_returns_none_when_input_does_not_start_with_slash() {
        let mut state = make_state();
        state.input = "hello".to_string();
        state.cursor = 3;
        assert!(state.slash_prefix_range().is_none());
    }

    #[test]
    fn activate_launcher_populates_all_commands() {
        let mut state = make_state();
        state.activate_launcher();
        assert!(state.is_launcher_active());
        let (query, entries) = state.launcher_view(100).unwrap();
        assert!(query.is_empty());
        assert!(!entries.is_empty());
        // All 21 static commands should be present with empty query.
        assert_eq!(
            entries.len(),
            crate::tui::commands::launcher_commands().len()
        );
    }

    #[test]
    fn launcher_push_char_filters_by_name() {
        let mut state = make_state();
        state.activate_launcher();
        state.launcher_push_char('h');
        state.launcher_push_char('e');
        state.launcher_push_char('l');
        let (_, entries) = state.launcher_view(100).unwrap();
        // "hel" should match /help and /history (contains) at minimum.
        assert!(entries.iter().any(|(c, _)| c.name == "/help"));
        for (cmd, _) in &entries {
            assert!(
                cmd.name.contains("hel") || cmd.description.to_lowercase().contains("hel"),
                "unexpected match: {}",
                cmd.name
            );
        }
    }

    #[test]
    fn launcher_backspace_restores_filter() {
        let mut state = make_state();
        state.activate_launcher();
        let total = state.launcher_view(100).unwrap().1.len();
        state.launcher_push_char('z'); // no match
        state.launcher_push_char('z');
        let (_, filtered) = state.launcher_view(100).unwrap();
        assert!(filtered.is_empty());
        state.launcher_backspace();
        state.launcher_backspace();
        let (_, restored) = state.launcher_view(100).unwrap();
        assert_eq!(restored.len(), total);
    }

    #[test]
    fn launcher_cycle_wraps_forward_and_backward() {
        let mut state = make_state();
        state.activate_launcher();
        let len = state.launcher_filtered.len();
        // Cycling backward from index 0 wraps to the last entry.
        state.launcher_cycle(true);
        assert_eq!(state.launcher_index, len - 1);
        // Cycling forward from last wraps to 0.
        state.launcher_cycle(false);
        assert_eq!(state.launcher_index, 0);
    }

    #[test]
    fn accept_launcher_writes_command_to_input_and_clears_launcher() {
        let mut state = make_state();
        state.activate_launcher();
        // Select the first entry.
        let expected_name = state.launcher_filtered[0].name;
        state.accept_launcher();
        assert!(!state.is_launcher_active());
        assert_eq!(state.input, format!("{} ", expected_name));
        assert_eq!(state.cursor, state.input.len());
    }

    #[test]
    fn cancel_launcher_clears_all_fields() {
        let mut state = make_state();
        state.activate_launcher();
        state.launcher_push_char('h');
        assert!(state.is_launcher_active());
        state.cancel_launcher();
        assert!(!state.is_launcher_active());
        assert!(state.launcher_query.is_empty());
        assert!(state.launcher_filtered.is_empty());
        assert_eq!(state.launcher_index, 0);
    }

    #[test]
    fn activate_launcher_dismisses_reverse_search() {
        let mut state = make_state();
        state.input_history.push("previous".to_string());
        state.activate_reverse_search();
        assert!(state.is_reverse_search_active());
        state.activate_launcher();
        assert!(!state.is_reverse_search_active());
        assert!(state.is_launcher_active());
    }

    #[test]
    fn activate_reverse_search_dismisses_launcher() {
        let mut state = make_state();
        state.input_history.push("previous".to_string());
        state.activate_launcher();
        assert!(state.is_launcher_active());
        state.activate_reverse_search();
        assert!(!state.is_launcher_active());
        assert!(state.is_reverse_search_active());
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
