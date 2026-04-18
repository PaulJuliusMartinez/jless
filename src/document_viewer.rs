use std::num::NonZeroUsize;
use std::ops::{Range, RangeInclusive};
use std::rc::Rc;

use crate::action::{Action, MovementMethod};
use crate::dimensions::Dimensions;
use crate::document::{ContentRange, Document};
use crate::rendering::{Attrs, StyledSegment, Text};
use crate::search::{JumpDirection, SearchDirection, SearchState};

/// The `DocumentViewer` manages what part of a document is displayed on screen
/// as the user takes actions to move the cursor or manipulate the document. Much
/// of the behavior here matches or is inspired by vim behavior. A type that
/// implements `Document` is responsible for deciding the actual content that goes
/// on each line, and when and where line wrapping should occur.
///
/// The basic objective here is to keep whatever part of the document is focused
/// (i.e., where the `Cursor` is) visible within the viewport, and for certain
/// scrolling actions that manipulate the viewport, the cursor should be updated
/// to a position in the document that is within the viewport.

pub struct DocumentViewer<D: Document> {
    pub doc: D,
    top_line: D::ScreenLine,
    // Soon: Make this private again.
    pub current_focus: D::Cursor,
    dimensions: Dimensions,

    // We call this scrolloff_setting, to differentiate between
    // what it's set to, and what the scrolloff functionally is
    // if it's set to value >= height / 2.
    //
    // Access the functional value via .effective_scrolloff().
    scrolloff_setting: usize,

    tailing_end_of_document: bool,
    jump_distance: Option<NonZeroUsize>,

    search_state: Option<SearchState>,
}

/// Computed details about how close some content is to the start or end of the document. If one of
/// these values is `None`, that means the start/end of the document is _more_ than some fixed
/// number of screen lines (usually the height of the viewport) before/after the content range.
struct BoundedProximityToDocEnds {
    screen_lines_to_start_of_doc: Option<usize>,
    screen_lines_to_end_of_doc: Option<usize>,
}

#[derive(Debug)]
struct AcceptableStartScreenIndexesToShowEntireContentRange {
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    content_height: usize,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    last_screen_index: usize,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    range_after_considering_scrolloff: RangeInclusive<usize>,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    range_after_relaxing_scrolloff_due_to_proximity_of_doc_ends: RangeInclusive<usize>,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    range_after_expanding_due_to_content_height: RangeInclusive<usize>,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    start_indexes_without_clamping_at_lines_to_start_of_doc: RangeInclusive<usize>,
    // The actual fields
    start: usize,
    end: usize,
}

#[derive(Debug, Copy, Clone)]
enum PositionOfScreenLine {
    AboveTopLine,
    AtScreenIndex(usize),
    BelowBottomLine,
}

#[derive(Debug, Copy, Clone)]
enum PositionOfContentInViewport {
    EntirelyInViewport {
        start_index: usize,
        end_index: usize,
    },
    StartsAboveViewport {
        end_index: usize,
    },
    EndsBelowViewport {
        start_index: usize,
    },
    StartsAndEndsOutsideViewport,
}

#[derive(Debug, Copy, Clone)]
enum RelativePosition {
    Before,
    After,
}

impl<D: Document> DocumentViewer<D> {
    pub fn new(
        doc: D,
        first_line: D::ScreenLine,
        initial_cursor: D::Cursor,
        dimensions: Dimensions,
        scrolloff: usize,
    ) -> Self {
        DocumentViewer {
            doc,
            top_line: first_line,
            current_focus: initial_cursor,
            dimensions,
            scrolloff_setting: scrolloff,
            tailing_end_of_document: false,
            jump_distance: None,
            search_state: None,
        }
    }

    pub fn set_scrolloff(&mut self, scrolloff: usize) {
        self.scrolloff_setting = scrolloff;
    }

    // Cap scrolloff at half the size of the screen.
    //
    // Height | Scrolloff | Min between edge of screen and cursor
    //   15   |     3     |                  3
    //   15   |     7     |                  7
    //   15   |     8     |                  7
    //   16   |     7     |                  7
    //   16   |     8     |                  7
    //   17   |     8     |                  8
    fn effective_scrolloff(&self) -> usize {
        usize::min(self.scrolloff_setting, (self.dimensions.height - 1) / 2)
    }

    // If the last line of the file appears before `screen_index`, this will return `None`.
    fn screen_line_at_screen_index(&self, mut screen_index: usize) -> Option<D::ScreenLine> {
        let mut curr_line = self.top_line.clone();
        while screen_index > 0 {
            curr_line = self.doc.next_screen_line(&curr_line)?;
            screen_index -= 1;
        }
        Some(curr_line)
    }

    // Moves the focus to the node that appears at the given screen index.
    fn move_focus_to_screen_index_or_eof(&mut self, screen_index: usize) {
        self.current_focus = match self.screen_line_at_screen_index(screen_index) {
            None => self
                .doc
                .bottom_screen_line_and_cursor()
                .expect(
                    "bottom_screen_line_and_cursor to be Some if we've created a DocumentViewer",
                )
                .1,
            Some(screen_line_at_index) => self
                .doc
                .convert_screen_line_to_cursor(screen_line_at_index, &self.current_focus),
        }
    }

    // If the last line of the file appears before `screen_index`, this will return the
    // last line of the file.
    fn last_screen_line_at_or_before_screen_index(&self, mut screen_index: usize) -> D::ScreenLine {
        let mut curr_line = self.top_line.clone();
        while screen_index > 0 {
            let Some(next_screen_line) = self.doc.next_screen_line(&curr_line) else {
                return curr_line;
            };
            curr_line = next_screen_line;
            screen_index -= 1;
        }
        curr_line
    }

    fn position_of_screen_line(&self, screen_line: &D::ScreenLine) -> PositionOfScreenLine {
        if screen_line < &self.top_line {
            return PositionOfScreenLine::AboveTopLine;
        }

        let mut screen_index = 0;
        let mut curr_screen_line = self.top_line.clone();
        while screen_index < self.dimensions.height {
            if *screen_line == curr_screen_line {
                return PositionOfScreenLine::AtScreenIndex(screen_index);
            }

            screen_index += 1;
            curr_screen_line = self
                .doc
                .next_screen_line(&curr_screen_line)
                .expect("`screen_line` should exist after top line and before EOF");
        }

        PositionOfScreenLine::BelowBottomLine
    }

    fn position_of_content_in_viewport(
        &self,
        content_range: &ContentRange<D::ScreenLine>,
    ) -> PositionOfContentInViewport {
        let start_position = self.position_of_screen_line(&content_range.start);
        let end_position = self.position_of_screen_line(&content_range.end);

        match (start_position, end_position) {
            (PositionOfScreenLine::AboveTopLine, PositionOfScreenLine::AboveTopLine) => {
                panic!("entirety of cursor is above the viewport");
            }
            (
                PositionOfScreenLine::AboveTopLine,
                PositionOfScreenLine::AtScreenIndex(end_index),
            ) => PositionOfContentInViewport::StartsAboveViewport { end_index },
            (PositionOfScreenLine::AboveTopLine, PositionOfScreenLine::BelowBottomLine) => {
                PositionOfContentInViewport::StartsAndEndsOutsideViewport
            }
            (PositionOfScreenLine::AtScreenIndex(_), PositionOfScreenLine::AboveTopLine) => {
                panic!("start of cursor is in viewport, but bottom is above the top line");
            }
            (
                PositionOfScreenLine::AtScreenIndex(start_index),
                PositionOfScreenLine::AtScreenIndex(end_index),
            ) => PositionOfContentInViewport::EntirelyInViewport {
                start_index,
                end_index,
            },
            (
                PositionOfScreenLine::AtScreenIndex(start_index),
                PositionOfScreenLine::BelowBottomLine,
            ) => PositionOfContentInViewport::EndsBelowViewport { start_index },
            (PositionOfScreenLine::BelowBottomLine, PositionOfScreenLine::AboveTopLine) => {
                panic!("end of cursor is below viewport, but start is above viewport");
            }
            (PositionOfScreenLine::BelowBottomLine, PositionOfScreenLine::AtScreenIndex(_)) => {
                panic!("end of cursor is below viewport, but start is in viewport");
            }
            (PositionOfScreenLine::BelowBottomLine, PositionOfScreenLine::BelowBottomLine) => {
                panic!("entirety of cursor is below the viewport");
            }
        }
    }

    fn position_of_current_focus_in_viewport(&self) -> PositionOfContentInViewport {
        self.position_of_content_in_viewport(&self.doc.cursor_range(&self.current_focus))
    }

    fn move_cursor_down(&mut self, lines: usize) {
        if let Some(new_cursor) = self.doc.move_cursor_down(lines, &self.current_focus) {
            self.current_focus = new_cursor;
            self.update_so_current_focus_is_visible();
        }
    }

    fn move_cursor_up(&mut self, lines: usize) {
        if let Some(new_cursor) = self.doc.move_cursor_up(lines, &self.current_focus) {
            self.current_focus = new_cursor;
            self.update_so_current_focus_is_visible();
        }
    }

    // When we collapse or expand nodes, all previous ScreenLines could be invalid, so we need
    // to completely recompute where the top of the screen. This function takes in some
    // modification callback, and assumes that collapsing occurred if and only if the callback
    // returns the exact same cursor. If it got the same cursor back, it'll try to keep the
    // start of it in the same spot on relative to the top of the screen, but if the cursor
    // changed, we'll just make sure it stays in the viewport.
    fn keep_start_of_current_focus_in_same_spot_if_focus_doesnt_move_else<F>(&mut self, mut f: F)
    where
        F: FnMut(&mut Self) -> Option<D::Cursor>,
    {
        let old_cursor_range = &self.doc.cursor_range(&self.current_focus);
        let old_cursor_position = self.position_of_content_in_viewport(&old_cursor_range);

        // We always try to keep the start of the cursor in the same position relative to
        // the top of the screen.
        let (n, relative_position) = match old_cursor_position {
            PositionOfContentInViewport::EntirelyInViewport { start_index, .. }
            | PositionOfContentInViewport::EndsBelowViewport { start_index } => {
                // Most common case; keep the start of the cursor in the same spot as before.
                (start_index, RelativePosition::Before)
            }
            PositionOfContentInViewport::StartsAboveViewport { .. }
            | PositionOfContentInViewport::StartsAndEndsOutsideViewport => {
                let n = self
                    .doc
                    .diff_screen_lines(&self.top_line, &old_cursor_range.start);
                (n, RelativePosition::After)
            }
        };

        let new_cursor = f(self);

        let Some(new_cursor) = new_cursor else {
            return;
        };

        // If the cursor changed, just make sure it's visible and we're done.
        if new_cursor != self.current_focus {
            self.current_focus = new_cursor;
            self.update_so_current_focus_is_visible();
            return;
        }

        // If the cursor didn't change, we'll keep the start of it in the same spot
        // (and then make sure we adhere to scrolloff).
        let new_cursor_range = self.doc.cursor_range(&self.current_focus);

        self.top_line = self.n_screen_lines_relative_to_or_end_of_doc(
            new_cursor_range.start,
            n,
            relative_position,
        );

        // Very unlikely, but we'll add this in. Imagine after collapsing a node and its neighbors:
        //
        // Before:      After
        //  (key (Ve
        // +---------+  +-----------+
        // | eeerrry |  | (…)
        // | y_long_ |  | (…)       |
        // | variant |  | (key (…)) |
        // +---------+  +-----------+
        //
        // And if scrolloff is 1, then "key" should be in the middle of the viewport.
        self.update_so_current_focus_is_visible();
    }

    fn expand_or_move_cursor_right_or_down(&mut self) {
        self.keep_start_of_current_focus_in_same_spot_if_focus_doesnt_move_else(|me| {
            me.doc
                .expand_or_move_cursor_right_or_down(&me.current_focus)
        });
    }

    fn collapse_or_move_cursor_left_or_up(&mut self) {
        self.keep_start_of_current_focus_in_same_spot_if_focus_doesnt_move_else(|me| {
            me.doc.collapse_or_move_cursor_left_or_up(&me.current_focus)
        });
    }

    fn move_cursor_left_or_up_without_collapsing(&mut self) {
        // No collapsing, so all the ScreenLines are still valid.
        if let Some(new_cursor) = self
            .doc
            .move_cursor_left_or_up_without_collapsing(&self.current_focus)
        {
            self.current_focus = new_cursor;
            self.update_so_current_focus_is_visible();
        }
    }

    fn collapse_node_and_siblings(&mut self, depth: Option<usize>) {
        self.keep_start_of_current_focus_in_same_spot_if_focus_doesnt_move_else(|me| {
            me.doc.collapse_node_and_siblings(&me.current_focus, depth)
        });
    }

    fn expand_node_and_siblings(&mut self, depth: Option<usize>) {
        self.keep_start_of_current_focus_in_same_spot_if_focus_doesnt_move_else(|me| {
            me.doc.expand_node_and_siblings(&me.current_focus, depth)
        });
    }

    fn move_cursor_to_first_sibling(&mut self) {
        if let Some(new_cursor) = self.doc.move_cursor_to_first_sibling(&self.current_focus) {
            self.current_focus = new_cursor;
            self.update_so_current_focus_is_visible();
        }
    }

    fn move_cursor_to_last_sibling(&mut self) {
        if let Some(new_cursor) = self.doc.move_cursor_to_last_sibling(&self.current_focus) {
            self.current_focus = new_cursor;
            self.update_so_current_focus_is_visible();
        }
    }

    fn focus_top(&mut self) {
        let (top_screen_line, cursor) = self
            .doc
            .top_screen_line_and_cursor()
            .expect("top_screen_line_and_cursor to be Some if we've created a DocumentViewer");
        self.top_line = top_screen_line;
        self.current_focus = cursor;
    }

    fn focus_bottom(&mut self) {
        let (bottom_screen_line, cursor) = self
            .doc
            .bottom_screen_line_and_cursor()
            .expect("bottom_screen_line_and_cursor to be Some if we've created a DocumentViewer");

        // If we scrolled past the previous bottom of the document, it's possible the new bottom is
        // already on the screen. Using this as the `top_line` will put the end of last node at the
        // bottom of the viewport. If the current top line is before this, then the bottom _isn't_
        // on screen, and we need to update the top line, but otherwise we're fine, and we don't
        // need to update anything.
        let potential_top_line = self
            .n_screen_lines_before_or_top_of_doc(bottom_screen_line, self.dimensions.height - 1);
        if self.top_line < potential_top_line {
            self.top_line = potential_top_line;
        }

        self.current_focus = cursor;
    }

    fn move_focused_elem_to_top(&mut self) {
        let cursor_range = self.doc.cursor_range(&self.current_focus);
        let scrolloff = self.effective_scrolloff();
        // If the focused node is multiple lines, put the start at the top of the screen.
        self.top_line = self.n_screen_lines_before_or_top_of_doc(cursor_range.start, scrolloff);
    }

    fn move_focused_elem_to_center(&mut self) {
        // We want to put the middle of the focused node, at the middle of the screen.
        // There are four cases: even/odd viewport height, even/odd cursor height.
        // In the odd/odd and even/even cases, things fit evenly, but otherwise we need
        // to bias in one direction. We'll opt towards putting it closer to the top of
        // the screen.
        //
        // Viewport Height | Cursor Height | Occupied Cursor Range | (VH - CH) / 2
        //         7       |       3       |    [--xxx--]  (2-4)   | (7 - 3) / 2 = 2
        //         7       |       4       |    [-xxxx--]  (1-4)   | (7 - 4) / 2 = 1
        //         8       |       3       |    [--xxx---] (2-4)   | (8 - 3) / 2 = 2
        //         8       |       4       |    [--xxxx--] (2-5)   | (8 - 4) / 2 = 2
        //
        // Cursor is *larger* than viewport. In this case, we want to show more of the
        // start of the cursor than the end:
        //         4       |       8       |  xx[xxxx]xx  (-2 - 5) | (4 - 8) / 2 = -2
        //         4       |       7       |  x[xxxx]xx   (-1 - 5) | (4 - 7) / 2 = -1.5
        //         3       |       8       |  xx[xxx]xxx  (-2 - 5) | (3 - 8) / 2 = -2.5
        //         3       |       7       |  xx[xxx]xx   (-2 - 4) | (3 - 7) / 2 = -2
        //
        // Conveniently, integer division truncates towards zero, giving us a neat formula here.

        let viewport_height = self.dimensions.height as isize;
        let cursor_range = self.doc.cursor_range(&self.current_focus);
        let cursor_height = cursor_range.num_screen_lines as isize;

        let offset = (viewport_height - cursor_height) / 2;

        if offset >= 0 {
            let offset = offset as usize;
            self.top_line = self.n_screen_lines_before_or_top_of_doc(cursor_range.start, offset);
        } else {
            let offset = (-offset) as usize;
            self.top_line = self.n_screen_lines_after(cursor_range.start, offset);
        }
    }

    fn move_focused_elem_to_bottom(&mut self) {
        let cursor_range = self.doc.cursor_range(&self.current_focus);
        let scrolloff = self.effective_scrolloff();
        // If `height = 5`, and `scrolloff = 1`, then we want the end to be the fourth line,
        // so we want to go `3 = height - 1 - scrolloff` lines before.
        let offset = (self.dimensions.height - 1) - scrolloff;
        // If the focused node is multiple lines, put the end at the bottom of the screen.
        self.top_line = self.n_screen_lines_before_or_top_of_doc(cursor_range.end, offset);
    }

    fn scroll_viewport_down(&mut self, mut lines: usize) {
        let mut lines_scrolled = 0;
        let mut next_top_line = self.top_line.clone();
        while lines > 0 {
            match self.doc.next_screen_line(&next_top_line) {
                None => break,
                Some(line) => {
                    lines -= 1;
                    lines_scrolled += 1;
                    next_top_line = line;
                }
            }
        }

        if lines_scrolled > 0 {
            self.top_line = next_top_line;
            self.maybe_update_focused_node_after_scroll();
        }
    }

    fn page_down(&mut self, pages: usize) {
        self.scroll_viewport_down(self.dimensions.height * pages);
    }

    fn scroll_viewport_up(&mut self, mut lines: usize) {
        let mut lines_scrolled = 0;
        let mut next_top_line = self.top_line.clone();
        while lines > 0 {
            match self.doc.prev_screen_line(&next_top_line) {
                None => break,
                Some(line) => {
                    lines -= 1;
                    lines_scrolled += 1;
                    next_top_line = line;
                }
            }
        }

        if lines_scrolled > 0 {
            self.top_line = next_top_line;
            self.maybe_update_focused_node_after_scroll();
        }
    }

    fn page_up(&mut self, pages: usize) {
        self.scroll_viewport_up(self.dimensions.height * pages);
    }

