use std::io;

use crossterm::cursor::SetCursorStyle;

use super::state::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum CursorShape {
    SteadyBar,
    SteadyBlock,
    SteadyUnderScore,
    BlinkingBlock,
}

impl CursorShape {
    pub(super) fn to_crossterm(self) -> SetCursorStyle {
        match self {
            CursorShape::SteadyBar => SetCursorStyle::SteadyBar,
            CursorShape::SteadyBlock => SetCursorStyle::SteadyBlock,
            CursorShape::SteadyUnderScore => SetCursorStyle::SteadyUnderScore,
            CursorShape::BlinkingBlock => SetCursorStyle::BlinkingBlock,
        }
    }
}

pub(super) fn sync_terminal_affordances(
    state: &AppState,
    last_shape: &mut Option<CursorShape>,
    out: &mut io::Stdout,
) -> io::Result<()> {
    let shape = if state.pending_approval.is_some() {
        CursorShape::BlinkingBlock
    } else if state.is_reverse_search_active() {
        CursorShape::SteadyUnderScore
    } else if state.is_busy {
        CursorShape::SteadyBlock
    } else {
        CursorShape::SteadyBar
    };
    if *last_shape != Some(shape) {
        crossterm::queue!(out, shape.to_crossterm())?;
        *last_shape = Some(shape);
    }
    Ok(())
}
