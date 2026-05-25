use std::io::{self, Write};

use crossterm::{
    cursor::MoveTo,
    queue,
    style::{Attribute, Color, Print, SetAttribute, SetForegroundColor},
    terminal::{self, Clear, ClearType},
};

use crate::app::Result;

use super::state::{AppState, ChatMessage, MessageKind, Role};

const RESERVED_LINES: u16 = 4;

/// Renders the entire TUI based on the current app state, including header, transcript, input, and status bar
pub fn render(stdout: &mut io::Stdout, state: &mut AppState) -> Result<()> {
    let (width, height) = terminal::size()?;
    let transcript_height = height.saturating_sub(RESERVED_LINES) as usize;

    queue!(stdout, Clear(ClearType::All), MoveTo(0, 0))?;
    draw_header(stdout, state, width)?;
    draw_transcript(stdout, state, width, transcript_height)?;
    draw_input(stdout, state, width, height)?;
    draw_status(stdout, state, width, height)?;
    queue!(
        stdout,
        MoveTo(input_cursor_x(state, width), height.saturating_sub(2))
    )?;
    stdout.flush()?;
    Ok(())
}

/// Draws the header section of the TUI, including the app name and instructions
fn draw_header(stdout: &mut io::Stdout, state: &AppState, width: u16) -> Result<()> {
    let title = format!(" {}  |  Ctrl+Q quit  |  Enter send ", state.app_name);
    queue!(
        stdout,
        SetAttribute(Attribute::Bold),
        Print(fit_line(&title, width)),
        SetAttribute(Attribute::Reset),
        MoveTo(0, 1),
        Print(horizontal_rule(width)),
    )?;
    Ok(())
}

/// Draws the transcript of messages, wrapping text as needed and showing only the most recent messages that fit
/// in the available space
fn draw_transcript(
    stdout: &mut io::Stdout,
    state: &mut AppState,
    width: u16,
    transcript_height: usize,
) -> Result<()> {
    let available_width = width.saturating_sub(1) as usize;
    let mut lines: Vec<(String, MessageKind)> = Vec::new();

    for (i, message) in state.messages.iter().enumerate() {
        // In collapsed state, hide the assistant message immediately after the
        // file read summary — it holds the raw file content from the runtime.
        if !state.expanded_file_read {
            if let Some(idx) = state.last_file_read_index {
                if i == idx + 1 && message.role == Role::Assistant {
                    continue;
                }
            }
        }

        let is_expanded_file_content = state.expanded_file_read
            && state.last_file_read_index.map_or(false, |idx| i == idx + 1)
            && message.role == Role::Assistant;
        let prefix = if is_expanded_file_content { "" } else { role_prefix(message) };
        let wrapped = wrap_text(
            &format!("{prefix}{}", message.content),
            available_width.max(8),
        );
        let kind = message.kind;
        for line in wrapped {
            lines.push((line, kind));
        }
        lines.push((String::new(), kind));
    }

    state.max_scroll = lines.len().saturating_sub(transcript_height);
    let offset = state.scroll_offset.min(state.max_scroll);
    let end = lines.len().saturating_sub(offset);
    let start = end.saturating_sub(transcript_height);
    let visible: Vec<(String, MessageKind)> = lines[start..end].to_vec();

    for (idx, (line, kind)) in visible.iter().enumerate() {
        queue!(stdout, MoveTo(0, (idx as u16) + 2))?;
        match kind {
            MessageKind::Dimmed => queue!(stdout, SetAttribute(Attribute::Dim))?,
            MessageKind::Alert => queue!(
                stdout,
                SetAttribute(Attribute::Bold),
                SetForegroundColor(Color::Yellow)
            )?,
            MessageKind::Error => queue!(stdout, SetForegroundColor(Color::Red))?,
            MessageKind::Normal => {}
        }
        queue!(stdout, Print(fit_line(line, width)), SetAttribute(Attribute::Reset))?;
    }

    if offset > 0 && !visible.is_empty() {
        let indicator = format!("↑ {} lines", offset);
        let row = (visible.len() as u16).saturating_sub(1) + 2;
        let col = width.saturating_sub(indicator.chars().count() as u16);
        queue!(stdout, MoveTo(col, row), Print(&indicator))?;
    }

    Ok(())
}

/// Draws the input line, showing a prefix and the portion of the input that fits within the available width
fn draw_input(stdout: &mut io::Stdout, state: &AppState, width: u16, height: u16) -> Result<()> {
    let row = height.saturating_sub(2);
    let prefix = "> ";
    let available_width = width.saturating_sub(prefix.len() as u16) as usize;
    let visible_input = visible_input_slice(&state.input, state.cursor, available_width.max(1));

    queue!(
        stdout,
        MoveTo(0, row.saturating_sub(1)),
        Print(horizontal_rule(width)),
        MoveTo(0, row),
        SetAttribute(Attribute::Bold),
        Print(prefix),
        SetAttribute(Attribute::Reset),
        Print(fit_line(
            &visible_input,
            width.saturating_sub(prefix.len() as u16)
        )),
    )?;

    Ok(())
}

/// Draws the status bar at the bottom of the TUI, showing the current status if activity is enabled
fn draw_status(stdout: &mut io::Stdout, state: &AppState, width: u16, height: u16) -> Result<()> {
    let row = height.saturating_sub(1);
    let text = if state.show_activity {
        format!("  {}  ", state.status)
    } else {
        " ".to_string()
    };

    queue!(stdout, MoveTo(0, row), Print(fit_line(&text, width)))?;
    Ok(())
}

/// Helper functions for rendering, including role prefixes, horizontal rules, text wrapping, and calculating the input cursor position
fn role_prefix(message: &ChatMessage) -> &'static str {
    match message.role {
        Role::System => "system: ",
        Role::User => "you: ",
        Role::Assistant => "assistant: ",
    }
}

/// Generates a horizontal rule string of the specified width using box-drawing characters
fn horizontal_rule(width: u16) -> String {
    "─".repeat(width as usize)
}

/// Truncates a string to fit within the specified width, ensuring it does not exceed the available space
fn fit_line(text: &str, width: u16) -> String {
    text.chars().take(width as usize).collect()
}

/// Wraps text to fit within the specified width, breaking at newlines and ensuring lines do not exceed the width
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

/// Calculates the visible portion of the input string based on the cursor position and available width, ensuring the cursor is always visible
fn visible_input_slice(input: &str, cursor: usize, width: usize) -> String {
    let chars = input.chars().collect::<Vec<_>>();
    if chars.len() <= width {
        return input.to_string();
    }

    let cursor_chars = input[..cursor].chars().count();
    let start = cursor_chars.saturating_sub(width.saturating_sub(1));
    chars[start..(start + width).min(chars.len())]
        .iter()
        .collect::<String>()
}

/// Calculates the x position of the input cursor based on the current input, cursor position, and available width, ensuring it stays within the visible portion of the input
fn input_cursor_x(state: &AppState, width: u16) -> u16 {
    let prefix = 2usize;
    let available_width = width.saturating_sub(prefix as u16) as usize;
    let visible_input = visible_input_slice(&state.input, state.cursor, available_width.max(1));
    let visible_chars = visible_input.chars().count();
    let cursor_chars = state.input[..state.cursor].chars().count();
    let start = cursor_chars.saturating_sub(available_width.saturating_sub(1));
    let relative = cursor_chars.saturating_sub(start).min(visible_chars);
    (prefix + relative) as u16
}
