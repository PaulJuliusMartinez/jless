use std::ops::Range;

use crate::rendering::PreHighlightingStyledSegment;
use crate::search::InvertedPairedDelimeters;

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

    // Returns the range of a cursor in "screen" space, as a start and end `ScreenLine`, along with how
    // many `ScreenLine`s the `Cursor` takes up. In the vast majority of cases, when no wrapping is
    // necessary, `start` will equal `end`, and `num_screen_lines` will be 1.
    fn cursor_range(&self, cursor: &Self::Cursor) -> ContentRange<Self::ScreenLine>;

    fn does_screen_line_intersect_cursor(
        &self,
        screen_line: &Self::ScreenLine,
        cursor: &Self::Cursor,
    ) -> bool {
        let ContentRange { start, end, .. } = self.cursor_range(cursor);
        start <= *screen_line && *screen_line <= end
    }

    fn center_of_content_range(
        &self,
        content_range: &ContentRange<Self::ScreenLine>,
    ) -> Self::ScreenLine {
        let diff = (content_range.num_screen_lines - 1) / 2;
        let mut screen_line = content_range.start.clone();
        while diff > 0 {
            screen_line = self
                .next_screen_line(&screen_line)
                .expect("must be `ScreenLine`s between start and end of `ContentRange`");
        }
        screen_line
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
                .expect("a >= b, but never found b before a");
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
    fn move_cursor_left_or_up_without_collapsing(
        &mut self,
        cursor: &Self::Cursor,
    ) -> Option<Self::Cursor>;

    fn move_cursor_to_first_sibling(&mut self, cursor: &Self::Cursor) -> Option<Self::Cursor>;
    fn move_cursor_to_last_sibling(&mut self, cursor: &Self::Cursor) -> Option<Self::Cursor>;

    fn collapse_node_and_siblings(
        &mut self,
        cursor: &Self::Cursor,
        depth: Option<usize>,
    ) -> Option<Self::Cursor>;

    fn expand_node_and_siblings(
        &mut self,
        cursor: &Self::Cursor,
        depth: Option<usize>,
    ) -> Option<Self::Cursor>;

    // Search

    fn inverted_paired_delimiters_for_search_input() -> InvertedPairedDelimeters {
        InvertedPairedDelimeters {
            square_brackets: false,
            curly_braces: false,
            parentheses: false,
        }
    }

    fn raw_bytes_for_searching(&self) -> &[u8];
    fn raw_byte_range_of_cursor(&self, cursor: &Self::Cursor) -> Range<usize>;

    /// This should return the cursor that is closest to the given index. It does not
    /// have to be visible.
    fn raw_byte_index_to_cursor(&self, index: usize) -> Self::Cursor;

    /// This should return the screen line that contains the given index (possibly as
    /// part of a collapsed node in that screen line though).
    fn raw_byte_index_to_visible_screen_line(&self, index: usize) -> Self::ScreenLine;

    fn is_raw_byte_range_visible(&self, range: Range<usize>) -> bool;

    fn raw_byte_range_to_visible_content_range(
        &self,
        range: Range<usize>,
    ) -> ContentRange<Self::ScreenLine> {
        // We normally want the last byte of the range (i.e. `end - 1`), but we can have empty
        // ranges, so in that case we'll use the start for both. We can also have an empty
        // range at the start of the doc, hence the saturating sub.
        let end_index = usize::max(range.start, range.end.saturating_sub(1));

        let start = self.raw_byte_index_to_visible_screen_line(range.start);
        let end = self.raw_byte_index_to_visible_screen_line(end_index);
        let num_screen_lines = self.diff_screen_lines(&end, &start) + 1;

        ContentRange {
            start,
            end,
            num_screen_lines,
        }
    }

    /// If the given cursor is hidden because it is collapsed, returns the first
    /// visible cursor before it. If the given cursor is visible, it just returns it.
    fn closest_visible_cursor(&self, cursor: &Self::Cursor) -> Self::Cursor;

    // Rendering

    // Soon: Uncomment this.
    // #[cfg(test)]
    fn debug_text_content(&self, screen_line: &Self::ScreenLine, cursor: &Self::Cursor) -> Vec<u8>;

    fn render_screen_line(
        &self,
        screen_line: &Self::ScreenLine,
        cursor: &Self::Cursor,
    ) -> Option<Vec<PreHighlightingStyledSegment>>;
}

pub struct ContentRange<SL> {
    pub start: SL,
    pub end: SL,
    pub num_screen_lines: usize,
}
