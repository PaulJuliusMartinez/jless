// Misc. notes:
//
// Maybe want:
//    LineRef::is_wrap_continuation(&self) -> bool
// or Document::line_number_for_break(&self, &LineRef) -> Option<usize>
//
// TextDocument has `LineWrapping`; JsonDocument/SexpDocument might `ContainerWrapping`?,
// and a `ContainerWrapping` can have a `LineWrapping` inside it.

pub trait Document {
    // `Ord` implementation for `ScreenLine` may panic if we accidentally compare
    // values before/after a resize.
    type ScreenLine: Clone + Eq + Ord + std::fmt::Debug;
    type Cursor: Clone + Ord + std::fmt::Debug;

    fn new(width: usize) -> Self;
    fn width(&self) -> usize;
    fn resize(&mut self, new_width: usize);

    fn append(&mut self, data: &[u8]);
    fn eof(&mut self);

    // Someday: This initialization is clumsy, but we need to know how
    // many lines there are before we know how much space we'll have...
    fn top_screen_line_and_cursor(&self) -> Option<(Self::ScreenLine, Self::Cursor)>;
    fn bottom_screen_line_and_cursor(&self) -> Option<(Self::ScreenLine, Self::Cursor)>;

    fn next_screen_line(&self, screen_line: &Self::ScreenLine) -> Option<Self::ScreenLine>;

    fn prev_screen_line(&self, screen_line: &Self::ScreenLine) -> Option<Self::ScreenLine>;

    // 1-indexed
    fn line_number(&self, screen_line: &Self::ScreenLine) -> usize;

    // Someday: Why aren't these methods on a ScreenLine trait? Do they need to take
    // in a Document?
    fn is_wrapped_line(&self, screen_line: &Self::ScreenLine) -> bool;
    fn is_start_of_wrapped_line(&self, screen_line: &Self::ScreenLine) -> bool;
    fn is_end_of_wrapped_line(&self, screen_line: &Self::ScreenLine) -> bool;
    fn is_after_start_of_wrapped_line(&self, screen_line: &Self::ScreenLine) -> bool;
    fn is_before_end_of_wrapped_line(&self, screen_line: &Self::ScreenLine) -> bool;

    fn is_first_screen_line_of_document(&self, screen_line: &Self::ScreenLine) -> bool {
        self.line_number(screen_line) == 1
            && (!self.is_wrapped_line(screen_line) || self.is_start_of_wrapped_line(screen_line))
    }

    // Someday: Should this return a NonZeroUsize?
    fn cursor_range(&self, cursor: &Self::Cursor) -> CursorRange<Self::ScreenLine>;

    fn does_screen_line_intersect_cursor(
        &self,
        screen_line: &Self::ScreenLine,
        cursor: &Self::Cursor,
    ) -> bool {
        let CursorRange { start, end, .. } = self.cursor_range(cursor);
        start <= *screen_line && *screen_line <= end
    }

    fn cursor_layout_details(
        &self,
        cursor: &Self::Cursor,
        bound: usize,
    ) -> CursorLayoutDetails<Self::ScreenLine> {
        let range = self.cursor_range(cursor);

        let mut screen_lines_before = 0;
        let mut prev_screen_line = range.start.clone();
        while screen_lines_before < bound {
            let Some(screen_line) = self.prev_screen_line(&prev_screen_line) else {
                break;
            };
            screen_lines_before += 1;
            prev_screen_line = screen_line;
        }
        let bounded_doc_screen_lines_before_start = if screen_lines_before <= bound {
            Some(screen_lines_before)
        } else {
            None
        };

        let mut screen_lines_after = 0;
        let mut next_screen_line = range.end.clone();
        while screen_lines_after < bound {
            let Some(screen_line) = self.next_screen_line(&next_screen_line) else {
                break;
            };
            screen_lines_after += 1;
            next_screen_line = screen_line;
        }
        let bounded_doc_screen_lines_after_end = if screen_lines_after <= bound {
            Some(screen_lines_after)
        } else {
            None
        };

        CursorLayoutDetails {
            range,
            bounded_doc_screen_lines_before_start,
            bounded_doc_screen_lines_after_end,
        }
    }

    // If a `Document` supports multiple focused nodes within a single `ScreenLine`, then it
    // should return a new cursor with similar horizontal positioning as `prev_cursor`.
    fn convert_screen_line_to_cursor(
        &self,
        screen_line: Self::ScreenLine,
        prev_cursor: &Self::Cursor,
    ) -> Self::Cursor;

    fn diff_screen_lines(&self, a: &Self::ScreenLine, b: &Self::ScreenLine) -> usize {
        debug_assert!(a >= b);
        let mut diff = 0;
        let mut t = a.clone();
        while t != *b {
            t = self
                .prev_screen_line(&t)
                .expect("a <= b, but never found b after a");
            diff += 1;
        }
        diff
    }

    // Actions

    fn move_cursor_down(&mut self, lines: usize, cursor: &Self::Cursor) -> Option<Self::Cursor>;
    fn move_cursor_up(&mut self, lines: usize, cursor: &Self::Cursor) -> Option<Self::Cursor>;
    fn expand_or_move_cursor_right_or_down(
        &mut self,
        cursor: &Self::Cursor,
    ) -> Option<Self::Cursor>;
    fn collapse_or_move_cursor_left_or_up(&mut self, cursor: &Self::Cursor)
        -> Option<Self::Cursor>;

    // Soon: Uncomment this.
    // #[cfg(test)]
    fn debug_text_content(&self, screen_line: &Self::ScreenLine, cursor: &Self::Cursor) -> Vec<u8>;

    fn raw_contents_for_searching(&self) -> &[u8];
}

/// Representation of a `Cursor` in "Screen" space, as a start and end `ScreenLine`, along with how
/// many `ScreenLine`s the `Cursor` takes up. In the vast majority of cases, when no wrapping is
/// necessary, `start` will equal `end`, and `num_screen_lines` will be 1.
pub struct CursorRange<SL> {
    pub start: SL,
    pub end: SL,
    pub num_screen_lines: usize,
}

/// Computed details used to help reposition the viewport on a specific node pointed to by a
/// `Cursor`. In addition to containing the actual range of the `Cursor`, it also checks to see if
/// the focused node is within a certain distance of the start or end of the document. If one
/// of those values is `None`, that means the start/end of the document is _more_ than some
/// fixed number of screen lines (usually the height of the viewport) before/after the cursor range.
pub struct CursorLayoutDetails<SL> {
    pub range: CursorRange<SL>,
    pub bounded_doc_screen_lines_before_start: Option<usize>,
    pub bounded_doc_screen_lines_after_end: Option<usize>,
}
