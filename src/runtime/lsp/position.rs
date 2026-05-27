use std::collections::HashSet;

use crate::core::error::{AppError, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HoverPosition {
    pub line: usize,
    pub column: usize,
}

pub(super) fn build_hover_positions(
    source: &str,
    line: usize,
    column: usize,
) -> Result<Vec<HoverPosition>> {
    let lines: Vec<&str> = source.lines().collect();
    if line == 0 || line > lines.len() {
        return Err(AppError::Tool(format!(
            "line {} out of range ({} lines)",
            line,
            lines.len()
        )));
    }

    let text = lines[line - 1];
    let char_count = text.chars().count();
    let requested = column.min(char_count.saturating_add(1)).max(1);
    let mut positions = Vec::new();
    let mut seen = HashSet::new();

    push_hover_position(&mut positions, &mut seen, line, requested);

    if let Some((start, end)) = identifier_span_near(text, requested) {
        let preferred = [start + 1, start + 2, ((start + end) / 2) + 1, end];
        for candidate in preferred {
            push_hover_position(&mut positions, &mut seen, line, candidate);
        }
    }

    for candidate in [requested.saturating_sub(1), requested + 1] {
        if candidate >= 1 && candidate <= char_count.saturating_add(1) {
            push_hover_position(&mut positions, &mut seen, line, candidate);
        }
    }

    Ok(positions)
}

fn push_hover_position(
    positions: &mut Vec<HoverPosition>,
    seen: &mut HashSet<(usize, usize)>,
    line: usize,
    column: usize,
) {
    if seen.insert((line, column)) {
        positions.push(HoverPosition { line, column });
    }
}

fn identifier_span_near(text: &str, requested_column: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = text.chars().collect();
    if chars.is_empty() {
        return None;
    }

    let nearest = nearest_identifier_index(&chars, requested_column.saturating_sub(1))?;
    let mut start = nearest;
    while start > 0 && is_identifier_char(chars[start - 1]) {
        start -= 1;
    }

    let mut end = nearest + 1;
    while end < chars.len() && is_identifier_char(chars[end]) {
        end += 1;
    }

    Some((start, end))
}

fn nearest_identifier_index(chars: &[char], requested_index: usize) -> Option<usize> {
    if chars.is_empty() {
        return None;
    }

    let max_index = chars.len().saturating_sub(1);
    let clamped = requested_index.min(max_index);
    if is_identifier_char(chars[clamped]) {
        return Some(clamped);
    }

    for distance in 1..=chars.len() {
        let left = clamped.checked_sub(distance);
        if let Some(index) = left {
            if is_identifier_char(chars[index]) {
                return Some(index);
            }
        }

        let right = clamped + distance;
        if right < chars.len() && is_identifier_char(chars[right]) {
            return Some(right);
        }
    }

    None
}

fn is_identifier_char(ch: char) -> bool {
    ch == '_' || ch.is_alphanumeric()
}

pub(super) fn line_column_to_utf16(source: &str, line: usize, column: usize) -> Result<usize> {
    let lines: Vec<&str> = source.lines().collect();
    if line == 0 || line > lines.len() {
        return Err(AppError::Tool(format!(
            "line {} out of range ({} lines)",
            line,
            lines.len()
        )));
    }

    let text = lines[line - 1];
    let char_count = text.chars().count();
    let clamped = column.min(char_count.saturating_add(1)).max(1);
    let utf16 = text
        .chars()
        .take(clamped.saturating_sub(1))
        .map(char::len_utf16)
        .sum();
    Ok(utf16)
}
