use std::cmp;
use std::num::NonZeroUsize;
use std::ops::RangeInclusive;

use crate::action::Action;
use crate::dimensions::Dimensions;
use crate::document::{CursorRange, Document};

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
}

#[derive(Debug)]
struct AcceptableStartScreenIndexesToShowCursorNode {
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    cursor_height: usize,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    last_screen_index: usize,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    range_after_considering_scrolloff: RangeInclusive<usize>,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    range_after_considering_start_and_end_of_document: RangeInclusive<usize>,
    #[allow(dead_code)]
    #[cfg(debug_assertions)]
    range_after_expanding_due_to_cursor_height: RangeInclusive<usize>,
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
enum PositionOfCursorInViewport {
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
            tailing_end_of_document: true,
            jump_distance: None,
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
        cmp::min(self.scrolloff_setting, (self.dimensions.height - 1) / 2)
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

    fn position_of_cursor_in_viewport(
        &self,
        cursor_range: &CursorRange<D::ScreenLine>,
    ) -> PositionOfCursorInViewport {
        let start_position = self.position_of_screen_line(&cursor_range.start);
        let end_position = self.position_of_screen_line(&cursor_range.end);

        match (start_position, end_position) {
            (PositionOfScreenLine::AboveTopLine, PositionOfScreenLine::AboveTopLine) => {
                panic!("entirety of cursor is above the viewport");
            }
            (
                PositionOfScreenLine::AboveTopLine,
                PositionOfScreenLine::AtScreenIndex(end_index),
            ) => PositionOfCursorInViewport::StartsAboveViewport { end_index },
            (PositionOfScreenLine::AboveTopLine, PositionOfScreenLine::BelowBottomLine) => {
                PositionOfCursorInViewport::StartsAndEndsOutsideViewport
            }
            (PositionOfScreenLine::AtScreenIndex(_), PositionOfScreenLine::AboveTopLine) => {
                panic!("start of cursor is in viewport, but bottom is above the top line");
            }
            (
                PositionOfScreenLine::AtScreenIndex(start_index),
                PositionOfScreenLine::AtScreenIndex(end_index),
            ) => PositionOfCursorInViewport::EntirelyInViewport {
                start_index,
                end_index,
            },
            (
                PositionOfScreenLine::AtScreenIndex(start_index),
                PositionOfScreenLine::BelowBottomLine,
            ) => PositionOfCursorInViewport::EndsBelowViewport { start_index },
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

    pub fn move_cursor_down(&mut self, lines: usize) {
        self.update_so_new_cursor_is_visible(self.doc.move_cursor_down(lines, &self.current_focus));
    }

    pub fn move_cursor_up(&mut self, lines: usize) {
        self.update_so_new_cursor_is_visible(self.doc.move_cursor_up(lines, &self.current_focus));
    }

    pub fn expand_or_move_cursor_right_or_down(&mut self) {
        self.update_so_new_cursor_is_visible(
            self.doc
                .expand_or_move_cursor_right_or_down(&self.current_focus),
        );
    }

    pub fn collapse_or_move_cursor_left_or_up(&mut self) {
        self.update_so_new_cursor_is_visible(
            self.doc
                .collapse_or_move_cursor_left_or_up(&self.current_focus),
        );
    }

    pub fn focus_top(&mut self) {
        let (top_screen_line, cursor) = self
            .doc
            .top_screen_line_and_cursor()
            .expect("top_screen_line_and_cursor to be Some if we've created a DocumentViewer");
        self.top_line = top_screen_line;
        self.current_focus = cursor;
    }

    pub fn focus_bottom(&mut self) {
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

    pub fn move_focused_elem_to_top(&mut self) {
        let cursor_range = self.doc.cursor_range(&self.current_focus);
        let scrolloff = self.effective_scrolloff();
        // If the focused node is multiple lines, put the start at the top of the screen.
        self.top_line = self.n_screen_lines_before_or_top_of_doc(cursor_range.start, scrolloff);
    }

    pub fn move_focused_elem_to_center(&mut self) {
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

    pub fn move_focused_elem_to_bottom(&mut self) {
        let cursor_range = self.doc.cursor_range(&self.current_focus);
        let scrolloff = self.effective_scrolloff();
        // If `height = 5`, and `scrolloff = 1`, then we want the end to be the fourth line,
        // so we want to go `3 = height - 1 - scrolloff` lines before.
        let offset = (self.dimensions.height - 1) - scrolloff;
        // If the focused node is multiple lines, put the end at the bottom of the screen.
        self.top_line = self.n_screen_lines_before_or_top_of_doc(cursor_range.end, offset);
    }

    pub fn scroll_viewport_down(&mut self, mut lines: usize) {
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

    pub fn scroll_viewport_up(&mut self, mut lines: usize) {
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

    fn jump_down(&mut self, num_screen_lines: Option<NonZeroUsize>) {
        let focused_range = self.doc.cursor_range(&self.current_focus);

        // It maybe feels a little weird to use the start/end index when something
        // is half on the screen, as opposed to something more "principled" like
        // start/2, but the user isn't trying to jump to a specific element, so
        // it doesn't really matter.
        //
        // Using the start/end index might also mean there will be a tiny bit less visual jitter.
        let focus_index = match self.position_of_cursor_in_viewport(&focused_range) {
            PositionOfCursorInViewport::StartsAboveViewport { end_index } => end_index,
            PositionOfCursorInViewport::EndsBelowViewport { start_index } => start_index,
            PositionOfCursorInViewport::StartsAndEndsOutsideViewport => self.dimensions.height / 2,
            PositionOfCursorInViewport::EntirelyInViewport { end_index, .. } => end_index,
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
                let focus_index = cmp::min(
                    focus_index + lines_to_move.get(),
                    self.dimensions.height - 1,
                );
                self.move_focus_to_screen_index_or_eof(focus_index);
            }
        }
    }

    fn jump_up(&mut self, num_screen_lines: Option<NonZeroUsize>) {
        let focused_range = self.doc.cursor_range(&self.current_focus);

        // Note that we pick the `start_index` in the `EntirelyInViewport` case. (See comment
        // below.)
        let focus_index = match self.position_of_cursor_in_viewport(&focused_range) {
            PositionOfCursorInViewport::StartsAboveViewport { end_index } => end_index,
            PositionOfCursorInViewport::EndsBelowViewport { start_index } => start_index,
            PositionOfCursorInViewport::StartsAndEndsOutsideViewport => self.dimensions.height / 2,
            PositionOfCursorInViewport::EntirelyInViewport { start_index, .. } => start_index,
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
        NonZeroUsize::new(cmp::max(self.dimensions.height / 2, 1)).unwrap()
    }

    fn update_so_new_cursor_is_visible(&mut self, new_cursor: Option<D::Cursor>) {
        // If an operation doesn't move the cursor, it will return `None`, so there's
        // nothing to do.
        let Some(new_cursor) = new_cursor else {
            return;
        };

        self.current_focus = new_cursor;

        let (cursor_range, acceptable_start_index_range) =
            self.calculate_acceptable_start_screen_indexes_to_show_cursor_node(&self.current_focus);

        let AcceptableStartScreenIndexesToShowCursorNode {
            start: start_index,
            end: end_index,
            ..
        } = acceptable_start_index_range;

        let screen_line_at_first_acceptable_start = self.screen_line_at_screen_index(start_index);
        let screen_line_at_last_acceptable_start = self.screen_line_at_screen_index(end_index);

        let cursor_start_is_before_first_acceptable_start =
            match screen_line_at_first_acceptable_start {
                // If there's no screen line af the first acceptable start, then must be the after
                // the end of the file, so the cursor start is definitely before then.
                None => true,
                Some(acceptable_start) => cursor_range.start < acceptable_start,
            };

        let cursor_start_is_at_or_before_last_acceptable_start =
            match screen_line_at_last_acceptable_start {
                None => true, // Same logic as above
                Some(acceptable_start) => cursor_range.start <= acceptable_start,
            };

        if cursor_start_is_before_first_acceptable_start {
            // Cursor is too close to the top of the screen (or past it); move the viewport so
            // the cursor is at the start of the acceptable range.
            self.top_line = self.n_screen_lines_before(cursor_range.start, start_index);
        } else if cursor_start_is_at_or_before_last_acceptable_start {
            // Nothing to do, the cursor is in an acceptable range!
        } else {
            // Cursor is too close to the bottom of the screen (or past it); move the viewport
            // so the cursor is at the end of the acceptable range.
            self.top_line = self.n_screen_lines_before(cursor_range.start, end_index);
        }
    }

    // Someday: We should compute the `CursorRange` outside of this function, pass it in,
    // and then get rid of `cursor_layout_details` in lieu of functions to directly
    // count `bounded_screen_lines_{before,after}_screen_line`.
    fn calculate_acceptable_start_screen_indexes_to_show_cursor_node(
        &self,
        cursor: &D::Cursor,
    ) -> (
        CursorRange<D::ScreenLine>,
        AcceptableStartScreenIndexesToShowCursorNode,
    ) {
        // We want to make sure that as much of the newly focused node is visible. The high level
        // logic here is to first determine the range of "acceptable" places for the cursor to be
        // This is by default the entire screen, but then it gets shrunk based on the `scrolloff`
        // setting, and then increased again based and the height of the focused node, or proximity
        // to the start/end of the document.
        //
        // Once we have this range, we will snap the position of the cursor into that range.

        let cursor_layout_details = self
            .doc
            .cursor_layout_details(&cursor, self.dimensions.height - 1);

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

        if let Some(lines_before_start) =
            cursor_layout_details.bounded_doc_screen_lines_before_start
        {
            first_acceptable_screen_index =
                cmp::min(first_acceptable_screen_index, lines_before_start);
        }

        if let Some(lines_after_end) = cursor_layout_details.bounded_doc_screen_lines_after_end {
            last_acceptable_screen_index = cmp::max(
                last_acceptable_screen_index,
                // The bound is approximate, to prevent us from having to look at the whole
                // document, not exact;
                // last_screen_index.saturating_sub(lines_after_end),
                last_screen_index - lines_after_end,
            );
        }

        #[cfg(debug_assertions)]
        let range_after_considering_start_and_end_of_document =
            first_acceptable_screen_index..=last_acceptable_screen_index;

        // Now we need to expand the acceptable range based on the height of the cursor, up
        // to the whole size of the screen again.

        let height_of_acceptable_range =
            last_acceptable_screen_index - first_acceptable_screen_index + 1;
        let cursor_height = cursor_layout_details.range.num_screen_lines;

        if cursor_height >= self.dimensions.height {
            // Simple case, the cursor is as big or bigger than the screen, so the whole screen
            // is available.
            first_acceptable_screen_index = 0;
            last_acceptable_screen_index = self.dimensions.height - 1;
        } else if cursor_height > height_of_acceptable_range {
            let mut additional_space_needed = cursor_height - height_of_acceptable_range;
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
                cmp::min(diff_between_sides, additional_space_needed);

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
        } else {
            // Cursor height <= height of acceptable range, so we don't need to
            // make any updates.
        }

        #[cfg(debug_assertions)]
        let range_after_expanding_due_to_cursor_height =
            first_acceptable_screen_index..=last_acceptable_screen_index;

        // Final step: convert the acceptable screen index range into an acceptable
        // range for the start of the cursor.
        let first_acceptable_screen_index_for_start_of_cursor = first_acceptable_screen_index;
        let last_acceptable_screen_index_for_start_of_cursor = cmp::max(
            first_acceptable_screen_index_for_start_of_cursor,
            // Subtract (size - 1); for example, if the cursor takes up two lines, then
            // the last acceptable start is one line before the last acceptable screen index.
            last_acceptable_screen_index
                .saturating_sub(cursor_layout_details.range.num_screen_lines - 1),
        );

        let acceptable_start_indexes = AcceptableStartScreenIndexesToShowCursorNode {
            #[cfg(debug_assertions)]
            cursor_height,
            #[cfg(debug_assertions)]
            last_screen_index,
            #[cfg(debug_assertions)]
            range_after_considering_scrolloff,
            #[cfg(debug_assertions)]
            range_after_considering_start_and_end_of_document,
            #[cfg(debug_assertions)]
            range_after_expanding_due_to_cursor_height,
            // The actual fields
            start: first_acceptable_screen_index_for_start_of_cursor,
            end: last_acceptable_screen_index_for_start_of_cursor,
        };

        (cursor_layout_details.range, acceptable_start_indexes)
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

        match self.position_of_cursor_in_viewport(&old_cursor_range) {
            PositionOfCursorInViewport::StartsAboveViewport { end_index } => {
                // Don't top line to keep anchored, so we'll keep the end of the line
                // in the same place.
                self.update_dimensions_and_resize_doc(new_dimensions);
                let new_cursor_range = self.doc.cursor_range(&self.current_focus);
                self.top_line =
                    self.n_screen_lines_before_or_top_of_doc(new_cursor_range.end, end_index);
            }
            PositionOfCursorInViewport::StartsAndEndsOutsideViewport => {
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
            PositionOfCursorInViewport::EntirelyInViewport { start_index, .. }
            | PositionOfCursorInViewport::EndsBelowViewport { start_index } => {
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

        match self.position_of_cursor_in_viewport(&cursor_range) {
            PositionOfCursorInViewport::StartsAboveViewport { end_index } => {
                // Keep the end of focused node in the same percentile
                anchor_screen_line = cursor_range.end;
                new_index = convert_old_index_to_new_index(end_index);
            }
            PositionOfCursorInViewport::StartsAndEndsOutsideViewport => {
                // Keep middle of what's visible on screen in the middle.
                let half_old_height = self.dimensions.height / 2;
                anchor_screen_line = self.screen_line_at_screen_index(half_old_height).unwrap();
                new_index = new_height / 2;
            }
            PositionOfCursorInViewport::EntirelyInViewport {
                start_index,
                end_index,
            } => {
                // Keep the middle of focused node in the same percentile.
                let middle_index = (start_index + end_index) / 2;
                anchor_screen_line = self.screen_line_at_screen_index(middle_index).unwrap();
                new_index = convert_old_index_to_new_index(middle_index);
            }
            PositionOfCursorInViewport::EndsBelowViewport { start_index } => {
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
        // relaxation here, so that if you do that, and the resize the screen, the cursor won't
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

    pub fn document_eof(&mut self) {
        self.doc.eof();
    }

    pub fn append_document_data(&mut self, data: &[u8]) {
        self.doc.append(data);

        if self.tailing_end_of_document {
            self.focus_bottom();
        }
    }

    pub fn do_action(&mut self, action: Action) {
        let prev_cursor = self.current_focus.clone();

        match action {
            Action::NoOp => (),
            Action::MoveCursorDown(n) => self.move_cursor_down(n),
            Action::MoveCursorUp(n) => self.move_cursor_up(n),
            Action::ExpandOrMoveCursorRightOrDown => self.expand_or_move_cursor_right_or_down(),
            Action::CollapseOrMoveCursorLeftOrUp => self.collapse_or_move_cursor_left_or_up(),
            Action::ScrollViewportDown(n) => self.scroll_viewport_down(n),
            Action::ScrollViewportUp(n) => self.scroll_viewport_up(n),
            Action::JumpDown(n) => self.jump_down(n),
            Action::JumpUp(n) => self.jump_up(n),
            Action::FocusTop => self.focus_top(),
            Action::FocusBottom => self.focus_bottom(),
            Action::MoveFocusedElemToTop => self.move_focused_elem_to_top(),
            Action::MoveFocusedElemToCenter => self.move_focused_elem_to_center(),
            Action::MoveFocusedElemToBottom => self.move_focused_elem_to_bottom(),
        }

        // When we focus the bottom of the document, we'll start tailing the
        // end, and we stop when we move the cursor.
        if matches!(action, Action::FocusBottom) {
            self.tailing_end_of_document = true;
        } else if prev_cursor != self.current_focus {
            self.tailing_end_of_document = false;
        }
    }

    pub fn viewport_lines<'a>(&'a self) -> impl Iterator<Item = Option<D::ScreenLine>> + 'a {
        ViewportLinesIterator {
            document: &self.doc,
            next_line: Some(self.top_line.clone()),
            remaining_height: self.dimensions.height,
        }
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
    use insta::{allow_duplicates, assert_debug_snapshot, assert_snapshot};

    use std::fmt::{self, Write};

    use crate::dimensions::Dimensions;
    use crate::test_helpers::format_table;
    use crate::text_document::{Cursor, TextDocument};

    fn init(
        contents: &[u8],
        width: usize,
        height: usize,
        scrolloff: usize,
    ) -> DocumentViewer<TextDocument> {
        let mut doc = TextDocument::new(width);
        doc.append(contents);

        let (top_line, initial_cursor) = doc.top_screen_line_and_cursor().unwrap();
        let dimensions = Dimensions { width, height };
        DocumentViewer::new(doc, top_line, initial_cursor, dimensions, scrolloff)
    }

    #[derive(Clone)]
    enum Change {
        Action(Action),
        ResizeWidth(usize),
        ResizeHeight(usize),
        Resize(Dimensions),
        SetScrolloff(usize),
        AppendDocumentData(Vec<u8>),
        // DocumentEof,
    }

    impl fmt::Display for Change {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> Result<(), fmt::Error> {
            match self {
                Change::Action(action) => write!(f, "{:?}", action),
                Change::ResizeWidth(width) => write!(f, "ResizeWidth({})", width),
                Change::ResizeHeight(height) => write!(f, "ResizeHeight({})", height),
                Change::Resize(Dimensions { width, height }) => {
                    write!(f, "Resize({{ width: {}, height: {} }})", width, height)
                }
                Change::SetScrolloff(scrolloff) => write!(f, "SetScrolloff({})", scrolloff),
                Change::AppendDocumentData(_) => write!(f, "AppendDocData"),
            }
        }
    }

    fn move_cursor_down(n: usize) -> Change {
        Change::Action(Action::MoveCursorDown(n))
    }

    fn move_cursor_up(n: usize) -> Change {
        Change::Action(Action::MoveCursorUp(n))
    }

    fn scroll_viewport_down(n: usize) -> Change {
        Change::Action(Action::ScrollViewportDown(n))
    }

    fn scroll_viewport_up(n: usize) -> Change {
        Change::Action(Action::ScrollViewportUp(n))
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

    impl<D: Document> DocumentViewer<D> {
        fn render(&self) -> String {
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
                    self.doc.debug_text_content(&screen_line).as_bstr(),
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
                viewer.render()
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
    ) -> AcceptableStartScreenIndexesToShowCursorNode {
        viewer
            .calculate_acceptable_start_screen_indexes_to_show_cursor_node(cursor)
            .1
    }

    #[test]
    fn test_acceptable_start_screen_indexes() {
        let mut viewer = init(b"a\nbbbb\nc\nd\ne\nf\ng\n", 1, 10, 0);
        assert_snapshot!(viewer.render(), @r"
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
        AcceptableStartScreenIndexesToShowCursorNode {
            cursor_height: 4,
            last_screen_index: 9,
            range_after_considering_scrolloff: 0..=9,
            range_after_considering_start_and_end_of_document: 0..=9,
            range_after_expanding_due_to_cursor_height: 0..=9,
            start: 0,
            end: 6,
        }
        ");

        viewer.set_scrolloff(3);
        assert_debug_snapshot!(acceptable_screen_indexes(&viewer, &line_2), @r"
        AcceptableStartScreenIndexesToShowCursorNode {
            cursor_height: 4,
            last_screen_index: 9,
            range_after_considering_scrolloff: 3..=6,
            range_after_considering_start_and_end_of_document: 1..=6,
            range_after_expanding_due_to_cursor_height: 1..=6,
            start: 1,
            end: 3,
        }
        ");

        // Example from the comment in `calculate_acceptable_start_screen_indexes_to_show_cursor_node`:
        let viewer = init(b"a\nbbbbbbbb\nc\nd\ne\nf\n", 1, 10, 4);
        assert_snapshot!(viewer.render(), @r"
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
        AcceptableStartScreenIndexesToShowCursorNode {
            cursor_height: 8,
            last_screen_index: 9,
            range_after_considering_scrolloff: 4..=5,
            range_after_considering_start_and_end_of_document: 1..=5,
            range_after_expanding_due_to_cursor_height: 1..=8,
            start: 1,
            end: 1,
        }
        ");
    }

    #[test]
    fn test_acceptable_start_screen_indexes_when_focused_node_bigger_than_viewport() {
        // Odd height
        allow_duplicates! {
            // Odd and even heights of the focused node
            for (input, height) in [("a\nb\nc\nddd\nc\nd\ne", 3), ("a\nb\nc\ndddd\nc\nd\ne", 4)].iter() {
                let viewer = init(input.as_bytes(), 1, 3, 0);
                assert_snapshot!(viewer.render(), @r"
                ┌SI┬─L#┬───┐
                │ 0│*1 │ a │
                │ 1│ 2 │ b │
                │ 2│ 3 │ c │
                └──┴───┴───┘
                ");

                let line_4 = viewer.doc.cursor_to_line_n(4);
                let mut acceptable_screen_indexes = acceptable_screen_indexes(&viewer, &line_4);
                assert_eq!(acceptable_screen_indexes.cursor_height, *height);
                // Clear for the snapshot, since it differs
                acceptable_screen_indexes.cursor_height = 0;
                assert_debug_snapshot!(acceptable_screen_indexes, @r"
                AcceptableStartScreenIndexesToShowCursorNode {
                    cursor_height: 0,
                    last_screen_index: 2,
                    range_after_considering_scrolloff: 0..=2,
                    range_after_considering_start_and_end_of_document: 0..=2,
                    range_after_expanding_due_to_cursor_height: 0..=2,
                    start: 0,
                    end: 0,
                }
                ");
            }
        }

        // Even height
        allow_duplicates! {
            // Odd and even heights of the focused node
            for (input, height) in [("a\nb\nc\nddddd\nc\nd\ne", 5), ("a\nb\nc\ndddd\nc\nd\ne", 4)].iter() {
                let viewer = init(input.as_bytes(), 1, 4, 0);
                assert_snapshot!(viewer.render(), @r"
                ┌SI┬─L#┬───┐
                │ 0│*1 │ a │
                │ 1│ 2 │ b │
                │ 2│ 3 │ c │
                │ 3│ 4 │ d↩│
                └──┴───┴───┘
                ");

                let line_4 = viewer.doc.cursor_to_line_n(4);
                let mut acceptable_screen_indexes = acceptable_screen_indexes(&viewer, &line_4);
                assert_eq!(acceptable_screen_indexes.cursor_height, *height);
                // Clear for the snapshot, since it differs
                acceptable_screen_indexes.cursor_height = 0;
                assert_debug_snapshot!(acceptable_screen_indexes, @r"
                AcceptableStartScreenIndexesToShowCursorNode {
                    cursor_height: 0,
                    last_screen_index: 3,
                    range_after_considering_scrolloff: 0..=3,
                    range_after_considering_start_and_end_of_document: 0..=3,
                    range_after_expanding_due_to_cursor_height: 0..=3,
                    start: 0,
                    end: 0,
                }
                ");
            }
        }
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
}
