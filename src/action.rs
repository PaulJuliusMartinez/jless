use std::num::NonZeroUsize;

#[derive(Debug, Copy, Clone)]
pub enum Action {
    // Does nothing, for debugging, shouldn't modify any state.
    #[allow(dead_code)]
    NoOp,

    MoveCursorDown(usize),
    MoveCursorUp(usize),
    ExpandOrMoveCursorRightOrDown,
    CollapseOrMoveCursorLeftOrUp,

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

    FocusTop,
    FocusBottom,
    MoveFocusedElemToCenter,
    MoveFocusedElemToTop,
    MoveFocusedElemToBottom,
}