    fn jump_down(&mut self, num_screen_lines: Option<NonZeroUsize>) {
        // It maybe feels a little weird to use the start/end index when something
        // is half on the screen, as opposed to something more "principled" like
        // start/2, but the user isn't trying to jump to a specific element, so
        // it doesn't really matter.
        //
        // Using the start/end index might also mean there will be a tiny bit less visual jitter.
        let focus_index = match self.position_of_current_focus_in_viewport() {
            PositionOfContentInViewport::StartsAboveViewport { end_index } => end_index,
            PositionOfContentInViewport::EndsBelowViewport { start_index } => start_index,
            PositionOfContentInViewport::StartsAndEndsOutsideViewport => self.dimensions.height / 2,
            PositionOfContentInViewport::EntirelyInViewport { end_index, .. } => end_index,
        };

        let lines_to_move =
            self.calculate_and_maybe_save_num_screen_lines_to_jump(num_screen_lines);

        let last_screen_line = self.screen_line_at_screen_index(self.dimensions.height - 1);

        // If the last screen line of the doc is visible on screen, we don't move the viewport.
        // The last screen line is visible if there's no screen line at the bottom of the viewport,
        // or if nothing comes after the screen line at the bottom of the viewport.
        let screen_line_at_bottom_of_viewport_thats_not_last_line_in_doc = match last_screen_line {
            None => None,
            Some(last_screen_line) => {
                if self.doc.next_screen_line(&last_screen_line).is_some() {
                    Some(last_screen_line)
                } else {
                    None
                }
            }
        };

        match screen_line_at_bottom_of_viewport_thats_not_last_line_in_doc {
            Some(last_screen_line) => {
                // If this exists, that means we're not at the end of the doc, so we need to
                // update the viewport.

                // Count past the last screen line, but not past the bottom of the doc, then
                // rewind to find the new top line.
                let new_last_screen_line = self
                    .n_screen_lines_after_or_bottom_of_doc(last_screen_line, lines_to_move.get());
                self.top_line = self.n_screen_lines_before_or_top_of_doc(
                    new_last_screen_line,
                    self.dimensions.height - 1,
                );
                // Keep focus in the same place on screen.
                self.move_focus_to_screen_index_or_eof(focus_index);
            }
            None => {
                // If we didn't move the viewport, we'll move the current focus by the desired
                // number of screen lines. Importantly, we need to make sure that we actually
                // make some progress even if the focused node is multipled lines tall.

                // Cap `focus_index` at the last index visible on screen.
                let focus_index = usize::min(
                    focus_index + lines_to_move.get(),
                    self.dimensions.height - 1,
                );
                self.move_focus_to_screen_index_or_eof(focus_index);
            }
        }
    }

    fn jump_up(&mut self, num_screen_lines: Option<NonZeroUsize>) {
        // Note that we pick the `start_index` in the `EntirelyInViewport` case. (See comment
        // below.)
        let focus_index = match self.position_of_current_focus_in_viewport() {
            PositionOfContentInViewport::StartsAboveViewport { end_index } => end_index,
            PositionOfContentInViewport::EndsBelowViewport { start_index } => start_index,
            PositionOfContentInViewport::StartsAndEndsOutsideViewport => self.dimensions.height / 2,
            PositionOfContentInViewport::EntirelyInViewport { start_index, .. } => start_index,
        };

        let lines_to_move = self
            .calculate_and_maybe_save_num_screen_lines_to_jump(num_screen_lines)
            .get();
        let mut lines_moved = 0;
        let mut next_top_line = self.top_line.clone();

        while lines_moved < lines_to_move {
            match self.doc.prev_screen_line(&next_top_line) {
                None => break,
                Some(line) => {
                    lines_moved += 1;
                    next_top_line = line;
                }
            }
        }

        self.top_line = next_top_line;

        if lines_moved == 0 {
            // We're at the top of the screen, move current focus by desired number of screen lines.
            //
            // Importantly, we need to make sure that this actually moves the focus in the case
            // where the focused line is multiple lines tall. If we're already at the top of the
            // file, the only possibilities for the position of the cursor are `EndsBelowViewport`
            // and `EntirelyInViewport`, and in both cases we pick the `start_index` as the focus
            // index, so anything before that will be a different node.
            let focus_index = focus_index.saturating_sub(lines_to_move);
            self.move_focus_to_screen_index_or_eof(focus_index);
        } else {
            // The screen moved, so keep the focus in the same place on screen.
            self.move_focus_to_screen_index_or_eof(focus_index);
        }
    }

    fn calculate_and_maybe_save_num_screen_lines_to_jump(
        &mut self,
        num_screen_lines: Option<NonZeroUsize>,
    ) -> NonZeroUsize {
        // If we have a value, save it for next time, and return it.
        if let Some(n) = num_screen_lines {
            self.jump_distance = Some(n);
            return n;
        }

        // If we don't have a value, use a previous one if we had it.
        if let Some(n) = self.jump_distance {
            return n;
        }

        // Otherwise use half the height of the screen (but at least 1).
        NonZeroUsize::new(usize::max(self.dimensions.height / 2, 1)).unwrap()
    }

    fn update_so_current_focus_is_visible(&mut self) {
        let cursor_range = self.doc.cursor_range(&self.current_focus);
        self.update_so_content_range_is_visible(cursor_range);
    }

    fn update_so_content_range_is_visible(&mut self, content_range: ContentRange<D::ScreenLine>) {
        // This will make the start of the content range visible, but maybe there are some cases,
        // like when moving up, or searching backwards, where we'd want to prioritize showing the
        // end. Just always showing the start is at least consistent though.
        if content_range.num_screen_lines >= self.dimensions.height {
            self.top_line = content_range.start;
            return;
        }

        let acceptable_start_index_range = self
            .calculate_acceptable_start_screen_indexes_to_show_entire_content_range(&content_range);

        let AcceptableStartScreenIndexesToShowEntireContentRange {
            start: start_index,
            end: end_index,
            ..
        } = acceptable_start_index_range;

        let first_acceptable_top_line =
            self.n_screen_lines_before(content_range.start.clone(), end_index);
        let last_acceptable_top_line =
            self.n_screen_lines_before(content_range.start.clone(), start_index);

        if self.top_line < first_acceptable_top_line {
            self.top_line = first_acceptable_top_line;
        } else if last_acceptable_top_line < self.top_line {
            self.top_line = last_acceptable_top_line;
        } else {
            // Top line is already in an ok spot!
        }
    }

    fn calculate_acceptable_start_screen_indexes_to_show_entire_content_range(
        &self,
        content_range: &ContentRange<D::ScreenLine>,
    ) -> AcceptableStartScreenIndexesToShowEntireContentRange {
        debug_assert!(content_range.num_screen_lines < self.dimensions.height);

        // We want to make sure that the entirely of the given content range is on screen,
        // while adhering to scrolloff settings as much as possible, and handling ranges that
        // span multiple lines.
        //
        // The high level logic here is to consider the range of "acceptable" places for the
        // content to be. This is by default the entire screen, but then it gets shrunk based on
        // the `scrolloff` setting, which gets relaxed when we are close to the start or end
        // of the document. Further, if the content is very large, we may be forced to violate
        // scrolloff (though we'll minimize the extent to which we do so if possible).
        //
        // Once we have this range, we can convert this to acceptable start indexes for
        // the content. Finally, we make sure that these start indexes won't force us to
        // put the top of the document below the top line, which isn't allowed.

        let bounded_proximity_to_doc_ends =
            self.bounded_proximity_to_doc_ends(content_range, self.dimensions.height - 1);

        // Initial acceptable range is the whole screen.
        let mut first_acceptable_screen_index = 0;
        let last_screen_index = self.dimensions.height - 1;
        let mut last_acceptable_screen_index = last_screen_index;

        let min_screenlines_between_edge_of_screen_and_cursor = self.effective_scrolloff();

        // Shrink the acceptable range based on the scrolloff setting. (Because we capped
        // scrolloff at half the height of the screen, that ensures this doesn't cause
        // the values to cross.)
        first_acceptable_screen_index += min_screenlines_between_edge_of_screen_and_cursor;
        last_acceptable_screen_index -= min_screenlines_between_edge_of_screen_and_cursor;

        #[cfg(debug_assertions)]
        let range_after_considering_scrolloff =
            first_acceptable_screen_index..=last_acceptable_screen_index;

        // Now we take into account the start/end of the document, where we don't enforce the
        // scrolloff setting. If we enforce scrolloff at the top of the file, then we'd have
        // to show empty lines before the start of the file, which would be silly. At the bottom
        // of the file, we won't enforce scrolloff, so that if you just hold the down arrow,
        // eventually you'll focus the last line of the file on the bottom of the screen.
        // (We do allow scrolling past the end of the file, but that decreases the start index,
        // and this operation relaxes the constraint by increasing the allowable start index,
        // so it is not relevant here.)

        if let Some(lines_to_start) = bounded_proximity_to_doc_ends.screen_lines_to_start_of_doc {
            first_acceptable_screen_index =
                usize::min(first_acceptable_screen_index, lines_to_start);
        }

        if let Some(lines_to_end) = bounded_proximity_to_doc_ends.screen_lines_to_end_of_doc {
            last_acceptable_screen_index = usize::max(
                last_acceptable_screen_index,
                // The bound is approximate, to prevent us from having to look at the whole
                // document, not exact.
                // Someday: Should this be `saturating_sub`?
                last_screen_index - lines_to_end,
            );
        }

        #[cfg(debug_assertions)]
        let range_after_relaxing_scrolloff_due_to_proximity_of_doc_ends =
            first_acceptable_screen_index..=last_acceptable_screen_index;

        // Now we need to expand the acceptable range based on the height of the cursor, up
        // to the whole size of the screen again.

        let height_of_acceptable_range =
            last_acceptable_screen_index - first_acceptable_screen_index + 1;
        let content_height = content_range.num_screen_lines;

        if content_height > height_of_acceptable_range {
            let mut additional_space_needed = content_height - height_of_acceptable_range;
            let space_to_reclaim_at_start = first_acceptable_screen_index;
            let space_to_reclaim_at_end = last_screen_index - last_acceptable_screen_index;

            // If we have more space to reclaim on one end (because of the start/end of the file)
            // we'll push that side back first. Imagine the second line of a file is 8 lines tall
            // (first line is 1 line tall), the height of the window is 10, and scrolloff is 4.
            // Then we have:
            //
            // Screen index range:            [0, 9]
            // Acceptable range w/ scrolloff: [4, 5]
            // After start/end of file:       [1, 5]
            // Height of acceptable range:    5
            //
            // If we subtracted evenly from both sides, we'd expand it to [0, 6], and then [0, 7],
            // which would be weird. If you loaded the file, and hit the down arrow, even though
            // the line is perfectly centered (it takes up [1, 8]), we would move it up to the top
            // of the screen.
            //
            // (I originally just thought "oh, we should try to keep it centered", and then while
            // trying to explain why this was better, I found this example that clearly shows the
            // other behavior is wrong.)
            let diff_between_sides =
                usize::abs_diff(space_to_reclaim_at_start, space_to_reclaim_at_end);
            let space_to_reclaim_from_one_side =
                usize::min(diff_between_sides, additional_space_needed);

            additional_space_needed -= space_to_reclaim_from_one_side;
            if space_to_reclaim_at_start > space_to_reclaim_at_end {
                first_acceptable_screen_index -= space_to_reclaim_from_one_side;
            } else if space_to_reclaim_at_end > space_to_reclaim_at_start {
                last_acceptable_screen_index += space_to_reclaim_from_one_side;
            }

            // Now both sides are equal; let's chop off equally from both sides now.
            // If we have an odd amount of additional space neeed, we'll round up and
            // take that amount from both sides so that we don't always have the bigger
            // half of the cursor on top.

            let to_reclaim = (additional_space_needed + 1) / 2;
            first_acceptable_screen_index -= to_reclaim;
            last_acceptable_screen_index += to_reclaim;
        }

        #[cfg(debug_assertions)]
        let range_after_expanding_due_to_content_height =
            first_acceptable_screen_index..=last_acceptable_screen_index;

        // Final step: convert the acceptable screen index range into an acceptable
        // range for the start of the content.
        let mut first_acceptable_screen_index_for_start_of_content = first_acceptable_screen_index;
        let mut last_acceptable_screen_index_for_start_of_content = usize::max(
            first_acceptable_screen_index_for_start_of_content,
            // Subtract (size - 1); for example, if the content takes up two lines, then
            // the last acceptable start is one line before the last acceptable screen index.
            last_acceptable_screen_index.saturating_sub(content_range.num_screen_lines - 1),
        );

        #[allow(dead_code)]
        #[cfg(debug_assertions)]
        let start_indexes_without_clamping_at_lines_to_start_of_doc =
            first_acceptable_screen_index_for_start_of_content
                ..=last_acceptable_screen_index_for_start_of_content;

        // If the content is close to the start of the doc, we can't put it further down the
        // screen than `screen_lines_to_start_of_doc`, otherwise we'd have to show empty lines
        // above the first line.
        if let Some(lines_to_start) = bounded_proximity_to_doc_ends.screen_lines_to_start_of_doc {
            first_acceptable_screen_index_for_start_of_content = usize::min(
                first_acceptable_screen_index_for_start_of_content,
                lines_to_start,
            );
            last_acceptable_screen_index_for_start_of_content = usize::min(
                last_acceptable_screen_index_for_start_of_content,
                lines_to_start,
            );
        }

        AcceptableStartScreenIndexesToShowEntireContentRange {
            #[cfg(debug_assertions)]
            content_height,
            #[cfg(debug_assertions)]
            last_screen_index,
            #[cfg(debug_assertions)]
            range_after_considering_scrolloff,
            #[cfg(debug_assertions)]
            range_after_relaxing_scrolloff_due_to_proximity_of_doc_ends,
            #[cfg(debug_assertions)]
            range_after_expanding_due_to_content_height,
            #[cfg(debug_assertions)]
            start_indexes_without_clamping_at_lines_to_start_of_doc,
            // The actual fields
            start: first_acceptable_screen_index_for_start_of_content,
            end: last_acceptable_screen_index_for_start_of_content,
        }
    }

    fn bounded_proximity_to_doc_ends(
        &self,
        content_range: &ContentRange<D::ScreenLine>,
        bound: usize,
    ) -> BoundedProximityToDocEnds {
        let mut screen_lines_before = 0;
        let mut prev_screen_line = content_range.start.clone();
        while screen_lines_before < bound {
            let Some(screen_line) = self.doc.prev_screen_line(&prev_screen_line) else {
                break;
            };
            screen_lines_before += 1;
            prev_screen_line = screen_line;
        }
        let screen_lines_to_start_of_doc = if screen_lines_before <= bound {
            Some(screen_lines_before)
        } else {
            None
        };

        let mut screen_lines_after = 0;
        let mut next_screen_line = content_range.end.clone();
        while screen_lines_after < bound {
            let Some(screen_line) = self.doc.next_screen_line(&next_screen_line) else {
                break;
            };
            screen_lines_after += 1;
            next_screen_line = screen_line;
        }
        let screen_lines_to_end_of_doc = if screen_lines_after <= bound {
            Some(screen_lines_after)
        } else {
            None
        };

        BoundedProximityToDocEnds {
            screen_lines_to_start_of_doc,
            screen_lines_to_end_of_doc,
        }
    }

    fn screen_indexes_within_scrolloff(&self) -> RangeInclusive<usize> {
        let min_screenlines_between_edge_of_screen_and_cursor = self.effective_scrolloff();

        // If the top of the screen is the top of the document, we don't enforce scrolloff.
        let first_acceptable_screen_index =
            if self.doc.is_first_screen_line_of_document(&self.top_line) {
                0
            } else {
                min_screenlines_between_edge_of_screen_and_cursor
            };

        let last_screen_index = self.dimensions.height - 1;
        let last_acceptable_screen_index =
            last_screen_index - min_screenlines_between_edge_of_screen_and_cursor;

        first_acceptable_screen_index..=last_acceptable_screen_index
    }

    fn maybe_update_focused_node_after_scroll(&mut self) {
        // When scrolling, we'll allow scrolling wrapped lines partly off the screen as long
        // as any part of the focused node still obeys the scrolloff setting.
        let acceptable_screen_indexes = self.screen_indexes_within_scrolloff();

        // Using `last_screen_line_at_or_before_screen_index` handles the case when the end
        // of the file is on screen. In the extreme example, consider if the last line of the
        // document is at the top of the screen. If the screen is 10 lines tall, and scrolloff
        // is 3, then the first/last acceptable screen indexes will be 3 and 6, but the first
        // and last acceptable screen line will both be the current top line, which is the last
        // line of the document.

        let first_acceptable_screen_line =
            self.last_screen_line_at_or_before_screen_index(*acceptable_screen_indexes.start());
        let last_acceptable_screen_line =
            self.last_screen_line_at_or_before_screen_index(*acceptable_screen_indexes.end());

        let focused_range = self.doc.cursor_range(&self.current_focus);

        if focused_range.end < first_acceptable_screen_line {
            self.current_focus = self
                .doc
                .convert_screen_line_to_cursor(first_acceptable_screen_line, &self.current_focus);
        } else if last_acceptable_screen_line < focused_range.start {
            self.current_focus = self
                .doc
                .convert_screen_line_to_cursor(last_acceptable_screen_line, &self.current_focus);
        } else {
            // Current focused range overlaps with acceptable screen line ranges;
            // nothing to do!
        }
    }

    pub fn resize(&mut self, new_dimensions: Dimensions) {
        // Handle resizes in two parts: first resize the width, then the height.
        self.resize_width(new_dimensions.width);
        self.resize_height(new_dimensions.height);
        self.move_current_focus_within_scrolloff_after_resize();
    }

    fn update_dimensions_and_resize_doc(&mut self, dimensions: Dimensions) {
        self.dimensions = dimensions;
        self.doc.resize(dimensions.width);
    }

