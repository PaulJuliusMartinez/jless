use std::num::NonZeroUsize;

use crate::search::JumpDirection;

#[derive(Debug, Copy, Clone)]
pub enum MovementMethod {
    MoveCursor,
    ScrollViewport,
}

#[derive(Debug, Copy, Clone)]
pub enum Action {
    // Does nothing, for debugging, shouldn't modify any state.
    #[allow(dead_code)]
    NoOp,

    MoveCursorDown(usize),
    MoveCursorUp(usize),
    ExpandOrMoveCursorRightOrDown,
    CollapseOrMoveCursorLeftOrUp,
    MoveCursorLeftOrUpWithoutCollapsing,

    MoveCursorToFirstSibling,
    MoveCursorToLastSibling,
    MoveCursorToNextSiblingOrDown(usize),
    MoveCursorToPrevSiblingOrUp(usize),

    MoveCursorToNextIndentationChange(usize),
    MoveCursorToPrevIndentationChange(usize),

    CollapseNodeAndSiblings(Option<usize>),
    ExpandNodeAndSiblings(Option<usize>),

    ScrollViewportDown(usize),
    ScrollViewportUp(usize),
    PageDown(usize),
    PageUp(usize),

    // Move the viewport by half the height of the screen, and update the focused node
    // so that the focus remains in the same spot on the screen.
    //
    // When a count is provided, move the viewport by that many _lines_, as opposed to
    // N * half-screen size increments. This count is stored and used for subsequent
    // jumps. It resets to half the screen size when the viewport height changes.
    //
    // vim always moves both the viewing window and the focused line by the appropriate
    // lines, so both the contents of the viewport and the physical location of the
    // focused node on the screen will move at the same time when jumping past the end
    // of the file (or before the start).
    //
    // We'll implement a slight variation on this behavior to make sure only one of the
    // contents of viewport or the location of the focused node changes at once. If the
    // viewing window moves, we'll keep the focused line in the same vertical location,
    // but once we're at the top of the file, and the viewing window doesn't change at
    // all, then we will change the focused node by the expected count.
    JumpDown(Option<NonZeroUsize>),
    JumpUp(Option<NonZeroUsize>),

    MoveToSearchMatch(MovementMethod, JumpDirection, usize),
    MoveToLineNumber(NonZeroUsize),

    FocusTop,
    FocusBottom,
    MoveFocusedElemToCenter,
    MoveFocusedElemToTop,
    MoveFocusedElemToBottom,
}

impl Action {
    pub fn is_intentionally_moving_cursor(&self) -> bool {
        match self {
            Action::MoveCursorDown(_)
            | Action::MoveCursorUp(_)
            | Action::ExpandOrMoveCursorRightOrDown
            | Action::CollapseOrMoveCursorLeftOrUp
            | Action::MoveCursorLeftOrUpWithoutCollapsing
            | Action::MoveCursorToFirstSibling
            | Action::MoveCursorToLastSibling
            | Action::MoveCursorToNextSiblingOrDown(_)
            | Action::MoveCursorToPrevSiblingOrUp(_)
            | Action::MoveCursorToNextIndentationChange(_)
            | Action::MoveCursorToPrevIndentationChange(_)
            | Action::MoveToSearchMatch(_, _, _)
            | Action::FocusTop
            | Action::FocusBottom
            | Action::MoveToLineNumber(_) => true,
            Action::NoOp
            | Action::CollapseNodeAndSiblings(_)
            | Action::ExpandNodeAndSiblings(_)
            | Action::ScrollViewportDown(_)
            | Action::ScrollViewportUp(_)
            | Action::PageDown(_)
            | Action::PageUp(_)
            | Action::JumpDown(_)
            | Action::JumpUp(_)
            | Action::MoveFocusedElemToTop
            | Action::MoveFocusedElemToCenter
            | Action::MoveFocusedElemToBottom => false,
        }
    }

    pub fn is_moving_to_adjacent_sibling(&self) -> bool {
        match self {
            Action::MoveCursorToNextSiblingOrDown(_) | Action::MoveCursorToPrevSiblingOrUp(_) => {
                true
            }
            _ => false,
        }
    }
}