    fn resize_width(&mut self, new_width: usize) {
        if new_width == self.dimensions.width {
            return;
        }

        let new_dimensions = Dimensions {
            width: new_width,
            ..self.dimensions
        };

        let old_cursor_range = self.doc.cursor_range(&self.current_focus);

        match self.position_of_content_in_viewport(&old_cursor_range) {
            PositionOfContentInViewport::StartsAboveViewport { end_index } => {
                // Don't top line to keep anchored, so we'll keep the end of the line
                // in the same place.
                self.update_dimensions_and_resize_doc(new_dimensions);
                let new_cursor_range = self.doc.cursor_range(&self.current_focus);
                self.top_line =
                    self.n_screen_lines_before_or_top_of_doc(new_cursor_range.end, end_index);
            }
            PositionOfContentInViewport::StartsAndEndsOutsideViewport => {
                let lines_above_top_of_screen = self
                    .doc
                    .diff_screen_lines(&self.top_line, &old_cursor_range.start);

                self.update_dimensions_and_resize_doc(new_dimensions);
                let new_cursor_range = self.doc.cursor_range(&self.current_focus);

                if old_cursor_range.num_screen_lines == new_cursor_range.num_screen_lines {
                    // If the cursor is the same number of lines long, then we'll keep the lines in
                    // the same spot.
                    self.top_line = self
                        .n_screen_lines_after(new_cursor_range.start, lines_above_top_of_screen);
                } else {
                    // If the size of the cursor changed, we'll try to keep the content
                    // near the center of the screen in approximately the same place.
                    //
                    // For example, if the focused node took up 100 screen lines before,
                    // and lines 70-90 were on screen, that means the 80% percentile of
                    // the node was in the middle of the screen. If, after the resize, it
                    // takes up 60 screen lines, then we want screen line 48 in the middle
                    // of the screen, so the screen will show lines 38-58. This is all
                    // very approximate, so I'm not worried too much about off-by-one errors.
                    let percentile_of_cursor_in_middle_of_screen: f64 = ((lines_above_top_of_screen
                        as f64)
                        + (self.dimensions.height as f64 / 2.0))
                        / (old_cursor_range.num_screen_lines as f64);
                    let new_cursor_index_in_middle_of_screen: usize =
                        (percentile_of_cursor_in_middle_of_screen
                            * (new_cursor_range.num_screen_lines as f64))
                            as usize;

                    let screen_line_in_middle_of_screen = self.n_screen_lines_after(
                        new_cursor_range.start,
                        new_cursor_index_in_middle_of_screen,
                    );
                    self.top_line = self.n_screen_lines_before_or_top_of_doc(
                        screen_line_in_middle_of_screen,
                        self.dimensions.height / 2,
                    );
                }
            }
            PositionOfContentInViewport::EntirelyInViewport { start_index, .. }
            | PositionOfContentInViewport::EndsBelowViewport { start_index } => {
                self.update_dimensions_and_resize_doc(new_dimensions);
                let new_cursor_range = self.doc.cursor_range(&self.current_focus);
                self.top_line =
                    self.n_screen_lines_before_or_top_of_doc(new_cursor_range.start, start_index);
            }
        }
    }

    fn resize_height(&mut self, new_height: usize) {
        let old_height = self.dimensions.height;
        if new_height == old_height {
            return;
        }

        // This gets reset when the size of the viewport changes.
        self.jump_distance = None;

        // We'll go with vim's approach of keeping the focused in the same percentile of the
        // screen.

        let anchor_screen_line;
        let new_index;

        let convert_old_index_to_new_index = |index| -> usize {
            // We do `old_height - 1` so that when we're using the last line as an anchor, it'll
            // stay the last line.
            let percentile = (index as f64) / ((old_height - 1) as f64);
            (percentile * ((new_height - 1) as f64)).round() as usize
        };

        let cursor_range = self.doc.cursor_range(&self.current_focus);

        match self.position_of_content_in_viewport(&cursor_range) {
            PositionOfContentInViewport::StartsAboveViewport { end_index } => {
                // Keep the end of focused node in the same percentile
                anchor_screen_line = cursor_range.end;
                new_index = convert_old_index_to_new_index(end_index);
            }
            PositionOfContentInViewport::StartsAndEndsOutsideViewport => {
                // Keep middle of what's visible on screen in the middle.
                let half_old_height = self.dimensions.height / 2;
                anchor_screen_line = self.screen_line_at_screen_index(half_old_height).unwrap();
                new_index = new_height / 2;
            }
            PositionOfContentInViewport::EntirelyInViewport {
                start_index,
                end_index,
            } => {
                // Keep the middle of focused node in the same percentile.
                let middle_index = (start_index + end_index) / 2;
                anchor_screen_line = self.screen_line_at_screen_index(middle_index).unwrap();
                new_index = convert_old_index_to_new_index(middle_index);
            }
            PositionOfContentInViewport::EndsBelowViewport { start_index } => {
                // Keep the start of focused node in the same percentile.
                anchor_screen_line = cursor_range.start;
                new_index = convert_old_index_to_new_index(start_index);
            }
        }

        self.top_line = self.n_screen_lines_before_or_top_of_doc(anchor_screen_line, new_index);

        self.dimensions = Dimensions {
            height: new_height,
            ..self.dimensions
        };
    }

    fn move_current_focus_within_scrolloff_after_resize(&mut self) {
        // After a resize, we'll allow part of the focused node to be outside of scrolloff,
        // but if that's not the case we'll move the screen slightly to make it so.
        let acceptable_screen_indexes = self.screen_indexes_within_scrolloff();

        // We use `last_screen_line_at_or_before_screen_index` in `maybe_update_focused_node_after_scroll`
        // to allow scrolling the end of the file to the very top of the screen. We'll use the same
        // relaxation here, so that if you do that, and then resize the screen, the cursor won't
        // "jump" into the scrolloff zone.

        let first_acceptable_screen_line =
            self.last_screen_line_at_or_before_screen_index(*acceptable_screen_indexes.start());
        let last_acceptable_screen_line =
            self.last_screen_line_at_or_before_screen_index(*acceptable_screen_indexes.end());

        let focused_range = self.doc.cursor_range(&self.current_focus);

        if focused_range.end < first_acceptable_screen_line {
            // Put the end of the focused range at the first acceptable screen index.
            self.top_line =
                self.n_screen_lines_before(focused_range.end, *acceptable_screen_indexes.start());
        } else if last_acceptable_screen_line < focused_range.start {
            // Put the start of the focused range at the last acceptable screen index.
            self.top_line =
                self.n_screen_lines_before(focused_range.start, *acceptable_screen_indexes.end());
        } else {
            // Current focused range overlaps with acceptable screen line ranges;
            // nothing to do!
        }
    }

    // Assumes that this will always exist.
    fn n_screen_lines_before(&self, mut screen_line: D::ScreenLine, mut n: usize) -> D::ScreenLine {
        while n > 0 {
            screen_line = self.doc.prev_screen_line(&screen_line).unwrap();
            n -= 1;
        }
        screen_line
    }

    fn n_screen_lines_before_or_top_of_doc(
        &self,
        mut screen_line: D::ScreenLine,
        mut n: usize,
    ) -> D::ScreenLine {
        while n > 0 {
            let Some(prev_screen_line) = self.doc.prev_screen_line(&screen_line) else {
                return screen_line;
            };
            screen_line = prev_screen_line;
            n -= 1;
        }
        screen_line
    }

    // Assumes that this will always exist
    fn n_screen_lines_after(&self, mut screen_line: D::ScreenLine, mut n: usize) -> D::ScreenLine {
        while n > 0 {
            screen_line = self.doc.next_screen_line(&screen_line).unwrap();
            n -= 1;
        }
        screen_line
    }

    fn n_screen_lines_after_or_bottom_of_doc(
        &self,
        mut screen_line: D::ScreenLine,
        mut n: usize,
    ) -> D::ScreenLine {
        while n > 0 {
            let Some(next_screen_line) = self.doc.next_screen_line(&screen_line) else {
                return screen_line;
            };
            screen_line = next_screen_line;
            n -= 1;
        }
        screen_line
    }

    fn n_screen_lines_relative_to_or_end_of_doc(
        &self,
        screen_line: D::ScreenLine,
        n: usize,
        relative_position: RelativePosition,
    ) -> D::ScreenLine {
        match relative_position {
            RelativePosition::Before => self.n_screen_lines_before_or_top_of_doc(screen_line, n),
            RelativePosition::After => self.n_screen_lines_after_or_bottom_of_doc(screen_line, n),
        }
    }

    pub fn document_eof(&mut self) {
        self.doc.eof();
        self.update_search_matches_after_receiving_more_data();
    }

    pub fn append_document_data(&mut self, data: &[u8]) {
        self.doc.append(data);
        self.update_search_matches_after_receiving_more_data();

        if self.tailing_end_of_document {
            self.focus_bottom();
        }
    }

    pub fn do_action(&mut self, action: Action) {
        let prev_cursor = self.current_focus.clone();

        let mut focused_bottom = false;
        let mut jumped_to_search_match = false;

        match action {
            Action::NoOp => (),
            Action::MoveCursorDown(n) => self.move_cursor_down(n),
            Action::MoveCursorUp(n) => self.move_cursor_up(n),
            Action::ExpandOrMoveCursorRightOrDown => self.expand_or_move_cursor_right_or_down(),
            Action::CollapseOrMoveCursorLeftOrUp => self.collapse_or_move_cursor_left_or_up(),
            Action::MoveCursorLeftOrUpWithoutCollapsing => {
                self.move_cursor_left_or_up_without_collapsing()
            }
            Action::MoveCursorToFirstSibling => self.move_cursor_to_first_sibling(),
            Action::MoveCursorToLastSibling => self.move_cursor_to_last_sibling(),
            Action::CollapseNodeAndSiblings(depth) => self.collapse_node_and_siblings(depth),
            Action::ExpandNodeAndSiblings(depth) => self.expand_node_and_siblings(depth),
            Action::ScrollViewportDown(n) => self.scroll_viewport_down(n),
            Action::ScrollViewportUp(n) => self.scroll_viewport_up(n),
            Action::PageDown(n) => self.page_down(n),
            Action::PageUp(n) => self.page_up(n),
            Action::JumpDown(n) => self.jump_down(n),
            Action::JumpUp(n) => self.jump_up(n),
            Action::MoveToSearchMatch(movement_method, jump_direction, n) => {
                jumped_to_search_match = true;
                self.move_to_search_match(movement_method, jump_direction, n)
            }
            Action::FocusTop => self.focus_top(),
            Action::FocusBottom => {
                focused_bottom = true;
                self.focus_bottom()
            }
            Action::MoveFocusedElemToTop => self.move_focused_elem_to_top(),
            Action::MoveFocusedElemToCenter => self.move_focused_elem_to_center(),
            Action::MoveFocusedElemToBottom => self.move_focused_elem_to_bottom(),
        }

        let cursor_moved = prev_cursor != self.current_focus;

        // Check if we need to clear the last jump of the search state.
        if let Some(search_state) = &mut self.search_state {
            if let Some(last_match_range) = search_state.last_match_range() {
                // Obviously don't clear the last jump if we just jumped.
                if !jumped_to_search_match {
                    if cursor_moved {
                        search_state.clear_last_jump();
                    } else {
                        // Even if the cursor didn't move, check if we're no longer pointing
                        // to the the match, possibly because we expanded the currently
                        // focused node.
                        let match_cursor =
                            self.doc.raw_byte_index_to_cursor(last_match_range.start);
                        if match_cursor != self.current_focus {
                            search_state.clear_last_jump();
                        }
                    }
                }
            }
        }

        // When we focus the bottom of the document, we'll start tailing the
        // end, and we stop when we move the cursor.
        if focused_bottom {
            self.tailing_end_of_document = true;
        } else if cursor_moved {
            self.tailing_end_of_document = false;
        }
    }

    ////////////
    // Search //
    ////////////

    pub fn initialize_search(
        &mut self,
        input: String,
        direction: SearchDirection,
    ) -> Result<(), String> {
        let haystack = self.doc.raw_bytes_for_searching();

        if input.is_empty() {
            return Err("Cannot initialize search with empty string".to_string());
        }

        self.search_state = Some(SearchState::new(
            input,
            haystack,
            direction,
            D::inverted_paired_delimiters_for_search_input(),
        )?);

        Ok(())
    }

    fn update_search_matches_after_receiving_more_data(&mut self) {
        if let Some(search_state) = &mut self.search_state {
            search_state.find_additional_matches(self.doc.raw_bytes_for_searching());
        }
    }

    pub fn has_initialized_search_state(&self) -> bool {
        self.search_state.is_some()
    }

    pub fn set_search_direction(&mut self, direction: SearchDirection) {
        debug_assert!(self.search_state.is_some());

        if let Some(search_state) = &mut self.search_state {
            search_state.set_search_direction(direction);
        }
    }

    // Returns `None` if no search is currently initialized.
    pub fn num_search_matches(&self) -> Option<usize> {
        match &self.search_state {
            None => None,
            Some(search_state) => Some(search_state.num_matches()),
        }
    }

    fn move_to_search_match(
        &mut self,
        movement_method: MovementMethod,
        jump_direction: JumpDirection,
        jumps: usize,
    ) {
        assert!(
            self.search_state.is_some(),
            "Shouldn't move to search match if no search state"
        );

        match movement_method {
            MovementMethod::MoveCursor => {
                let (new_focus, content_range) = self.find_search_match(jump_direction, jumps);
                self.current_focus = new_focus;
                self.update_so_content_range_is_visible(content_range);
            }
            MovementMethod::ScrollViewport => {
                // As we scroll through matches, we want the actual match to stay in the exact
                // same spot on the screen, so, to handle matches within long lines, we want to
                // get the location of the last match we jumped to, not just the current cursor.
                let current_range = match self.search_state.as_ref().unwrap().last_match_range() {
                    None => self.doc.cursor_range(&self.current_focus),
                    Some(match_byte_range) => self
                        .doc
                        .raw_byte_range_to_visible_content_range(match_byte_range),
                };

                let (new_focus, content_range) = self.find_search_match(jump_direction, jumps);

                let (ref_screen_line, at_screen_index) = match self
                    .position_of_content_in_viewport(&current_range)
                {
                    PositionOfContentInViewport::EntirelyInViewport {
                        start_index,
                        end_index,
                    } => {
                        // If the sizes are the same, put it in exactly the same spot, otherwise,
                        // make the centers line up.
                        if end_index - start_index + 1 == content_range.num_screen_lines {
                            (content_range.start, start_index)
                        } else {
                            let center_screen_line =
                                self.doc.center_of_content_range(&content_range);
                            let center_index = start_index + (end_index - start_index) / 2;
                            (center_screen_line, center_index)
                        }
                    }
                    PositionOfContentInViewport::StartsAboveViewport { end_index } => {
                        // If the current match/cursor starts above the viewport, just line up
                        // end of the new match with the end of the previous range.
                        (content_range.end, end_index)
                    }
                    PositionOfContentInViewport::EndsBelowViewport { start_index } => {
                        // Same thinking as `StartsAboveViewport` case.
                        (content_range.start, start_index)
                    }
                    PositionOfContentInViewport::StartsAndEndsOutsideViewport => {
                        // Just focus the new match in the center.
                        let center_screen_line = self.doc.center_of_content_range(&content_range);
                        let center_index = self.dimensions.height / 2;
                        (center_screen_line, center_index)
                    }
                };

                self.current_focus = new_focus;
                self.top_line =
                    self.n_screen_lines_before_or_top_of_doc(ref_screen_line, at_screen_index);
            }
        }
    }

    fn find_search_match(
        &mut self,
        jump_direction: JumpDirection,
        jumps: usize,
    ) -> (D::Cursor, ContentRange<D::ScreenLine>) {
        let search_state = self.search_state.as_mut().unwrap();

        // Only capture reference to doc/current focus, and not self, so we can still mutate
        // `search_state`.
        let doc = &self.doc;
        let current_focus = &self.current_focus;

        let cursor_will_move = |match_range: Range<usize>| {
            let cursor = doc.raw_byte_index_to_cursor(match_range.start);
            doc.closest_visible_cursor(&cursor) != *current_focus
        };

        let is_match_visible =
            |match_range: Range<usize>| doc.is_raw_byte_range_visible(match_range);

        let current_focused_range = self.doc.raw_byte_range_of_cursor(&self.current_focus);

        let match_byte_range = search_state.jump_to_next_match(
            current_focused_range,
            jump_direction,
            jumps,
            &cursor_will_move,
            &is_match_visible,
        );

        let match_cursor = self.doc.raw_byte_index_to_cursor(match_byte_range.start);
        let closest_visible_cursor = self.doc.closest_visible_cursor(&match_cursor);

        if match_cursor == closest_visible_cursor {
            // We explicitly make sure that the *match* is visible, as opposed to the cursor,
            // to handle the case where we have a very, very long line that spans more than
            // an entire screen, and we're jumping to specific matches in that line.
            let match_content_range = self
                .doc
                .raw_byte_range_to_visible_content_range(match_byte_range);
            (match_cursor, match_content_range)
        } else {
            // The match isn't visible, so we just move to the closest visible cursor
            // (likely the container that contains the match).
            let cursor_range = self.doc.cursor_range(&closest_visible_cursor);
            (closest_visible_cursor, cursor_range)
        }
    }

    ///////////////
    // Rendering //
    ///////////////

    pub fn viewport_lines<'a>(&'a self) -> impl Iterator<Item = Option<D::ScreenLine>> + 'a {
        ViewportLinesIterator {
            document: &self.doc,
            next_line: Some(self.top_line.clone()),
            remaining_height: self.dimensions.height,
        }
    }

    pub fn render(&self) -> (Vec<Vec<crate::rendering::StyledSegment>>, &[u8]) {
        let mut rendered_lines = vec![];
        let default = Attrs::default();
        let inverted = default.invert();
        let dimmed = Attrs {
            dimmed: true,
            ..default
        };

        let search_match_ranges = match &self.search_state {
            None => &[],
            Some(search_state) => search_state.search_match_ranges(),
        };

        for screen_line in self.viewport_lines() {
            match screen_line {
                None => {
                    let empty_line_segment = StyledSegment {
                        attrs: dimmed,
                        content: Text::Static("~"),
                    };
                    rendered_lines.push(vec![empty_line_segment]);
                }
                Some(screen_line) => {
                    match self
                        .doc
                        .render_screen_line(&screen_line, &self.current_focus)
                    {
                        Some(unhighlighted_segments) => {
                            let highlighted_segments: Vec<StyledSegment> = unhighlighted_segments
                                .into_iter()
                                .map(|segment| {
                                    segment.highlight_search_matches(search_match_ranges)
                                })
                                .flatten()
                                .collect();

                            rendered_lines.push(highlighted_segments);
                        }
                        None => {
                            // Fallback to `debug_text_content`
                            let fallback = self
                                .doc
                                .debug_text_content(&screen_line, &self.current_focus);
                            let s = String::from_utf8_lossy(&fallback).to_string();
                            let attrs = if self.doc.does_screen_line_intersect_cursor(
                                &screen_line,
                                &self.current_focus,
                            ) {
                                inverted
                            } else {
                                default
                            };

                            let debug_len = s.len();
                            let debug_segment = StyledSegment {
                                attrs,
                                content: Text::String((Rc::new(s), 0..debug_len)),
                            };
                            rendered_lines.push(vec![debug_segment]);
                        }
                    }
                }
            }
        }

        (rendered_lines, self.doc.raw_bytes_for_searching())
    }
}

struct ViewportLinesIterator<'a, D: Document> {
    document: &'a D,
    next_line: Option<D::ScreenLine>,
    remaining_height: usize,
}

impl<'a, D: Document> Iterator for ViewportLinesIterator<'a, D> {
    type Item = Option<D::ScreenLine>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining_height == 0 {
            return None;
        }
        self.remaining_height -= 1;

        let Some(curr_line) = self.next_line.take() else {
            return Some(None);
        };

        self.next_line = self.document.next_screen_line(&curr_line);

        Some(Some(curr_line))
    }
}

#[cfg(test)]
mod test {
    use super::*;

    use bstr::ByteSlice;
    use insta::{assert_debug_snapshot, assert_snapshot};

    use std::fmt::{self, Write};

    use crate::dimensions::Dimensions;
    use crate::document::Document;
    use crate::sexp::document::SexpDocument;
    use crate::test_helpers::format_table;
    use crate::text_document::{Cursor, TextDocument};

    fn init_doc<D: Document>(
        contents: &[u8],
        width: usize,
        height: usize,
        scrolloff: usize,
    ) -> DocumentViewer<D> {
        let mut doc = D::new(width);
        doc.append(contents);

        let (top_line, initial_cursor) = doc.top_screen_line_and_cursor().unwrap();
        let dimensions = Dimensions { width, height };
        DocumentViewer::new(doc, top_line, initial_cursor, dimensions, scrolloff)
    }

    fn init(
        contents: &[u8],
        width: usize,
        height: usize,
        scrolloff: usize,
    ) -> DocumentViewer<TextDocument> {
        init_doc::<TextDocument>(contents, width, height, scrolloff)
    }

    fn init_sexp(
        contents: &[u8],
        width: usize,
        height: usize,
        scrolloff: usize,
    ) -> DocumentViewer<SexpDocument> {
        init_doc::<SexpDocument>(contents, width, height, scrolloff)
    }

    #[derive(Clone)]
    enum Change {
        Action(Action),
        ResizeWidth(usize),
        ResizeHeight(usize),
        Resize(Dimensions),
        SetScrolloff(usize),
        AppendDocumentData(Vec<u8>),
        InitializeSearch(String, SearchDirection),
        // DocumentEof,
    }

    impl fmt::Display for Change {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
            match self {
                Change::Action(action) => {
                    // Let's shink some of these
                    match action {
                        Action::MoveToSearchMatch(movement, dir, count) => match movement {
                            MovementMethod::MoveCursor => {
                                write!(f, "JumpToSearchMatch({dir:?}, {count})")
                            }
                            MovementMethod::ScrollViewport => {
                                write!(f, "ScrollToSearchMatch({dir:?}, {count})")
                            }
                        },
                        _ => write!(f, "{:?}", action),
                    }
                }
                Change::ResizeWidth(width) => write!(f, "ResizeWidth({})", width),
                Change::ResizeHeight(height) => write!(f, "ResizeHeight({})", height),
                Change::Resize(Dimensions { width, height }) => {
                    write!(f, "Resize({{ width: {}, height: {} }})", width, height)
                }
                Change::SetScrolloff(scrolloff) => write!(f, "SetScrolloff({})", scrolloff),
                Change::AppendDocumentData(_) => write!(f, "AppendDocData"),
                Change::InitializeSearch(s, d) => write!(f, "{}{}", d.prompt_str(), s),
            }
        }
    }

    fn move_cursor_down(n: usize) -> Change {
        Change::Action(Action::MoveCursorDown(n))
    }

    fn move_cursor_up(n: usize) -> Change {
        Change::Action(Action::MoveCursorUp(n))
    }

    fn press_left() -> Change {
        Change::Action(Action::CollapseOrMoveCursorLeftOrUp)
    }

    fn press_right() -> Change {
        Change::Action(Action::ExpandOrMoveCursorRightOrDown)
    }

    fn collapse_node_and_siblings(depth: Option<usize>) -> Change {
        Change::Action(Action::CollapseNodeAndSiblings(depth))
    }

    fn expand_node_and_siblings(depth: Option<usize>) -> Change {
        Change::Action(Action::ExpandNodeAndSiblings(depth))
    }

    fn scroll_viewport_down(n: usize) -> Change {
        Change::Action(Action::ScrollViewportDown(n))
    }

    fn scroll_viewport_up(n: usize) -> Change {
        Change::Action(Action::ScrollViewportUp(n))
    }

    fn page_down(n: usize) -> Change {
        Change::Action(Action::PageDown(n))
    }

    fn page_up(n: usize) -> Change {
        Change::Action(Action::PageUp(n))
    }

    fn jump_down(n: Option<usize>) -> Change {
        let n = n.map(NonZeroUsize::new).flatten();
        Change::Action(Action::JumpDown(n))
    }

    fn jump_up(n: Option<usize>) -> Change {
        let n = n.map(NonZeroUsize::new).flatten();
        Change::Action(Action::JumpUp(n))
    }

    fn focus_top() -> Change {
        Change::Action(Action::FocusTop)
    }

    fn focus_bottom() -> Change {
        Change::Action(Action::FocusBottom)
    }

    fn move_focused_elem_to_top() -> Change {
        Change::Action(Action::MoveFocusedElemToTop)
    }

    fn move_focused_elem_to_center() -> Change {
        Change::Action(Action::MoveFocusedElemToCenter)
    }

    fn move_focused_elem_to_bottom() -> Change {
        Change::Action(Action::MoveFocusedElemToBottom)
    }

    fn resize_width(width: usize) -> Change {
        Change::ResizeWidth(width)
    }

    fn resize_height(height: usize) -> Change {
        Change::ResizeHeight(height)
    }

    fn resize(dimensions: Dimensions) -> Change {
        Change::Resize(dimensions)
    }

    fn set_scrolloff(scrolloff: usize) -> Change {
        Change::SetScrolloff(scrolloff)
    }

    fn append_document_data(data: &[u8]) -> Change {
        Change::AppendDocumentData(data.to_vec())
    }

    fn initialize_search(search_input: &str, search_direction: SearchDirection) -> Change {
        Change::InitializeSearch(search_input.to_string(), search_direction)
    }

    fn jump_to_next_match(jumps: usize) -> Change {
        Change::Action(Action::MoveToSearchMatch(
            MovementMethod::MoveCursor,
            JumpDirection::Next,
            jumps,
        ))
    }

    fn jump_to_prev_match(jumps: usize) -> Change {
        Change::Action(Action::MoveToSearchMatch(
            MovementMethod::MoveCursor,
            JumpDirection::Prev,
            jumps,
        ))
    }

    fn scroll_to_next_match(jumps: usize) -> Change {
        Change::Action(Action::MoveToSearchMatch(
            MovementMethod::ScrollViewport,
            JumpDirection::Next,
            jumps,
        ))
    }

    fn scroll_to_prev_match(jumps: usize) -> Change {
        Change::Action(Action::MoveToSearchMatch(
            MovementMethod::ScrollViewport,
            JumpDirection::Prev,
            jumps,
        ))
    }

    impl<D: Document> DocumentViewer<D> {
        fn debug_render(&self) -> String {
            // |12345678       9|
            // | ##|##| <width> |
            let content_width = self.dimensions.width;
            let mut s = String::new();
            writeln!(s, "┌SI┬─L#┬─{:─<content_width$}─┐", "").unwrap();
            for (screen_index, screen_line) in self.viewport_lines().enumerate() {
                let Some(screen_line) = screen_line else {
                    writeln!(s, "│{:>2}│ ~ │ {: <content_width$} │", screen_index, "").unwrap();
                    continue;
                };

                let is_focused = self
                    .doc
                    .does_screen_line_intersect_cursor(&screen_line, &self.current_focus);
                let line_number = self.doc.line_number(&screen_line);
                let wraps_from_prev_line = self.doc.is_after_start_of_wrapped_line(&screen_line);
                let wraps_onto_next_line = self.doc.is_before_end_of_wrapped_line(&screen_line);

                writeln!(
                    s,
                    "│{:>2}│{}{:<2}│{}{: <content_width$}{}│",
                    screen_index,
                    if is_focused { '*' } else { ' ' },
                    line_number,
                    if wraps_from_prev_line { '↪' } else { ' ' },
                    self.doc
                        .debug_text_content(&screen_line, &self.current_focus)
                        .as_bstr(),
                    if wraps_onto_next_line { '↩' } else { ' ' },
                )
                .unwrap();
            }
            writeln!(s, "└──┴───┴─{:─<content_width$}─┘", "").unwrap();
            s
        }

        fn do_change(&mut self, change: Change) {
            match change {
                Change::Action(action) => self.do_action(action),
                Change::ResizeWidth(width) => self.resize_width(width),
                Change::ResizeHeight(height) => self.resize_height(height),
                Change::Resize(dimensions) => self.resize(dimensions),
                Change::SetScrolloff(scrolloff) => self.set_scrolloff(scrolloff),
                Change::AppendDocumentData(data) => self.append_document_data(&data),
                Change::InitializeSearch(search_input, search_direction) => self
                    .initialize_search(search_input, search_direction)
                    .unwrap(),
            }
        }
    }

    fn run<D: Document>(viewer: &mut DocumentViewer<D>, mut changes: Vec<Vec<Change>>) -> String {
        changes.insert(0, vec![]);

        let formatted_changes: Vec<String> = changes
            .iter()
            .map(|changes| {
                changes
                    .iter()
                    .map(Change::to_string)
                    .collect::<Vec<String>>()
                    .join("\n")
            })
            .collect();

        let renders: Vec<String> = changes
            .into_iter()
            .map(|changes| {
                for change in changes.into_iter() {
                    viewer.do_change(change);
                }
                viewer.debug_render()
            })
            .collect();

        format_table(&vec![formatted_changes, renders], false)
    }

    #[test]
    fn test_render() {
        let mut viewer = init(b"aaa\nbb\ncccc\ndddddd\ne\n", 4, 7, 0);
        let output = run(&mut viewer, vec![]);
        assert_snapshot!(output, @r"
        ┌SI┬─L#┬──────┐
        │ 0│*1 │ aaa  │
        │ 1│ 2 │ bb   │
        │ 2│ 3 │ cccc │
        │ 3│ 4 │ dddd↩│
        │ 4│ 4 │↪dd   │
        │ 5│ 5 │ e    │
        │ 6│ ~ │      │
        └──┴───┴──────┘
        ");
    }

    fn acceptable_screen_indexes(
        viewer: &DocumentViewer<TextDocument>,
        cursor: &Cursor,
    ) -> AcceptableStartScreenIndexesToShowEntireContentRange {
        let cursor_range = viewer.doc.cursor_range(cursor);
        viewer.calculate_acceptable_start_screen_indexes_to_show_entire_content_range(&cursor_range)
    }

    #[test]
    fn test_acceptable_start_screen_indexes() {
        let mut viewer = init(b"a\nbbbb\nc\nd\ne\nf\ng\n", 1, 10, 0);
        assert_snapshot!(viewer.debug_render(), @r"
        ┌SI┬─L#┬───┐
        │ 0│*1 │ a │
        │ 1│ 2 │ b↩│
        │ 2│ 2 │↪b↩│
        │ 3│ 2 │↪b↩│
        │ 4│ 2 │↪b │
        │ 5│ 3 │ c │
        │ 6│ 4 │ d │
        │ 7│ 5 │ e │
        │ 8│ 6 │ f │
        │ 9│ 7 │ g │
        └──┴───┴───┘
        ");

        let line_2 = viewer.doc.cursor_to_line_n(2);
        assert_debug_snapshot!(acceptable_screen_indexes(&viewer, &line_2), @r"
        AcceptableStartScreenIndexesToShowEntireContentRange {
            content_height: 4,
            last_screen_index: 9,
            range_after_considering_scrolloff: 0..=9,
            range_after_relaxing_scrolloff_due_to_proximity_of_doc_ends: 0..=9,
            range_after_expanding_due_to_content_height: 0..=9,
            start_indexes_without_clamping_at_lines_to_start_of_doc: 0..=6,
            start: 0,
            end: 1,
        }
        ");

        viewer.set_scrolloff(3);
        assert_debug_snapshot!(acceptable_screen_indexes(&viewer, &line_2), @r"
        AcceptableStartScreenIndexesToShowEntireContentRange {
            content_height: 4,
            last_screen_index: 9,
            range_after_considering_scrolloff: 3..=6,
            range_after_relaxing_scrolloff_due_to_proximity_of_doc_ends: 1..=6,
            range_after_expanding_due_to_content_height: 1..=6,
            start_indexes_without_clamping_at_lines_to_start_of_doc: 1..=3,
            start: 1,
            end: 1,
        }
        ");

        // Example from the comment in
        // `calculate_acceptable_start_screen_indexes_to_show_entire_content_range`:
        let viewer = init(b"a\nbbbbbbbb\nc\nd\ne\nf\n", 1, 10, 4);
        assert_snapshot!(viewer.debug_render(), @r"
        ┌SI┬─L#┬───┐
        │ 0│*1 │ a │
        │ 1│ 2 │ b↩│
        │ 2│ 2 │↪b↩│
        │ 3│ 2 │↪b↩│
        │ 4│ 2 │↪b↩│
        │ 5│ 2 │↪b↩│
        │ 6│ 2 │↪b↩│
        │ 7│ 2 │↪b↩│
        │ 8│ 2 │↪b │
        │ 9│ 3 │ c │
        └──┴───┴───┘
        ");

        let line_2 = viewer.doc.cursor_to_line_n(2);
        assert_debug_snapshot!(acceptable_screen_indexes(&viewer, &line_2), @r"
        AcceptableStartScreenIndexesToShowEntireContentRange {
            content_height: 8,
            last_screen_index: 9,
            range_after_considering_scrolloff: 4..=5,
            range_after_relaxing_scrolloff_due_to_proximity_of_doc_ends: 1..=5,
            range_after_expanding_due_to_content_height: 1..=8,
            start_indexes_without_clamping_at_lines_to_start_of_doc: 1..=1,
            start: 1,
            end: 1,
        }
        ");
    }

    #[test]
    fn test_move_cursor_up_and_down() {
        let mut viewer = init(b"aaa\nbb\ncccc\ndddddd\ne\n", 4, 7, 0);

        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(1)],
                vec![move_cursor_down(3)],
                vec![move_cursor_up(1)],
                vec![move_cursor_up(10)],
            ],
        );
        assert_snapshot!(output, @r"
                        MoveCursorDown(1) MoveCursorDown(3) MoveCursorUp(1) MoveCursorUp(10)
        ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐   ┌SI┬─L#┬──────┐   ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐
        │ 0│*1 │ aaa  │ │ 0│ 1 │ aaa  │   │ 0│ 1 │ aaa  │   │ 0│ 1 │ aaa  │ │ 0│*1 │ aaa  │
        │ 1│ 2 │ bb   │ │ 1│*2 │ bb   │   │ 1│ 2 │ bb   │   │ 1│ 2 │ bb   │ │ 1│ 2 │ bb   │
        │ 2│ 3 │ cccc │ │ 2│ 3 │ cccc │   │ 2│ 3 │ cccc │   │ 2│ 3 │ cccc │ │ 2│ 3 │ cccc │
        │ 3│ 4 │ dddd↩│ │ 3│ 4 │ dddd↩│   │ 3│ 4 │ dddd↩│   │ 3│*4 │ dddd↩│ │ 3│ 4 │ dddd↩│
        │ 4│ 4 │↪dd   │ │ 4│ 4 │↪dd   │   │ 4│ 4 │↪dd   │   │ 4│*4 │↪dd   │ │ 4│ 4 │↪dd   │
        │ 5│ 5 │ e    │ │ 5│ 5 │ e    │   │ 5│*5 │ e    │   │ 5│ 5 │ e    │ │ 5│ 5 │ e    │
        │ 6│ ~ │      │ │ 6│ ~ │      │   │ 6│ ~ │      │   │ 6│ ~ │      │ │ 6│ ~ │      │
        └──┴───┴──────┘ └──┴───┴──────┘   └──┴───┴──────┘   └──┴───┴──────┘ └──┴───┴──────┘
        ");
    }

    #[test]
    fn test_move_cursor_up_and_down_and_move_viewport() {
        let mut viewer = init(
            b"aaa\nbb\ncccc\ndddddd\neeeeeee\nff\nggggg\nhh\ni\n",
            4,
            5,
            1,
        );
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(1)],
                vec![move_cursor_down(2)],
                vec![move_cursor_down(1)],
                vec![move_cursor_up(1)],
                vec![move_cursor_down(100)],
            ],
        );
        assert_snapshot!(output, @r"
                        MoveCursorDown(1) MoveCursorDown(2) MoveCursorDown(1) MoveCursorUp(1) MoveCursorDown(100)
        ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐   ┌SI┬─L#┬──────┐   ┌SI┬─L#┬──────┐   ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐
        │ 0│*1 │ aaa  │ │ 0│ 1 │ aaa  │   │ 0│ 2 │ bb   │   │ 0│ 4 │ dddd↩│   │ 0│ 3 │ cccc │ │ 0│ 6 │ ff   │
        │ 1│ 2 │ bb   │ │ 1│*2 │ bb   │   │ 1│ 3 │ cccc │   │ 1│ 4 │↪dd   │   │ 1│*4 │ dddd↩│ │ 1│ 7 │ gggg↩│
        │ 2│ 3 │ cccc │ │ 2│ 3 │ cccc │   │ 2│*4 │ dddd↩│   │ 2│*5 │ eeee↩│   │ 2│*4 │↪dd   │ │ 2│ 7 │↪g    │
        │ 3│ 4 │ dddd↩│ │ 3│ 4 │ dddd↩│   │ 3│*4 │↪dd   │   │ 3│*5 │↪eee  │   │ 3│ 5 │ eeee↩│ │ 3│ 8 │ hh   │
        │ 4│ 4 │↪dd   │ │ 4│ 4 │↪dd   │   │ 4│ 5 │ eeee↩│   │ 4│ 6 │ ff   │   │ 4│ 5 │↪eee  │ │ 4│*9 │ i    │
        └──┴───┴──────┘ └──┴───┴──────┘   └──┴───┴──────┘   └──┴───┴──────┘   └──┴───┴──────┘ └──┴───┴──────┘
        ");
    }

    #[test]
    fn test_move_cursor_up_and_down_to_very_long_line() {
        let mut viewer = init(b"a\nb\nc\nd\ne1e2e3e4e5e6e7e8\nf\ng\nh\ni\n", 2, 4, 1);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(3)],
                vec![move_cursor_down(1)],
                vec![move_cursor_down(1)],
            ],
        );
        assert_snapshot!(output, @r"
                      MoveCursorDown(3) MoveCursorDown(1) MoveCursorDown(1)
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐     ┌SI┬─L#┬────┐     ┌SI┬─L#┬────┐
        │ 0│*1 │ a  │ │ 0│ 2 │ b  │     │ 0│*5 │ e1↩│     │ 0│ 5 │↪e7↩│
        │ 1│ 2 │ b  │ │ 1│ 3 │ c  │     │ 1│*5 │↪e2↩│     │ 1│ 5 │↪e8 │
        │ 2│ 3 │ c  │ │ 2│*4 │ d  │     │ 2│*5 │↪e3↩│     │ 2│*6 │ f  │
        │ 3│ 4 │ d  │ │ 3│ 5 │ e1↩│     │ 3│*5 │↪e4↩│     │ 3│ 7 │ g  │
        └──┴───┴────┘ └──┴───┴────┘     └──┴───┴────┘     └──┴───┴────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(100)],
                vec![move_cursor_up(3)],
                vec![move_cursor_up(1)],
                vec![move_cursor_up(1)],
            ],
        );
        assert_snapshot!(output, @r"
                      MoveCursorDown(100) MoveCursorUp(3) MoveCursorUp(1) MoveCursorUp(1)
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐       ┌SI┬─L#┬────┐   ┌SI┬─L#┬────┐   ┌SI┬─L#┬────┐
        │ 0│ 5 │↪e7↩│ │ 0│ 6 │ f  │       │ 0│ 5 │↪e8 │   │ 0│*5 │ e1↩│   │ 0│ 3 │ c  │
        │ 1│ 5 │↪e8 │ │ 1│ 7 │ g  │       │ 1│*6 │ f  │   │ 1│*5 │↪e2↩│   │ 1│*4 │ d  │
        │ 2│*6 │ f  │ │ 2│ 8 │ h  │       │ 2│ 7 │ g  │   │ 2│*5 │↪e3↩│   │ 2│ 5 │ e1↩│
        │ 3│ 7 │ g  │ │ 3│*9 │ i  │       │ 3│ 8 │ h  │   │ 3│*5 │↪e4↩│   │ 3│ 5 │↪e2↩│
        └──┴───┴────┘ └──┴───┴────┘       └──┴───┴────┘   └──┴───┴────┘   └──┴───┴────┘
        ");
    }

    #[test]
    fn test_left_and_right_dont_cache_top_line() {
        let text = b"((a 1)(b 1)) ((a 2)(b 2))";
        let mut viewer = init_sexp(text, 7, 4, 0);

        let output = run(&mut viewer, vec![vec![press_left()]]);
        assert_snapshot!(output, @r"
                           CollapseOrMoveCursorLeftOrUp
        ┌SI┬─L#┬─────────┐ ┌SI┬─L#┬─────────┐
        │ 0│*1 │ [(a 1)  │ │ 0│*1 │ […)     │
        │ 1│ 2 │  (b 1)) │ │ 1│ 2 │ ((a 2)  │
        │ 2│ 2 │ ((a 2)  │ │ 2│ 2 │  (b 2)) │
        │ 3│ 2 │  (b 2)) │ │ 3│ ~ │         │
        └──┴───┴─────────┘ └──┴───┴─────────┘
        ");

        let text = b"w ((a 1)(b 2)) x y z";
        let mut viewer = init_sexp(text, 7, 4, 0);

        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(1), press_left(), scroll_viewport_down(1)],
                vec![press_right()],
            ],
        );
        assert_snapshot!(output, @r"
                           MoveCursorDown(1)            ExpandOrMoveCursorRightOrDown
                           CollapseOrMoveCursorLeftOrUp
                           ScrollViewportDown(1)
        ┌SI┬─L#┬─────────┐ ┌SI┬─L#┬─────────┐           ┌SI┬─L#┬─────────┐
        │ 0│*1 │ *       │ │ 0│*2 │ […)     │           │ 0│*2 │ [(a 1)  │
        │ 1│ 2 │ ((a 1)  │ │ 1│ 2 │ x       │           │ 1│ 2 │  (b 2)) │
        │ 2│ 2 │  (b 2)) │ │ 2│ 2 │ y       │           │ 2│ 2 │ x       │
        │ 3│ 2 │ x       │ │ 3│ ~ │         │           │ 3│ 2 │ y       │
        └──┴───┴─────────┘ └──┴───┴─────────┘           └──┴───┴─────────┘
        ");
    }

    #[test]
    fn test_collapse_and_expand_siblings_keeps_focus_at_same_screen_index() {
        let text = b"((a 1)(b 1)) ((a 2)(b 2)) ((a 3)(b 3)) ((a 4)(b 4)) ((a 5)(b 5))";
        let mut viewer = init_sexp(text, 7, 4, 0);

        let output = run(&mut viewer, vec![vec![collapse_node_and_siblings(None)]]);
        assert_snapshot!(output, @r"
                           CollapseNodeAndSiblings(None)
        ┌SI┬─L#┬─────────┐ ┌SI┬─L#┬─────────┐
        │ 0│*1 │ [(a 1)  │ │ 0│*1 │ […)     │
        │ 1│ 2 │  (b 1)) │ │ 1│ 2 │ (…)     │
        │ 2│ 2 │ ((a 2)  │ │ 2│ 2 │ (…)     │
        │ 3│ 2 │  (b 2)) │ │ 3│ 2 │ (…)     │
        └──┴───┴─────────┘ └──┴───┴─────────┘
        ");

        let text = b"((a 1)(b 1)) ((a 2)(b 2)) ((a 3)(b 3)) ((a 4)(b 4)) ((a 5)(b 5))";
        let mut viewer = init_sexp(text, 7, 4, 0);

        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(2), scroll_viewport_down(1)],
                vec![collapse_node_and_siblings(None)],
            ],
        );
        assert_snapshot!(output, @r"
                           MoveCursorDown(2)     CollapseNodeAndSiblings(None)
                           ScrollViewportDown(1)
        ┌SI┬─L#┬─────────┐ ┌SI┬─L#┬─────────┐    ┌SI┬─L#┬─────────┐
        │ 0│*1 │ [(a 1)  │ │ 0│ 2 │  (b 1)) │    │ 0│ 1 │ (…)     │
        │ 1│ 2 │  (b 1)) │ │ 1│*2 │ [(a 2)  │    │ 1│*2 │ […)     │
        │ 2│ 2 │ ((a 2)  │ │ 2│ 2 │  (b 2)) │    │ 2│ 2 │ (…)     │
        │ 3│ 2 │  (b 2)) │ │ 3│ 2 │ ((a 3)  │    │ 3│ 2 │ (…)     │
        └──┴───┴─────────┘ └──┴───┴─────────┘    └──┴───┴─────────┘
        ");

        let text = b"((a 1)(b 1)) ((a 2)(b 2)) ((a 3)(b 3)) ((a 4)(b 4)) ((a 5)(b 5))";
        let mut viewer = init_sexp(text, 7, 4, 0);

        let output = run(
            &mut viewer,
            vec![
                vec![
                    move_cursor_down(2),
                    press_left(),
                    move_cursor_down(1),
                    press_left(),
                    scroll_viewport_down(2),
                ],
                vec![expand_node_and_siblings(None)],
            ],
        );
        assert_snapshot!(output, @r"
                           MoveCursorDown(2)            ExpandNodeAndSiblings(None)
                           CollapseOrMoveCursorLeftOrUp
                           MoveCursorDown(1)
                           CollapseOrMoveCursorLeftOrUp
                           ScrollViewportDown(2)
        ┌SI┬─L#┬─────────┐ ┌SI┬─L#┬─────────┐           ┌SI┬─L#┬─────────┐
        │ 0│*1 │ [(a 1)  │ │ 0│ 2 │ (…)     │           │ 0│ 2 │  (b 2)) │
        │ 1│ 2 │  (b 1)) │ │ 1│*2 │ […)     │           │ 1│*2 │ [(a 3)  │
        │ 2│ 2 │ ((a 2)  │ │ 2│ 2 │ ((a 4)  │           │ 2│ 2 │  (b 3)) │
        │ 3│ 2 │  (b 2)) │ │ 3│ 2 │  (b 4)) │           │ 3│ 2 │ ((a 4)  │
        └──┴───┴─────────┘ └──┴───┴─────────┘           └──┴───┴─────────┘
        ");
    }

    #[test]
    fn test_scroll_up_and_down() {
        let mut viewer = init(
            b"aaa\nbb\ncccc\ndddddd\neeeeeee\nff\nggggg\nhh\ni\n",
            4,
            5,
            1,
        );
        let output = run(
            &mut viewer,
            vec![
                vec![scroll_viewport_down(1)],
                vec![scroll_viewport_down(1)],
                vec![scroll_viewport_down(1)],
                vec![scroll_viewport_down(1)],
                vec![scroll_viewport_down(10)],
            ],
        );
        assert_snapshot!(output, @r"
                        ScrollViewportDown(1) ScrollViewportDown(1) ScrollViewportDown(1) ScrollViewportDown(1) ScrollViewportDown(10)
        ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐       ┌SI┬─L#┬──────┐       ┌SI┬─L#┬──────┐       ┌SI┬─L#┬──────┐       ┌SI┬─L#┬──────┐
        │ 0│*1 │ aaa  │ │ 0│ 2 │ bb   │       │ 0│ 3 │ cccc │       │ 0│*4 │ dddd↩│       │ 0│ 4 │↪dd   │       │ 0│*9 │ i    │
        │ 1│ 2 │ bb   │ │ 1│*3 │ cccc │       │ 1│*4 │ dddd↩│       │ 1│*4 │↪dd   │       │ 1│*5 │ eeee↩│       │ 1│ ~ │      │
        │ 2│ 3 │ cccc │ │ 2│ 4 │ dddd↩│       │ 2│*4 │↪dd   │       │ 2│ 5 │ eeee↩│       │ 2│*5 │↪eee  │       │ 2│ ~ │      │
        │ 3│ 4 │ dddd↩│ │ 3│ 4 │↪dd   │       │ 3│ 5 │ eeee↩│       │ 3│ 5 │↪eee  │       │ 3│ 6 │ ff   │       │ 3│ ~ │      │
        │ 4│ 4 │↪dd   │ │ 4│ 5 │ eeee↩│       │ 4│ 5 │↪eee  │       │ 4│ 6 │ ff   │       │ 4│ 7 │ gggg↩│       │ 4│ ~ │      │
        └──┴───┴──────┘ └──┴───┴──────┘       └──┴───┴──────┘       └──┴───┴──────┘       └──┴───┴──────┘       └──┴───┴──────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![scroll_viewport_up(4)],
                vec![scroll_viewport_up(1)],
                vec![scroll_viewport_up(1)],
                vec![scroll_viewport_up(1)],
                vec![scroll_viewport_up(10)],
            ],
        );
        assert_snapshot!(output, @r"
                        ScrollViewportUp(4) ScrollViewportUp(1) ScrollViewportUp(1) ScrollViewportUp(1) ScrollViewportUp(10)
        ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐     ┌SI┬─L#┬──────┐     ┌SI┬─L#┬──────┐     ┌SI┬─L#┬──────┐     ┌SI┬─L#┬──────┐
        │ 0│*9 │ i    │ │ 0│ 6 │ ff   │     │ 0│ 5 │↪eee  │     │ 0│ 5 │ eeee↩│     │ 0│ 4 │↪dd   │     │ 0│ 1 │ aaa  │
        │ 1│ ~ │      │ │ 1│ 7 │ gggg↩│     │ 1│ 6 │ ff   │     │ 1│ 5 │↪eee  │     │ 1│ 5 │ eeee↩│     │ 1│ 2 │ bb   │
        │ 2│ ~ │      │ │ 2│ 7 │↪g    │     │ 2│*7 │ gggg↩│     │ 2│ 6 │ ff   │     │ 2│ 5 │↪eee  │     │ 2│ 3 │ cccc │
        │ 3│ ~ │      │ │ 3│*8 │ hh   │     │ 3│*7 │↪g    │     │ 3│*7 │ gggg↩│     │ 3│*6 │ ff   │     │ 3│*4 │ dddd↩│
        │ 4│ ~ │      │ │ 4│ 9 │ i    │     │ 4│ 8 │ hh   │     │ 4│*7 │↪g    │     │ 4│ 7 │ gggg↩│     │ 4│*4 │↪dd   │
        └──┴───┴──────┘ └──┴───┴──────┘     └──┴───┴──────┘     └──┴───┴──────┘     └──┴───┴──────┘     └──┴───┴──────┘
        ");
    }

    #[test]
    fn test_page_up_and_down() {
        let mut viewer = init(b"a\nb\nc\nd\ne\nf\ng\nh\ni\nj\nk\n", 1, 3, 1);

        let output = run(
            &mut viewer,
            vec![vec![page_down(1)], vec![page_down(2)], vec![page_down(1)]],
        );
        assert_snapshot!(output, @r"
                     PageDown(1)  PageDown(2)  PageDown(1)
        ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐
        │ 0│*1 │ a │ │ 0│ 4 │ d │ │ 0│ 10│ j │ │ 0│*11│ k │
        │ 1│ 2 │ b │ │ 1│*5 │ e │ │ 1│*11│ k │ │ 1│ ~ │   │
        │ 2│ 3 │ c │ │ 2│ 6 │ f │ │ 2│ ~ │   │ │ 2│ ~ │   │
        └──┴───┴───┘ └──┴───┴───┘ └──┴───┴───┘ └──┴───┴───┘
        ");

        let output = run(
            &mut viewer,
            vec![vec![page_up(1)], vec![page_up(2)], vec![page_up(1)]],
        );
        assert_snapshot!(output, @r"
                     PageUp(1)    PageUp(2)    PageUp(1)
        ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐
        │ 0│*11│ k │ │ 0│ 8 │ h │ │ 0│ 2 │ b │ │ 0│ 1 │ a │
        │ 1│ ~ │   │ │ 1│*9 │ i │ │ 1│*3 │ c │ │ 1│*2 │ b │
        │ 2│ ~ │   │ │ 2│ 10│ j │ │ 2│ 4 │ d │ │ 2│ 3 │ c │
        └──┴───┴───┘ └──┴───┴───┘ └──┴───┴───┘ └──┴───┴───┘
        ");
    }

    #[test]
    fn test_scrolling_with_very_long_line() {
        let mut viewer = init(b"a\nb\nc1c2c3c4c5c6c7c8\nd\ne\n", 2, 4, 1);
        let output = run(
            &mut viewer,
            vec![
                vec![scroll_viewport_down(2)],
                vec![scroll_viewport_down(4)],
                vec![scroll_viewport_down(2)],
                vec![scroll_viewport_down(1)],
            ],
        );
        assert_snapshot!(output, @r"
                      ScrollViewportDown(2) ScrollViewportDown(4) ScrollViewportDown(2) ScrollViewportDown(1)
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐         ┌SI┬─L#┬────┐         ┌SI┬─L#┬────┐         ┌SI┬─L#┬────┐
        │ 0│*1 │ a  │ │ 0│*3 │ c1↩│         │ 0│*3 │↪c5↩│         │ 0│*3 │↪c7↩│         │ 0│ 3 │↪c8 │
        │ 1│ 2 │ b  │ │ 1│*3 │↪c2↩│         │ 1│*3 │↪c6↩│         │ 1│*3 │↪c8 │         │ 1│*4 │ d  │
        │ 2│ 3 │ c1↩│ │ 2│*3 │↪c3↩│         │ 2│*3 │↪c7↩│         │ 2│ 4 │ d  │         │ 2│ 5 │ e  │
        │ 3│ 3 │↪c2↩│ │ 3│*3 │↪c4↩│         │ 3│*3 │↪c8 │         │ 3│ 5 │ e  │         │ 3│ ~ │    │
        └──┴───┴────┘ └──┴───┴────┘         └──┴───┴────┘         └──┴───┴────┘         └──┴───┴────┘
        ");

        let output = run(
            &mut viewer,
            vec![vec![scroll_viewport_up(2)], vec![scroll_viewport_up(7)]],
        );
        assert_snapshot!(output, @r"
                      ScrollViewportUp(2) ScrollViewportUp(7)
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐       ┌SI┬─L#┬────┐
        │ 0│ 3 │↪c8 │ │ 0│*3 │↪c6↩│       │ 0│ 1 │ a  │
        │ 1│*4 │ d  │ │ 1│*3 │↪c7↩│       │ 1│ 2 │ b  │
        │ 2│ 5 │ e  │ │ 2│*3 │↪c8 │       │ 2│*3 │ c1↩│
        │ 3│ ~ │    │ │ 3│ 4 │ d  │       │ 3│*3 │↪c2↩│
        └──┴───┴────┘ └──┴───┴────┘       └──┴───┴────┘
        ");
    }

    #[test]
    fn test_jump_up_and_down() {
        let mut viewer = init(b"a\nbb\nc\nddd\ne\nff\ng\nhhh\ni\nj\nk\nl\n", 1, 6, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(2), scroll_viewport_down(1)],
                vec![jump_down(None)],    // Jump by half screen height (3)
                vec![jump_down(Some(2))], // Jump by 2
                vec![jump_down(None)],    // Still jump by 2
                vec![jump_up(None)],      // Still jump by 2
                vec![resize_height(7), resize_height(6), jump_down(None)], // After resize, resets to 3
            ],
        );
        assert_snapshot!(output, @r"
                     MoveCursorDown(2)     JumpDown(None) JumpDown(Some(2)) JumpDown(None) JumpUp(None) ResizeHeight(7)
                     ScrollViewportDown(1)                                                              ResizeHeight(6)
                                                                                                        JumpDown(None)
        ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐          ┌SI┬─L#┬───┐   ┌SI┬─L#┬───┐      ┌SI┬─L#┬───┐   ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐
        │ 0│*1 │ a │ │ 0│ 2 │ b↩│          │ 0│*4 │ d↩│   │ 0│ 4 │↪d │      │ 0│ 6 │ f↩│   │ 0│ 4 │↪d │ │ 0│ 6 │↪f │
        │ 1│ 2 │ b↩│ │ 1│ 2 │↪b │          │ 1│*4 │↪d↩│   │ 1│ 5 │ e │      │ 1│ 6 │↪f │   │ 1│ 5 │ e │ │ 1│ 7 │ g │
        │ 2│ 2 │↪b │ │ 2│*3 │ c │          │ 2│*4 │↪d │   │ 2│*6 │ f↩│      │ 2│ 7 │ g │   │ 2│*6 │ f↩│ │ 2│*8 │ h↩│
        │ 3│ 3 │ c │ │ 3│ 4 │ d↩│          │ 3│ 5 │ e │   │ 3│*6 │↪f │      │ 3│*8 │ h↩│   │ 3│*6 │↪f │ │ 3│*8 │↪h↩│
        │ 4│ 4 │ d↩│ │ 4│ 4 │↪d↩│          │ 4│ 6 │ f↩│   │ 4│ 7 │ g │      │ 4│*8 │↪h↩│   │ 4│ 7 │ g │ │ 4│*8 │↪h │
        │ 5│ 4 │↪d↩│ │ 5│ 4 │↪d │          │ 5│ 6 │↪f │   │ 5│ 8 │ h↩│      │ 5│*8 │↪h │   │ 5│ 8 │ h↩│ │ 5│ 9 │ i │
        └──┴───┴───┘ └──┴───┴───┘          └──┴───┴───┘   └──┴───┴───┘      └──┴───┴───┘   └──┴───┴───┘ └──┴───┴───┘
        ");
        viewer.do_action(Action::FocusBottom);
        viewer.do_action(Action::ScrollViewportUp(1));
        viewer.do_action(Action::MoveCursorUp(2));
        let output = run(
            &mut viewer,
            vec![
                vec![jump_down(Some(5))],
                vec![jump_down(None)],
                vec![move_cursor_up(3), scroll_viewport_down(1)],
                vec![jump_down(Some(2))],
                vec![jump_down(None)],
                vec![jump_up(None)],
            ],
        );
        assert_snapshot!(output, @r"
                     JumpDown(Some(5)) JumpDown(None) MoveCursorUp(3)       JumpDown(Some(2)) JumpDown(None) JumpUp(None)
                                                      ScrollViewportDown(1)
        ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐      ┌SI┬─L#┬───┐   ┌SI┬─L#┬───┐          ┌SI┬─L#┬───┐      ┌SI┬─L#┬───┐   ┌SI┬─L#┬───┐
        │ 0│ 8 │ h↩│ │ 0│ 8 │↪h↩│      │ 0│ 8 │↪h↩│   │ 0│ 8 │↪h │          │ 0│ 8 │↪h │      │ 0│ 8 │↪h │   │ 0│ 8 │ h↩│
        │ 1│ 8 │↪h↩│ │ 1│ 8 │↪h │      │ 1│ 8 │↪h │   │ 1│*9 │ i │          │ 1│ 9 │ i │      │ 1│ 9 │ i │   │ 1│ 8 │↪h↩│
        │ 2│ 8 │↪h │ │ 2│ 9 │ i │      │ 2│ 9 │ i │   │ 2│ 10│ j │          │ 2│ 10│ j │      │ 2│ 10│ j │   │ 2│ 8 │↪h │
        │ 3│*9 │ i │ │ 3│*10│ j │      │ 3│ 10│ j │   │ 3│ 11│ k │          │ 3│*11│ k │      │ 3│ 11│ k │   │ 3│ 9 │ i │
        │ 4│ 10│ j │ │ 4│ 11│ k │      │ 4│ 11│ k │   │ 4│ 12│ l │          │ 4│ 12│ l │      │ 4│*12│ l │   │ 4│*10│ j │
        │ 5│ 11│ k │ │ 5│ 12│ l │      │ 5│*12│ l │   │ 5│ ~ │   │          │ 5│ ~ │   │      │ 5│ ~ │   │   │ 5│ 11│ k │
        └──┴───┴───┘ └──┴───┴───┘      └──┴───┴───┘   └──┴───┴───┘          └──┴───┴───┘      └──┴───┴───┘   └──┴───┴───┘
        ");
        viewer.do_action(Action::FocusTop);
        viewer.do_action(Action::ScrollViewportDown(1));
        viewer.do_action(Action::MoveCursorDown(2));
        let output = run(
            &mut viewer,
            vec![
                vec![jump_up(Some(2))],
                vec![jump_up(None)],
                vec![jump_up(None)],
                vec![jump_up(None)],
            ],
        );
        assert_snapshot!(output, @r"
                     JumpUp(Some(2)) JumpUp(None) JumpUp(None) JumpUp(None)
        ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐    ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐ ┌SI┬─L#┬───┐
        │ 0│ 2 │ b↩│ │ 0│ 1 │ a │    │ 0│ 1 │ a │ │ 0│*1 │ a │ │ 0│*1 │ a │
        │ 1│ 2 │↪b │ │ 1│ 2 │ b↩│    │ 1│*2 │ b↩│ │ 1│ 2 │ b↩│ │ 1│ 2 │ b↩│
        │ 2│ 3 │ c │ │ 2│ 2 │↪b │    │ 2│*2 │↪b │ │ 2│ 2 │↪b │ │ 2│ 2 │↪b │
        │ 3│*4 │ d↩│ │ 3│*3 │ c │    │ 3│ 3 │ c │ │ 3│ 3 │ c │ │ 3│ 3 │ c │
        │ 4│*4 │↪d↩│ │ 4│ 4 │ d↩│    │ 4│ 4 │ d↩│ │ 4│ 4 │ d↩│ │ 4│ 4 │ d↩│
        │ 5│*4 │↪d │ │ 5│ 4 │↪d↩│    │ 5│ 4 │↪d↩│ │ 5│ 4 │↪d↩│ │ 5│ 4 │↪d↩│
        └──┴───┴───┘ └──┴───┴───┘    └──┴───┴───┘ └──┴───┴───┘ └──┴───┴───┘
        ");
    }

    #[test]
    fn test_focus_top_and_bottom() {
        let mut viewer = init(b"a\nb\nc\nd\ne\nffff\n", 2, 5, 1);
        let output = run(
            &mut viewer,
            vec![
                vec![focus_bottom()],
                vec![move_cursor_up(1), scroll_viewport_down(1)],
                // Don't move the last line to the bottom of the viewport
                // if it's already visible.
                vec![focus_bottom()],
                vec![focus_top()],
            ],
        );
        assert_snapshot!(output, @r"
                      FocusBottom   MoveCursorUp(1)       FocusBottom   FocusTop
                                    ScrollViewportDown(1)
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐         ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐
        │ 0│*1 │ a  │ │ 0│ 3 │ c  │ │ 0│ 4 │ d  │         │ 0│ 4 │ d  │ │ 0│*1 │ a  │
        │ 1│ 2 │ b  │ │ 1│ 4 │ d  │ │ 1│*5 │ e  │         │ 1│ 5 │ e  │ │ 1│ 2 │ b  │
        │ 2│ 3 │ c  │ │ 2│ 5 │ e  │ │ 2│ 6 │ ff↩│         │ 2│*6 │ ff↩│ │ 2│ 3 │ c  │
        │ 3│ 4 │ d  │ │ 3│*6 │ ff↩│ │ 3│ 6 │↪ff │         │ 3│*6 │↪ff │ │ 3│ 4 │ d  │
        │ 4│ 5 │ e  │ │ 4│*6 │↪ff │ │ 4│ ~ │    │         │ 4│ ~ │    │ │ 4│ 5 │ e  │
        └──┴───┴────┘ └──┴───┴────┘ └──┴───┴────┘         └──┴───┴────┘ └──┴───┴────┘
        ");
    }

    #[test]
    fn test_move_focused_elem_to_top_and_bottom() {
        let mut viewer = init(b"a\nb1b2b3\nc1c2c3\nd1d2\n", 2, 5, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_focused_elem_to_bottom()],
                vec![move_cursor_down(2), move_focused_elem_to_bottom()],
                vec![move_cursor_down(1), move_focused_elem_to_bottom()],
                vec![set_scrolloff(1), move_focused_elem_to_bottom()],
            ],
        );
        assert_snapshot!(output, @r"
                      MoveFocusedElemToBottom MoveCursorDown(2)       MoveCursorDown(1)       SetScrolloff(1)
                                              MoveFocusedElemToBottom MoveFocusedElemToBottom MoveFocusedElemToBottom
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐
        │ 0│*1 │ a  │ │ 0│*1 │ a  │           │ 0│ 2 │↪b2↩│           │ 0│ 3 │ c1↩│           │ 0│ 3 │↪c2↩│
        │ 1│ 2 │ b1↩│ │ 1│ 2 │ b1↩│           │ 1│ 2 │↪b3 │           │ 1│ 3 │↪c2↩│           │ 1│ 3 │↪c3 │
        │ 2│ 2 │↪b2↩│ │ 2│ 2 │↪b2↩│           │ 2│*3 │ c1↩│           │ 2│ 3 │↪c3 │           │ 2│*4 │ d1↩│
        │ 3│ 2 │↪b3 │ │ 3│ 2 │↪b3 │           │ 3│*3 │↪c2↩│           │ 3│*4 │ d1↩│           │ 3│*4 │↪d2 │
        │ 4│ 3 │ c1↩│ │ 4│ 3 │ c1↩│           │ 4│*3 │↪c3 │           │ 4│*4 │↪d2 │           │ 4│ ~ │    │
        └──┴───┴────┘ └──┴───┴────┘           └──┴───┴────┘           └──┴───┴────┘           └──┴───┴────┘
        ");
        let output = run(
            &mut viewer,
            vec![
                vec![move_focused_elem_to_top()],
                vec![set_scrolloff(0), move_focused_elem_to_top()],
                vec![move_cursor_up(1), move_focused_elem_to_top()],
            ],
        );
        assert_snapshot!(output, @r"
                      MoveFocusedElemToTop SetScrolloff(0)      MoveCursorUp(1)
                                           MoveFocusedElemToTop MoveFocusedElemToTop
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐        ┌SI┬─L#┬────┐        ┌SI┬─L#┬────┐
        │ 0│ 3 │↪c2↩│ │ 0│ 3 │↪c3 │        │ 0│*4 │ d1↩│        │ 0│*3 │ c1↩│
        │ 1│ 3 │↪c3 │ │ 1│*4 │ d1↩│        │ 1│*4 │↪d2 │        │ 1│*3 │↪c2↩│
        │ 2│*4 │ d1↩│ │ 2│*4 │↪d2 │        │ 2│ ~ │    │        │ 2│*3 │↪c3 │
        │ 3│*4 │↪d2 │ │ 3│ ~ │    │        │ 3│ ~ │    │        │ 3│ 4 │ d1↩│
        │ 4│ ~ │    │ │ 4│ ~ │    │        │ 4│ ~ │    │        │ 4│ 4 │↪d2 │
        └──┴───┴────┘ └──┴───┴────┘        └──┴───┴────┘        └──┴───┴────┘
        ");
    }

    #[test]
    fn test_move_focused_elem_to_center() {
        let mut viewer = init(
            b"a\nb1b2b3\nc1c2c3\nd1d2d3d4\ne1e2e3e4e5e6e7\nf1f2f3f4f5f6f7f8\n",
            2,
            7,
            0,
        );
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(2), move_focused_elem_to_center()],
                vec![resize_height(8), move_focused_elem_to_center()],
                vec![move_cursor_down(1), move_focused_elem_to_center()],
                vec![resize_height(7), move_focused_elem_to_center()],
            ],
        );
        // Viewport Height | Cursor Height | Occupied Cursor Range | (VH - CH) / 2
        //         7       |       3       |    [--xxx--]  (2-4)   | (7 - 3) / 2 = 2
        //         7       |       4       |    [-xxxx--]  (1-4)   | (7 - 4) / 2 = 1
        //         8       |       3       |    [--xxx---] (2-4)   | (8 - 3) / 2 = 2
        //         8       |       4       |    [--xxxx--] (2-5)   | (8 - 4) / 2 = 2
        assert_snapshot!(output, @r"
                      MoveCursorDown(2)       ResizeHeight(8)         MoveCursorDown(1)       ResizeHeight(7)
                      MoveFocusedElemToCenter MoveFocusedElemToCenter MoveFocusedElemToCenter MoveFocusedElemToCenter
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐
        │ 0│*1 │ a  │ │ 0│ 2 │↪b2↩│           │ 0│ 2 │↪b2↩│           │ 0│ 3 │↪c2↩│           │ 0│ 3 │↪c3 │
        │ 1│ 2 │ b1↩│ │ 1│ 2 │↪b3 │           │ 1│ 2 │↪b3 │           │ 1│ 3 │↪c3 │           │ 1│*4 │ d1↩│
        │ 2│ 2 │↪b2↩│ │ 2│*3 │ c1↩│           │ 2│*3 │ c1↩│           │ 2│*4 │ d1↩│           │ 2│*4 │↪d2↩│
        │ 3│ 2 │↪b3 │ │ 3│*3 │↪c2↩│           │ 3│*3 │↪c2↩│           │ 3│*4 │↪d2↩│           │ 3│*4 │↪d3↩│
        │ 4│ 3 │ c1↩│ │ 4│*3 │↪c3 │           │ 4│*3 │↪c3 │           │ 4│*4 │↪d3↩│           │ 4│*4 │↪d4 │
        │ 5│ 3 │↪c2↩│ │ 5│ 4 │ d1↩│           │ 5│ 4 │ d1↩│           │ 5│*4 │↪d4 │           │ 5│ 5 │ e1↩│
        │ 6│ 3 │↪c3 │ │ 6│ 4 │↪d2↩│           │ 6│ 4 │↪d2↩│           │ 6│ 5 │ e1↩│           │ 6│ 5 │↪e2↩│
        └──┴───┴────┘ └──┴───┴────┘           │ 7│ 4 │↪d3↩│           │ 7│ 5 │↪e2↩│           └──┴───┴────┘
                                              └──┴───┴────┘           └──┴───┴────┘
        ");
        let output = run(
            &mut viewer,
            vec![
                vec![
                    resize_height(3),
                    move_cursor_down(1),
                    move_focused_elem_to_center(),
                ],
                vec![resize_height(4), move_focused_elem_to_center()],
                vec![move_cursor_down(1), move_focused_elem_to_center()],
                vec![resize_height(3), move_focused_elem_to_center()],
            ],
        );
        // Viewport Height | Cursor Height | Occupied Cursor Range | (VH - CH) / 2
        //         4       |       8       |  xx[xxxx]xx  (-2 - 5) | (4 - 8) / 2 = -2
        //         4       |       7       |  x[xxxx]xx   (-1 - 5) | (4 - 7) / 2 = -1.5
        //         3       |       8       |  xx[xxx]xxx  (-2 - 5) | (3 - 8) / 2 = -2.5
        //         3       |       7       |  xx[xxx]xx   (-2 - 4) | (3 - 7) / 2 = -2
        // 7 'e's, 8 'f's
        assert_snapshot!(output, @r"
                      ResizeHeight(3)         ResizeHeight(4)         MoveCursorDown(1)       ResizeHeight(3)
                      MoveCursorDown(1)       MoveFocusedElemToCenter MoveFocusedElemToCenter MoveFocusedElemToCenter
                      MoveFocusedElemToCenter
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐           ┌SI┬─L#┬────┐
        │ 0│ 3 │↪c3 │ │ 0│*5 │↪e3↩│           │ 0│*5 │↪e2↩│           │ 0│*6 │↪f3↩│           │ 0│*6 │↪f3↩│
        │ 1│*4 │ d1↩│ │ 1│*5 │↪e4↩│           │ 1│*5 │↪e3↩│           │ 1│*6 │↪f4↩│           │ 1│*6 │↪f4↩│
        │ 2│*4 │↪d2↩│ │ 2│*5 │↪e5↩│           │ 2│*5 │↪e4↩│           │ 2│*6 │↪f5↩│           │ 2│*6 │↪f5↩│
        │ 3│*4 │↪d3↩│ └──┴───┴────┘           │ 3│*5 │↪e5↩│           │ 3│*6 │↪f6↩│           └──┴───┴────┘
        │ 4│*4 │↪d4 │                         └──┴───┴────┘           └──┴───┴────┘
        │ 5│ 5 │ e1↩│
        │ 6│ 5 │↪e2↩│
        └──┴───┴────┘
        ");
    }

    #[test]
    fn tail_end_of_document_after_focus_bottom() {
        let mut viewer = init(b"a\nb\n", 3, 4, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![focus_bottom()],
                vec![append_document_data(b"c\n")],
                vec![append_document_data(b"d\ne\n")],
                vec![move_cursor_up(1), scroll_viewport_down(2)],
                vec![append_document_data(b"f\ne\n")],
            ],
        );
        assert_snapshot!(output, @r"
                       FocusBottom    AppendDocData  AppendDocData  MoveCursorUp(1)       AppendDocData
                                                                    ScrollViewportDown(2)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐        ┌SI┬─L#┬─────┐
        │ 0│*1 │ a   │ │ 0│ 1 │ a   │ │ 0│ 1 │ a   │ │ 0│ 2 │ b   │ │ 0│*4 │ d   │        │ 0│*4 │ d   │
        │ 1│ 2 │ b   │ │ 1│*2 │ b   │ │ 1│ 2 │ b   │ │ 1│ 3 │ c   │ │ 1│ 5 │ e   │        │ 1│ 5 │ e   │
        │ 2│ ~ │     │ │ 2│ ~ │     │ │ 2│*3 │ c   │ │ 2│ 4 │ d   │ │ 2│ ~ │     │        │ 2│ 6 │ f   │
        │ 3│ ~ │     │ │ 3│ ~ │     │ │ 3│ ~ │     │ │ 3│*5 │ e   │ │ 3│ ~ │     │        │ 3│ 7 │ e   │
        └──┴───┴─────┘ └──┴───┴─────┘ └──┴───┴─────┘ └──┴───┴─────┘ └──┴───┴─────┘        └──┴───┴─────┘
        ");
    }

    #[test]
    fn test_resize_width() {
        let text = b"a\n\
            b\n\
            c\n\
            d\n\
            1eeee2eeee3eeee4e!ee5eeee6eeee7eeee8eeee9eeee0eeee\n\
            f\n\
            g\n\
            h\n\
            i\n";
        let mut viewer = init(text, 5, 5, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(4)],
                vec![scroll_viewport_down(6)],
                // start AboveTopLine, end AtScreenIndex case, keep end of cursor in same spot
                vec![resize_width(25)],
                // start AtScreenIndex case, keep start of cursor in same spot
                vec![resize_width(5)],
            ],
        );
        assert_snapshot!(output, @r"
                         MoveCursorDown(4) ScrollViewportDown(6) ResizeWidth(25)                      ResizeWidth(5)
        ┌SI┬─L#┬───────┐ ┌SI┬─L#┬───────┐  ┌SI┬─L#┬───────┐      ┌SI┬─L#┬───────────────────────────┐ ┌SI┬─L#┬───────┐
        │ 0│*1 │ a     │ │ 0│*5 │ 1eeee↩│  │ 0│*5 │↪7eeee↩│      │ 0│ 3 │ c                         │ │ 0│ 3 │ c     │
        │ 1│ 2 │ b     │ │ 1│*5 │↪2eeee↩│  │ 1│*5 │↪8eeee↩│      │ 1│ 4 │ d                         │ │ 1│ 4 │ d     │
        │ 2│ 3 │ c     │ │ 2│*5 │↪3eeee↩│  │ 2│*5 │↪9eeee↩│      │ 2│*5 │ 1eeee2eeee3eeee4e!ee5eeee↩│ │ 2│*5 │ 1eeee↩│
        │ 3│ 4 │ d     │ │ 3│*5 │↪4e!ee↩│  │ 3│*5 │↪0eeee │      │ 3│*5 │↪6eeee7eeee8eeee9eeee0eeee │ │ 3│*5 │↪2eeee↩│
        │ 4│ 5 │ 1eeee↩│ │ 4│*5 │↪5eeee↩│  │ 4│ 6 │ f     │      │ 4│ 6 │ f                         │ │ 4│*5 │↪3eeee↩│
        └──┴───┴───────┘ └──┴───┴───────┘  └──┴───┴───────┘      └──┴───┴───────────────────────────┘ └──┴───┴───────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![scroll_viewport_down(3)],
                // Now we're showing chars 6-30 on screen, so char 18 out of 50 (the '!') is in the
                // middle, which is the 36th percentile. If we make the screen 2 chars wide, we
                // should still see that '!' in the middle of the screen.
                vec![resize_width(2)],
                vec![resize_width(3)],
                vec![resize_width(4)],
                vec![resize_width(5)],
                // Guess there's a slight off-by-one here, but seems fine.
                vec![resize_width(6)],
            ],
        );
        assert_snapshot!(output, @r"
                         ScrollViewportDown(3) ResizeWidth(2) ResizeWidth(3) ResizeWidth(4)  ResizeWidth(5)   ResizeWidth(6)
        ┌SI┬─L#┬───────┐ ┌SI┬─L#┬───────┐      ┌SI┬─L#┬────┐  ┌SI┬─L#┬─────┐ ┌SI┬─L#┬──────┐ ┌SI┬─L#┬───────┐ ┌SI┬─L#┬────────┐
        │ 0│ 3 │ c     │ │ 0│*5 │↪2eeee↩│      │ 0│*5 │↪ee↩│  │ 0│*5 │↪e3e↩│ │ 0│*5 │↪ee3e↩│ │ 0│*5 │↪2eeee↩│ │ 0│*5 │↪eeee3e↩│
        │ 1│ 4 │ d     │ │ 1│*5 │↪3eeee↩│      │ 1│*5 │↪e4↩│  │ 1│*5 │↪eee↩│ │ 1│*5 │↪eee4↩│ │ 1│*5 │↪3eeee↩│ │ 1│*5 │↪eee4e!↩│
        │ 2│*5 │ 1eeee↩│ │ 2│*5 │↪4e!ee↩│      │ 2│*5 │↪e!↩│  │ 2│*5 │↪4e!↩│ │ 2│*5 │↪e!ee↩│ │ 2│*5 │↪4e!ee↩│ │ 2│*5 │↪ee5eee↩│
        │ 3│*5 │↪2eeee↩│ │ 3│*5 │↪5eeee↩│      │ 3│*5 │↪ee↩│  │ 3│*5 │↪ee5↩│ │ 3│*5 │↪5eee↩│ │ 3│*5 │↪5eeee↩│ │ 3│*5 │↪e6eeee↩│
        │ 4│*5 │↪3eeee↩│ │ 4│*5 │↪6eeee↩│      │ 4│*5 │↪5e↩│  │ 4│*5 │↪eee↩│ │ 4│*5 │↪e6ee↩│ │ 4│*5 │↪6eeee↩│ │ 4│*5 │↪7eeee8↩│
        └──┴───┴───────┘ └──┴───┴───────┘      └──┴───┴────┘  └──┴───┴─────┘ └──┴───┴──────┘ └──┴───┴───────┘ └──┴───┴────────┘
        ");
    }

    #[test]
    fn test_resize_height() {
        let text = b"a\n\
            b\n\
            c\n\
            d\n\
            1ee2ee3ee4ee5ee6ee7ee8ee9ee0ee\n\
            f\n\
            g\n\
            h\n\
            i\n";
        let mut viewer = init(text, 3, 5, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(4)],
                vec![scroll_viewport_up(1)],
                vec![resize_height(10)],
            ],
        );

        assert_snapshot!(output, @r"
                       MoveCursorDown(4) ScrollViewportUp(1) ResizeHeight(10)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐    ┌SI┬─L#┬─────┐      ┌SI┬─L#┬─────┐
        │ 0│*1 │ a   │ │ 0│*5 │ 1ee↩│    │ 0│ 4 │ d   │      │ 0│ 3 │ c   │
        │ 1│ 2 │ b   │ │ 1│*5 │↪2ee↩│    │ 1│*5 │ 1ee↩│      │ 1│ 4 │ d   │
        │ 2│ 3 │ c   │ │ 2│*5 │↪3ee↩│    │ 2│*5 │↪2ee↩│      │ 2│*5 │ 1ee↩│
        │ 3│ 4 │ d   │ │ 3│*5 │↪4ee↩│    │ 3│*5 │↪3ee↩│      │ 3│*5 │↪2ee↩│
        │ 4│ 5 │ 1ee↩│ │ 4│*5 │↪5ee↩│    │ 4│*5 │↪4ee↩│      │ 4│*5 │↪3ee↩│
        └──┴───┴─────┘ └──┴───┴─────┘    └──┴───┴─────┘      │ 5│*5 │↪4ee↩│
                                                             │ 6│*5 │↪5ee↩│
                                                             │ 7│*5 │↪6ee↩│
                                                             │ 8│*5 │↪7ee↩│
                                                             │ 9│*5 │↪8ee↩│
                                                             └──┴───┴─────┘
        ");

        let mut viewer = init(text, 3, 5, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(4)],
                vec![scroll_viewport_down(8)],
                // Roughly what we're aiming for; unclear why it's not four rows of line 5 after he
                // resize.
                vec![resize_height(10)],
            ],
        );
        assert_snapshot!(output, @r"
                       MoveCursorDown(4) ScrollViewportDown(8) ResizeHeight(10)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐    ┌SI┬─L#┬─────┐        ┌SI┬─L#┬─────┐
        │ 0│*1 │ a   │ │ 0│*5 │ 1ee↩│    │ 0│*5 │↪9ee↩│        │ 0│*5 │↪8ee↩│
        │ 1│ 2 │ b   │ │ 1│*5 │↪2ee↩│    │ 1│*5 │↪0ee │        │ 1│*5 │↪9ee↩│
        │ 2│ 3 │ c   │ │ 2│*5 │↪3ee↩│    │ 2│ 6 │ f   │        │ 2│*5 │↪0ee │
        │ 3│ 4 │ d   │ │ 3│*5 │↪4ee↩│    │ 3│ 7 │ g   │        │ 3│ 6 │ f   │
        │ 4│ 5 │ 1ee↩│ │ 4│*5 │↪5ee↩│    │ 4│ 8 │ h   │        │ 4│ 7 │ g   │
        └──┴───┴─────┘ └──┴───┴─────┘    └──┴───┴─────┘        │ 5│ 8 │ h   │
                                                               │ 6│ 9 │ i   │
                                                               │ 7│ ~ │     │
                                                               │ 8│ ~ │     │
                                                               │ 9│ ~ │     │
                                                               └──┴───┴─────┘
        ");

        let mut viewer = init(text, 3, 5, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(4)],
                vec![scroll_viewport_down(1)],
                vec![resize_height(3)],
            ],
        );
        assert_snapshot!(output, @r"
                       MoveCursorDown(4) ScrollViewportDown(1) ResizeHeight(3)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐    ┌SI┬─L#┬─────┐        ┌SI┬─L#┬─────┐
        │ 0│*1 │ a   │ │ 0│*5 │ 1ee↩│    │ 0│*5 │↪2ee↩│        │ 0│*5 │↪3ee↩│
        │ 1│ 2 │ b   │ │ 1│*5 │↪2ee↩│    │ 1│*5 │↪3ee↩│        │ 1│*5 │↪4ee↩│
        │ 2│ 3 │ c   │ │ 2│*5 │↪3ee↩│    │ 2│*5 │↪4ee↩│        │ 2│*5 │↪5ee↩│
        │ 3│ 4 │ d   │ │ 3│*5 │↪4ee↩│    │ 3│*5 │↪5ee↩│        └──┴───┴─────┘
        │ 4│ 5 │ 1ee↩│ │ 4│*5 │↪5ee↩│    │ 4│*5 │↪6ee↩│
        └──┴───┴─────┘ └──┴───┴─────┘    └──┴───┴─────┘
        ");

        let mut viewer = init(text, 10, 5, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(4)],
                vec![scroll_viewport_down(1)],
                vec![resize_height(7)],
            ],
        );
        assert_snapshot!(output, @r"
                              MoveCursorDown(4)     ScrollViewportDown(1) ResizeHeight(7)
        ┌SI┬─L#┬────────────┐ ┌SI┬─L#┬────────────┐ ┌SI┬─L#┬────────────┐ ┌SI┬─L#┬────────────┐
        │ 0│*1 │ a          │ │ 0│ 3 │ c          │ │ 0│ 4 │ d          │ │ 0│ 3 │ c          │
        │ 1│ 2 │ b          │ │ 1│ 4 │ d          │ │ 1│*5 │ 1ee2ee3ee4↩│ │ 1│ 4 │ d          │
        │ 2│ 3 │ c          │ │ 2│*5 │ 1ee2ee3ee4↩│ │ 2│*5 │↪ee5ee6ee7e↩│ │ 2│*5 │ 1ee2ee3ee4↩│
        │ 3│ 4 │ d          │ │ 3│*5 │↪ee5ee6ee7e↩│ │ 3│*5 │↪e8ee9ee0ee │ │ 3│*5 │↪ee5ee6ee7e↩│
        │ 4│ 5 │ 1ee2ee3ee4↩│ │ 4│*5 │↪e8ee9ee0ee │ │ 4│ 6 │ f          │ │ 4│*5 │↪e8ee9ee0ee │
        └──┴───┴────────────┘ └──┴───┴────────────┘ └──┴───┴────────────┘ │ 5│ 6 │ f          │
                                                                          │ 6│ 7 │ g          │
                                                                          └──┴───┴────────────┘
        ");
    }

    #[test]
    fn test_resize() {
        let text = b"\
            01\n02\n03\n04\n55\n06\n07\n08\n09\n10\n\
            11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n";
        let mut viewer = init(text, 3, 15, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(13)],
                vec![resize(Dimensions {
                    width: 3,
                    height: 4,
                })],
            ],
        );
        assert_snapshot!(output, @r"
                       MoveCursorDown(13) Resize({ width: 3, height: 4 })
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐     ┌SI┬─L#┬─────┐
        │ 0│*1 │ 01  │ │ 0│ 1 │ 01  │     │ 0│ 11│ 11  │
        │ 1│ 2 │ 02  │ │ 1│ 2 │ 02  │     │ 1│ 12│ 12  │
        │ 2│ 3 │ 03  │ │ 2│ 3 │ 03  │     │ 2│ 13│ 13  │
        │ 3│ 4 │ 04  │ │ 3│ 4 │ 04  │     │ 3│*14│ 14  │
        │ 4│ 5 │ 55  │ │ 4│ 5 │ 55  │     └──┴───┴─────┘
        │ 5│ 6 │ 06  │ │ 5│ 6 │ 06  │
        │ 6│ 7 │ 07  │ │ 6│ 7 │ 07  │
        │ 7│ 8 │ 08  │ │ 7│ 8 │ 08  │
        │ 8│ 9 │ 09  │ │ 8│ 9 │ 09  │
        │ 9│ 10│ 10  │ │ 9│ 10│ 10  │
        │10│ 11│ 11  │ │10│ 11│ 11  │
        │11│ 12│ 12  │ │11│ 12│ 12  │
        │12│ 13│ 13  │ │12│ 13│ 13  │
        │13│ 14│ 14  │ │13│*14│ 14  │
        │14│ 15│ 15  │ │14│ 15│ 15  │
        └──┴───┴─────┘ └──┴───┴─────┘
        ");

        // Same as above, but now with scrolloff = 1; the new cursor position obeys scrolloff.
        let mut viewer = init(text, 3, 15, 1);
        let output = run(
            &mut viewer,
            vec![vec![
                move_cursor_down(13),
                resize(Dimensions {
                    width: 3,
                    height: 4,
                }),
            ]],
        );
        assert_snapshot!(output, @r"
                       MoveCursorDown(13)
                       Resize({ width: 3, height: 4 })
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐
        │ 0│*1 │ 01  │ │ 0│ 12│ 12  │
        │ 1│ 2 │ 02  │ │ 1│ 13│ 13  │
        │ 2│ 3 │ 03  │ │ 2│*14│ 14  │
        │ 3│ 4 │ 04  │ │ 3│ 15│ 15  │
        │ 4│ 5 │ 55  │ └──┴───┴─────┘
        │ 5│ 6 │ 06  │
        │ 6│ 7 │ 07  │
        │ 7│ 8 │ 08  │
        │ 8│ 9 │ 09  │
        │ 9│ 10│ 10  │
        │10│ 11│ 11  │
        │11│ 12│ 12  │
        │12│ 13│ 13  │
        │13│ 14│ 14  │
        │14│ 15│ 15  │
        └──┴───┴─────┘
        ");

        let text = b"a\nb\nc\nd\nxxxxxxxxxxxxxxxxxxxx\ne\nf\ng\n";
        let mut viewer = init(text, 3, 5, 2);
        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(3), scroll_viewport_down(2)],
                // Anchor point is second line, but snaps into viewport.
                vec![resize(Dimensions {
                    width: 30,
                    height: 5,
                })],
            ],
        );
        assert_snapshot!(output, @r"
                       MoveCursorDown(3)     Resize({ width: 30, height: 5 })
                       ScrollViewportDown(2)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐        ┌SI┬─L#┬────────────────────────────────┐
        │ 0│*1 │ a   │ │ 0│ 4 │ d   │        │ 0│ 3 │ c                              │
        │ 1│ 2 │ b   │ │ 1│*5 │ xxx↩│        │ 1│ 4 │ d                              │
        │ 2│ 3 │ c   │ │ 2│*5 │↪xxx↩│        │ 2│*5 │ xxxxxxxxxxxxxxxxxxxx           │
        │ 3│ 4 │ d   │ │ 3│*5 │↪xxx↩│        │ 3│ 6 │ e                              │
        │ 4│ 5 │ xxx↩│ │ 4│*5 │↪xxx↩│        │ 4│ 7 │ f                              │
        └──┴───┴─────┘ └──┴───┴─────┘        └──┴───┴────────────────────────────────┘
        ");
    }

    #[test]
    fn test_basic_search() {
        let text = br"1
2a
3a
4
5
6b
7aa
8
9b
0";
        let mut viewer = init(text, 3, 4, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![
                    initialize_search("a", SearchDirection::Forward),
                    jump_to_next_match(1),
                ],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(2)],
                vec![jump_to_prev_match(3)],
            ],
        );
        assert_snapshot!(output, @r"
                       /a                         JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 2) JumpToSearchMatch(Prev, 3)
                       JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐             ┌SI┬─L#┬─────┐             ┌SI┬─L#┬─────┐             ┌SI┬─L#┬─────┐
        │ 0│*1 │ 1   │ │ 0│ 1 │ 1   │             │ 0│ 1 │ 1   │             │ 0│ 4 │ 4   │             │ 0│*2 │ 2a  │
        │ 1│ 2 │ 2a  │ │ 1│*2 │ 2a  │             │ 1│ 2 │ 2a  │             │ 1│ 5 │ 5   │             │ 1│ 3 │ 3a  │
        │ 2│ 3 │ 3a  │ │ 2│ 3 │ 3a  │             │ 2│*3 │ 3a  │             │ 2│ 6 │ 6b  │             │ 2│ 4 │ 4   │
        │ 3│ 4 │ 4   │ │ 3│ 4 │ 4   │             │ 3│ 4 │ 4   │             │ 3│*7 │ 7aa │             │ 3│ 5 │ 5   │
        └──┴───┴─────┘ └──┴───┴─────┘             └──┴───┴─────┘             └──┴───┴─────┘             └──┴───┴─────┘
        ");

        // Moving the cursor resets the search (we don't jump to line 7).
        let output = run(
            &mut viewer,
            vec![
                vec![jump_to_next_match(1)],
                vec![focus_top(), jump_to_next_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                       JumpToSearchMatch(Next, 1) FocusTop
                                                  JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐             ┌SI┬─L#┬─────┐
        │ 0│*2 │ 2a  │ │ 0│ 2 │ 2a  │             │ 0│ 1 │ 1   │
        │ 1│ 3 │ 3a  │ │ 1│*3 │ 3a  │             │ 1│*2 │ 2a  │
        │ 2│ 4 │ 4   │ │ 2│ 4 │ 4   │             │ 2│ 3 │ 3a  │
        │ 3│ 5 │ 5   │ │ 3│ 5 │ 5   │             │ 3│ 4 │ 4   │
        └──┴───┴─────┘ └──┴───┴─────┘             └──┴───┴─────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![
                    initialize_search("b", SearchDirection::Reverse),
                    jump_to_next_match(1),
                ],
                vec![jump_to_next_match(1)],
                vec![jump_to_prev_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                       ?b                         JumpToSearchMatch(Next, 1) JumpToSearchMatch(Prev, 1)
                       JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬─────┐ ┌SI┬─L#┬─────┐             ┌SI┬─L#┬─────┐             ┌SI┬─L#┬─────┐
        │ 0│ 1 │ 1   │ │ 0│ 6 │ 6b  │             │ 0│*6 │ 6b  │             │ 0│ 6 │ 6b  │
        │ 1│*2 │ 2a  │ │ 1│ 7 │ 7aa │             │ 1│ 7 │ 7aa │             │ 1│ 7 │ 7aa │
        │ 2│ 3 │ 3a  │ │ 2│ 8 │ 8   │             │ 2│ 8 │ 8   │             │ 2│ 8 │ 8   │
        │ 3│ 4 │ 4   │ │ 3│*9 │ 9b  │             │ 3│ 9 │ 9b  │             │ 3│*9 │ 9b  │
        └──┴───┴─────┘ └──┴───┴─────┘             └──┴───┴─────┘             └──┴───┴─────┘
        ");
    }

    #[test]
    fn test_jump_to_search_matches_in_very_long_line() {
        let text = b"a\nb\nc\nd\ne\n1   2H H3   4   5   6   7   8 H 9   10  11H 12  12  14H 15  \nv\nw\nx\ny\nz\n";
        let mut viewer = init(text, 4, 5, 1);
        let output = run(
            &mut viewer,
            vec![
                vec![
                    initialize_search("H", SearchDirection::Forward),
                    jump_to_next_match(1),
                ],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                        /H                         JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1)
                        JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐            ┌SI┬─L#┬──────┐            ┌SI┬─L#┬──────┐            ┌SI┬─L#┬──────┐            ┌SI┬─L#┬──────┐
        │ 0│*1 │ a    │ │ 0│ 4 │ d    │            │ 0│ 4 │ d    │            │ 0│*6 │↪5   ↩│            │ 0│*6 │↪8 H ↩│            │ 0│*6 │↪11H ↩│
        │ 1│ 2 │ b    │ │ 1│ 5 │ e    │            │ 1│ 5 │ e    │            │ 1│*6 │↪6   ↩│            │ 1│*6 │↪9   ↩│            │ 1│*6 │↪12  ↩│
        │ 2│ 3 │ c    │ │ 2│*6 │ 1   ↩│            │ 2│*6 │ 1   ↩│            │ 2│*6 │↪7   ↩│            │ 2│*6 │↪10  ↩│            │ 2│*6 │↪12  ↩│
        │ 3│ 4 │ d    │ │ 3│*6 │↪2H H↩│            │ 3│*6 │↪2H H↩│            │ 3│*6 │↪8 H ↩│            │ 3│*6 │↪11H ↩│            │ 3│*6 │↪14H ↩│
        │ 4│ 5 │ e    │ │ 4│*6 │↪3   ↩│            │ 4│*6 │↪3   ↩│            │ 4│*6 │↪9   ↩│            │ 4│*6 │↪12  ↩│            │ 4│*6 │↪15   │
        └──┴───┴──────┘ └──┴───┴──────┘            └──┴───┴──────┘            └──┴───┴──────┘            └──┴───┴──────┘            └──┴───┴──────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(2)],
                vec![jump_to_prev_match(2)],
                vec![jump_to_next_match(2)],
            ],
        );
        assert_snapshot!(output, @r"
                        MoveCursorDown(2) JumpToSearchMatch(Prev, 2) JumpToSearchMatch(Next, 2)
        ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐   ┌SI┬─L#┬──────┐            ┌SI┬─L#┬──────┐
        │ 0│*6 │↪11H ↩│ │ 0│ 6 │↪14H ↩│   │ 0│*6 │↪10  ↩│            │ 0│*6 │ 1   ↩│
        │ 1│*6 │↪12  ↩│ │ 1│ 6 │↪15   │   │ 1│*6 │↪11H ↩│            │ 1│*6 │↪2H H↩│
        │ 2│*6 │↪12  ↩│ │ 2│ 7 │ v    │   │ 2│*6 │↪12  ↩│            │ 2│*6 │↪3   ↩│
        │ 3│*6 │↪14H ↩│ │ 3│*8 │ w    │   │ 3│*6 │↪12  ↩│            │ 3│*6 │↪4   ↩│
        │ 4│*6 │↪15   │ │ 4│ 9 │ x    │   │ 4│*6 │↪14H ↩│            │ 4│*6 │↪5   ↩│
        └──┴───┴──────┘ └──┴───┴──────┘   └──┴───┴──────┘            └──┴───┴──────┘
        ");
    }

    #[test]
    fn test_scroll_to_search_matches() {
        let text = b"a\nb\nc\nd H\ne\n1   2 H 3   4   5   6 H 7   8   9   10  11H\n";
        let mut viewer = init(text, 4, 5, 2);
        viewer.move_cursor_down(1);
        let output = run(
            &mut viewer,
            vec![
                vec![
                    initialize_search("H", SearchDirection::Forward),
                    scroll_to_next_match(1),
                ],
                vec![scroll_to_next_match(2)],
                vec![scroll_to_prev_match(2)],
                vec![scroll_to_prev_match(1)],
            ],
        );

        // Note that we violate scrolloff when jumping to the first match. This is intentional, to
        // support scrolling when starting on one of the first few lines of the file.
        assert_snapshot!(output, @r"
                        /H                           ScrollToSearchMatch(Next, 2) ScrollToSearchMatch(Prev, 2) ScrollToSearchMatch(Prev, 1)
                        ScrollToSearchMatch(Next, 1)
        ┌SI┬─L#┬──────┐ ┌SI┬─L#┬──────┐              ┌SI┬─L#┬──────┐              ┌SI┬─L#┬──────┐              ┌SI┬─L#┬──────┐
        │ 0│ 1 │ a    │ │ 0│ 3 │ c    │              │ 0│*6 │↪5   ↩│              │ 0│ 3 │ c    │              │ 0│*6 │↪10  ↩│
        │ 1│*2 │ b    │ │ 1│*4 │ d H  │              │ 1│*6 │↪6 H ↩│              │ 1│*4 │ d H  │              │ 1│*6 │↪11H  │
        │ 2│ 3 │ c    │ │ 2│ 5 │ e    │              │ 2│*6 │↪7   ↩│              │ 2│ 5 │ e    │              │ 2│ ~ │      │
        │ 3│ 4 │ d H  │ │ 3│ 6 │ 1   ↩│              │ 3│*6 │↪8   ↩│              │ 3│ 6 │ 1   ↩│              │ 3│ ~ │      │
        │ 4│ 5 │ e    │ │ 4│ 6 │↪2 H ↩│              │ 4│*6 │↪9   ↩│              │ 4│ 6 │↪2 H ↩│              │ 4│ ~ │      │
        └──┴───┴──────┘ └──┴───┴──────┘              └──┴───┴──────┘              └──┴───┴──────┘              └──┴───┴──────┘
        ");
    }

    #[test]
    fn test_search_finds_additional_matches_after_reading_more_data() {
        let text = br"1
2a
3a
4
";
        let mut viewer = init(text, 2, 5, 0);
        let output = run(
            &mut viewer,
            vec![
                vec![
                    initialize_search("a", SearchDirection::Forward),
                    jump_to_next_match(2),
                ],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                      /a                         JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1)
                      JumpToSearchMatch(Next, 2)
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐              ┌SI┬─L#┬────┐              ┌SI┬─L#┬────┐
        │ 0│*1 │ 1  │ │ 0│ 1 │ 1  │              │ 0│ 1 │ 1  │              │ 0│ 1 │ 1  │
        │ 1│ 2 │ 2a │ │ 1│ 2 │ 2a │              │ 1│*2 │ 2a │              │ 1│ 2 │ 2a │
        │ 2│ 3 │ 3a │ │ 2│*3 │ 3a │              │ 2│ 3 │ 3a │              │ 2│*3 │ 3a │
        │ 3│ 4 │ 4  │ │ 3│ 4 │ 4  │              │ 3│ 4 │ 4  │              │ 3│ 4 │ 4  │
        │ 4│ ~ │    │ │ 4│ ~ │    │              │ 4│ ~ │    │              │ 4│ ~ │    │
        └──┴───┴────┘ └──┴───┴────┘              └──┴───┴────┘              └──┴───┴────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![append_document_data(b"5a\n6a\n")],
                vec![jump_to_next_match(2)],
                vec![jump_to_next_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                      AppendDocData JumpToSearchMatch(Next, 2) JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐ ┌SI┬─L#┬────┐              ┌SI┬─L#┬────┐
        │ 0│ 1 │ 1  │ │ 0│ 1 │ 1  │ │ 0│ 2 │ 2a │              │ 0│*2 │ 2a │
        │ 1│ 2 │ 2a │ │ 1│ 2 │ 2a │ │ 1│ 3 │ 3a │              │ 1│ 3 │ 3a │
        │ 2│*3 │ 3a │ │ 2│*3 │ 3a │ │ 2│ 4 │ 4  │              │ 2│ 4 │ 4  │
        │ 3│ 4 │ 4  │ │ 3│ 4 │ 4  │ │ 3│ 5 │ 5a │              │ 3│ 5 │ 5a │
        │ 4│ ~ │    │ │ 4│ 5 │ 5a │ │ 4│*6 │ 6a │              │ 4│ 6 │ 6a │
        └──┴───┴────┘ └──┴───┴────┘ └──┴───┴────┘              └──┴───┴────┘
        ");
    }

    #[test]
    fn test_search_into_collapsed_containers() {
        let text = b"((k1 a)(k2 b))((k3 a)(k4 b)(k5 b))((k6 b)(k7 b))(a)(b)(c)";
        let mut viewer = init_sexp(text, 8, 5, 0);

        let output = run(
            &mut viewer,
            vec![
                vec![
                    initialize_search("b", SearchDirection::Forward),
                    jump_to_next_match(1),
                ],
                vec![jump_to_next_match(1)],
                vec![press_left(), press_left(), move_cursor_up(1)],
            ],
        );
        assert_snapshot!(output, @r"
                            /b                         JumpToSearchMatch(Next, 1) CollapseOrMoveCursorLeftOrUp
                            JumpToSearchMatch(Next, 1)                            CollapseOrMoveCursorLeftOrUp
                                                                                  MoveCursorUp(1)
        ┌SI┬─L#┬──────────┐ ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐
        │ 0│*1 │ [(k1 a)  │ │ 0│ 1 │ ((k1 a)  │        │ 0│ 1 │ ((k1 a)  │        │ 0│ 1 │ ((k1 a)  │
        │ 1│ 2 │  (k2 b)) │ │ 1│*2 │  [k2 b)) │        │ 1│ 2 │  (k2 b)) │        │ 1│*2 │  [k2 b)) │
        │ 2│ 2 │ ((k3 a)  │ │ 2│ 2 │ ((k3 a)  │        │ 2│ 2 │ ((k3 a)  │        │ 2│ 2 │ (…)      │
        │ 3│ 2 │  (k4 b)  │ │ 3│ 2 │  (k4 b)  │        │ 3│*2 │  [k4 b)  │        │ 3│ 2 │ ((k6 b)  │
        │ 4│ 2 │  (k5 b)) │ │ 4│ 2 │  (k5 b)) │        │ 4│ 2 │  (k5 b)) │        │ 4│ 2 │  (k7 b)) │
        └──┴───┴──────────┘ └──┴───┴──────────┘        └──┴───┴──────────┘        └──┴───┴──────────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
                vec![press_left(), press_left(), focus_top()],
            ],
        );
        assert_snapshot!(output, @r"
                            JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1) CollapseOrMoveCursorLeftOrUp
                                                                                                             CollapseOrMoveCursorLeftOrUp
                                                                                                             FocusTop
        ┌SI┬─L#┬──────────┐ ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐
        │ 0│ 1 │ ((k1 a)  │ │ 0│ 1 │ ((k1 a)  │        │ 0│ 1 │ ((k1 a)  │        │ 0│ 1 │ ((k1 a)  │        │ 0│*1 │ [(k1 a)  │
        │ 1│*2 │  [k2 b)) │ │ 1│*2 │  [k2 b)) │        │ 1│ 2 │  (k2 b)) │        │ 1│ 2 │  (k2 b)) │        │ 1│ 2 │  (k2 b)) │
        │ 2│ 2 │ (…)      │ │ 2│ 2 │ (…)      │        │ 2│*2 │ […)      │        │ 2│ 2 │ (…)      │        │ 2│ 2 │ (…)      │
        │ 3│ 2 │ ((k6 b)  │ │ 3│ 2 │ ((k6 b)  │        │ 3│ 2 │ ((k6 b)  │        │ 3│*2 │ ([k6 b)  │        │ 3│ 2 │ (…)      │
        │ 4│ 2 │  (k7 b)) │ │ 4│ 2 │  (k7 b)) │        │ 4│ 2 │  (k7 b)) │        │ 4│ 2 │  (k7 b)) │        │ 4│ 2 │ (a)      │
        └──┴───┴──────────┘ └──┴───┴──────────┘        └──┴───┴──────────┘        └──┴───┴──────────┘        └──┴───┴──────────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
                vec![jump_to_next_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                            JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬──────────┐ ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐
        │ 0│*1 │ [(k1 a)  │ │ 0│ 1 │ ((k1 a)  │        │ 0│ 1 │ ((k1 a)  │        │ 0│ 1 │ ((k1 a)  │        │ 0│ 2 │  (k2 b)) │
        │ 1│ 2 │  (k2 b)) │ │ 1│*2 │  [k2 b)) │        │ 1│ 2 │  (k2 b)) │        │ 1│ 2 │  (k2 b)) │        │ 1│ 2 │ (…)      │
        │ 2│ 2 │ (…)      │ │ 2│ 2 │ (…)      │        │ 2│*2 │ […)      │        │ 2│ 2 │ (…)      │        │ 2│ 2 │ (…)      │
        │ 3│ 2 │ (…)      │ │ 3│ 2 │ (…)      │        │ 3│ 2 │ (…)      │        │ 3│*2 │ […)      │        │ 3│ 2 │ (a)      │
        │ 4│ 2 │ (a)      │ │ 4│ 2 │ (a)      │        │ 4│ 2 │ (a)      │        │ 4│ 2 │ (a)      │        │ 4│*2 │ (*)      │
        └──┴───┴──────────┘ └──┴───┴──────────┘        └──┴───┴──────────┘        └──┴───┴──────────┘        └──┴───┴──────────┘
        ");

        let output = run(
            &mut viewer,
            vec![
                vec![focus_bottom()],
                vec![jump_to_prev_match(1)],
                vec![jump_to_prev_match(1)],
                vec![jump_to_prev_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                            FocusBottom         JumpToSearchMatch(Prev, 1) JumpToSearchMatch(Prev, 1) JumpToSearchMatch(Prev, 1)
        ┌SI┬─L#┬──────────┐ ┌SI┬─L#┬──────────┐ ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐
        │ 0│ 2 │  (k2 b)) │ │ 0│ 2 │ (…)      │ │ 0│ 2 │ (…)      │        │ 0│ 2 │ (…)      │        │ 0│*2 │ […)      │
        │ 1│ 2 │ (…)      │ │ 1│ 2 │ (…)      │ │ 1│ 2 │ (…)      │        │ 1│*2 │ […)      │        │ 1│ 2 │ (…)      │
        │ 2│ 2 │ (…)      │ │ 2│ 2 │ (a)      │ │ 2│ 2 │ (a)      │        │ 2│ 2 │ (a)      │        │ 2│ 2 │ (a)      │
        │ 3│ 2 │ (a)      │ │ 3│ 2 │ (b)      │ │ 3│*2 │ (*)      │        │ 3│ 2 │ (b)      │        │ 3│ 2 │ (b)      │
        │ 4│*2 │ (*)      │ │ 4│*2 │ [c)      │ │ 4│ 2 │ (c)      │        │ 4│ 2 │ (c)      │        │ 4│ 2 │ (c)      │
        └──┴───┴──────────┘ └──┴───┴──────────┘ └──┴───┴──────────┘        └──┴───┴──────────┘        └──┴───┴──────────┘
        ");

        // Technically we previously jumped to the k5 'b', so when we expand the container, the cursor
        // is now very disconnected from that last jump, so we want to clear it.
        let output = run(
            &mut viewer,
            vec![vec![press_right()], vec![jump_to_prev_match(1)]],
        );
        assert_snapshot!(output, @r"
                            ExpandOrMoveCursorRightOrDown JumpToSearchMatch(Prev, 1)
        ┌SI┬─L#┬──────────┐ ┌SI┬─L#┬──────────┐           ┌SI┬─L#┬──────────┐
        │ 0│*2 │ […)      │ │ 0│*2 │ [(k3 a)  │           │ 0│*2 │  [k2 b)) │
        │ 1│ 2 │ (…)      │ │ 1│ 2 │  (k4 b)  │           │ 1│ 2 │ ((k3 a)  │
        │ 2│ 2 │ (a)      │ │ 2│ 2 │  (k5 b)) │           │ 2│ 2 │  (k4 b)  │
        │ 3│ 2 │ (b)      │ │ 3│ 2 │ (…)      │           │ 3│ 2 │  (k5 b)) │
        │ 4│ 2 │ (c)      │ │ 4│ 2 │ (a)      │           │ 4│ 2 │ (…)      │
        └──┴───┴──────────┘ └──┴───┴──────────┘           └──┴───┴──────────┘
        ");

        // Reset the doc a bit before we test a similar thing going forward
        viewer.do_action(Action::MoveCursorDown(1));
        viewer.do_action(Action::CollapseOrMoveCursorLeftOrUp);
        viewer.do_action(Action::MoveCursorUp(1));

        let output = run(
            &mut viewer,
            vec![
                vec![jump_to_next_match(1)], // First jump goes to the 'b' on the same line.
                vec![jump_to_next_match(1)],
                vec![press_right()],
                vec![jump_to_next_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                            JumpToSearchMatch(Next, 1) JumpToSearchMatch(Next, 1) ExpandOrMoveCursorRightOrDown JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬──────────┐ ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐        ┌SI┬─L#┬──────────┐           ┌SI┬─L#┬──────────┐
        │ 0│*2 │  [k2 b)) │ │ 0│*2 │  [k2 b)) │        │ 0│ 2 │  (k2 b)) │        │ 0│ 2 │  (k2 b)) │           │ 0│ 2 │  (k2 b)) │
        │ 1│ 2 │ (…)      │ │ 1│ 2 │ (…)      │        │ 1│*2 │ […)      │        │ 1│*2 │ [(k3 a)  │           │ 1│ 2 │ ((k3 a)  │
        │ 2│ 2 │ (…)      │ │ 2│ 2 │ (…)      │        │ 2│ 2 │ (…)      │        │ 2│ 2 │  (k4 b)  │           │ 2│*2 │  [k4 b)  │
        │ 3│ 2 │ (a)      │ │ 3│ 2 │ (a)      │        │ 3│ 2 │ (a)      │        │ 3│ 2 │  (k5 b)) │           │ 3│ 2 │  (k5 b)) │
        │ 4│ 2 │ (b)      │ │ 4│ 2 │ (b)      │        │ 4│ 2 │ (b)      │        │ 4│ 2 │ (…)      │           │ 4│ 2 │ (…)      │
        └──┴───┴──────────┘ └──┴───┴──────────┘        └──┴───┴──────────┘        └──┴───┴──────────┘           └──┴───┴──────────┘
        ");
    }

    #[test]
    fn test_starting_search_from_line_with_variant_record_value_on_it() {
        let text = b"((k1 a)(k2 (V b)))(V c)";
        let mut viewer = init_sexp(text, 15, 5, 0);

        let output = run(
            &mut viewer,
            vec![
                vec![move_cursor_down(1), press_left()],
                vec![
                    initialize_search("v", SearchDirection::Forward),
                    jump_to_next_match(1),
                ],
                vec![jump_to_next_match(1)],
            ],
        );
        assert_snapshot!(output, @r"
                                   MoveCursorDown(1)            /v                         JumpToSearchMatch(Next, 1)
                                   CollapseOrMoveCursorLeftOrUp JumpToSearchMatch(Next, 1)
        ┌SI┬─L#┬─────────────────┐ ┌SI┬─L#┬─────────────────┐   ┌SI┬─L#┬─────────────────┐ ┌SI┬─L#┬─────────────────┐
        │ 0│*1 │ [(k1 a)         │ │ 0│ 1 │ ((k1 a)         │   │ 0│ 1 │ ((k1 a)         │ │ 0│ 1 │ ((k1 a)         │
        │ 1│ 2 │  (k2 (V         │ │ 1│*2 │  [k2 (…)))      │   │ 1│*2 │  [k2 (…)))      │ │ 1│ 2 │  (k2 (…)))      │
        │ 2│ 2 │    b)))         │ │ 2│ 2 │ (V              │   │ 2│ 2 │ (V              │ │ 2│*2 │ [V              │
        │ 3│ 2 │ (V              │ │ 3│ 2 │   c)            │   │ 3│ 2 │   c)            │ │ 3│ 2 │   c)            │
        │ 4│ 2 │   c)            │ │ 4│ ~ │                 │   │ 4│ ~ │                 │ │ 4│ ~ │                 │
        └──┴───┴─────────────────┘ └──┴───┴─────────────────┘   └──┴───┴─────────────────┘ └──┴───┴─────────────────┘
        ");
    }
}
