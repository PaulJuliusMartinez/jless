use clap::ArgEnum;

use crate::flatjson::{FlatJson, Index, OptionIndex};
use crate::types::TTYDimensions;

#[derive(PartialEq, Eq, Copy, Clone, Debug, ArgEnum)]
pub enum Mode {
    Line,
    Data,
}

const DEFAULT_SCROLLOFF: u16 = 3;

pub struct JsonViewer {
    pub flatjson: FlatJson,
    pub top_row: Index,
    pub focused_row: Index,

    // Used for Focus{Prev,Next}Sibling actions.
    desired_depth: usize,

    // Used for JumpDown/JumpUp (ctrl-d/ctrl-u) actions.
    jump_distance: Option<usize>,

    pub dimensions: TTYDimensions,
    // We call this scrolloff_setting, to differentiate between
    // what it's set to, and what the scrolloff functionally is
    // if it's set to value >= height / 2.
    //
    // Access the functional value via .scrolloff().
    pub scrolloff_setting: u16,
    pub mode: Mode,
}

impl JsonViewer {
    pub fn new(flatjson: FlatJson, mode: Mode) -> JsonViewer {
        JsonViewer {
            flatjson,
            top_row: 0,
            focused_row: 0,
            desired_depth: 0,
            jump_distance: None,
            dimensions: TTYDimensions::default(),
            scrolloff_setting: DEFAULT_SCROLLOFF,
            mode,
        }
    }
}

#[derive(Debug, Copy, Clone)]
pub enum Action {
    // Does nothing, for debugging, shouldn't modify any state.
    #[allow(dead_code)]
    NoOp,

    MoveUp(usize),
    MoveDown(usize),
    MoveLeft,
    MoveRight,
    MoveTo(Index),

    // TODO: Come up with better names for these. Their behavior is
    // a little subtle. When moving down it'll move forward until
    // the depth changes. If the depth increases (because it got to
    // an expanded container) it'll stop on the line of the opening
    // of the container, but if the depth decreases (because we moved
    // past the last child of the current container) it'll focus the
    // line after.
    MoveUpUntilDepthChange,
    MoveDownUntilDepthChange,

    FocusParent,

    // The behavior of these is subtle and stateful. These move to the
    // previous/next sibling of the focused element. If we are focused
    // on the first/last child, we will move to the parent, but we
    // will remember what depth we were at when we first performed
    // this action, and move back to that depth the next time we can.
    FocusPrevSibling(usize),
    FocusNextSibling(usize),

    FocusFirstSibling,
    FocusLastSibling,
    FocusTop,
    FocusBottom,
    FocusMatchingPair,

    ScrollUp(usize),
    ScrollDown(usize),

    // By default, these move by half a screen, and move the focus by
    // the same number of lines, so the focus doesn't appear to move
    // on the screen. When jumping down, it will not show lines past
    // the end of the file.
    //
    // When a count is provided, we'll move by that many *lines* (not
    // N half screen sizes). This count is stored in
    // JsonViewer.jump_distance and used for subsequent jumps, rather
    // than half a screen size.
    //
    // vim always moves both the viewing window and the focused line
    // by the appropriate lines, so the location of the focused line
    // on the screen will move when jumping past the end of the file
    // (or before the start).
    //
    // We'll implement a slight variation on this behavior. If the
    // viewing window moves, we'll keep the focused line in the same
    // vertical location, but once we're at the top of the file, and
    // the viewing window doesn't change at all, then we will change
    // the focused line by the expected count.
    //
    // These commands ignore the scrolloff option.
    JumpUp(Option<usize>),
    JumpDown(Option<usize>),

    PageUp(usize),
    PageDown(usize),

    MoveFocusedLineToTop,
    MoveFocusedLineToCenter,
    MoveFocusedLineToBottom,

    Click(u16),

    ToggleCollapsed,
    CollapseNodeAndSiblings,
    ExpandNodeAndSiblings,

    ToggleMode,

    ResizeViewerDimensions(TTYDimensions),
}

impl JsonViewer {
    pub fn perform_action(&mut self, action: Action) {
        let track_window = JsonViewer::should_refocus_window(&action);
        let reset_desired_depth = JsonViewer::should_reset_desired_depth(&action);

        match action {
            Action::NoOp => {}
            Action::MoveUp(n) => self.move_up(n),
            Action::MoveDown(n) => self.move_down(n),
            Action::MoveLeft => self.move_left(),
            Action::MoveRight => self.move_right(),
            Action::MoveTo(index) => self.focused_row = index,
            Action::MoveUpUntilDepthChange => self.move_up_until_depth_change(),
            Action::MoveDownUntilDepthChange => self.move_down_until_depth_change(),
            Action::FocusParent => self.focus_parent(),
            Action::FocusPrevSibling(n) => self.focus_prev_sibling(n),
            Action::FocusNextSibling(n) => self.focus_next_sibling(n),
            Action::FocusFirstSibling => self.focus_first_sibling(),
            Action::FocusLastSibling => self.focus_last_sibling(),
            Action::FocusTop => self.focus_top(),
            Action::FocusBottom => self.focus_bottom(),
            Action::FocusMatchingPair => self.focus_matching_pair(),
            Action::ScrollUp(n) => self.scroll_up(n),
            Action::ScrollDown(n) => self.scroll_down(n),
            Action::JumpUp(option_n) => self.jump_up(option_n),
            Action::JumpDown(option_n) => self.jump_down(option_n),
            Action::PageUp(n) => self.scroll_up(self.dimensions.height as usize * n),
            Action::PageDown(n) => self.scroll_down(self.dimensions.height as usize * n),
            Action::MoveFocusedLineToTop => self.move_focused_line_to_top(),
            Action::MoveFocusedLineToCenter => self.move_focused_line_to_center(),
            Action::MoveFocusedLineToBottom => self.move_focused_line_to_bottom(),
            Action::Click(n) => self.click_row(n),
            Action::ToggleCollapsed => self.toggle_collapsed(),
            Action::CollapseNodeAndSiblings => self.collapse_node_and_siblings(),
            Action::ExpandNodeAndSiblings => self.expand_node_and_siblings(),
            Action::ToggleMode => self.toggle_mode(),
            Action::ResizeViewerDimensions(dims) => self.dimensions = dims,
        }

        if reset_desired_depth {
            self.desired_depth = self.flatjson[self.focused_row].depth;
        }

        if track_window {
            self.ensure_focused_row_is_visible();
        }
    }

    fn should_refocus_window(action: &Action) -> bool {
        match action {
            Action::NoOp => false,
            Action::MoveUp(_) => true,
            Action::MoveDown(_) => true,
            Action::MoveLeft => true,
            Action::MoveRight => true,
            Action::MoveTo(_) => true,
            Action::MoveUpUntilDepthChange => true,
            Action::MoveDownUntilDepthChange => true,
            Action::FocusParent => true,
            Action::FocusPrevSibling(_) => true,
            Action::FocusNextSibling(_) => true,
            Action::FocusFirstSibling => true,
            Action::FocusLastSibling => true,
            Action::FocusTop => false, // Window refocusing is handled in focus_top.
            Action::FocusBottom => true,
            Action::FocusMatchingPair => true,
            Action::ScrollUp(_) => false,
            Action::ScrollDown(_) => false,
            Action::JumpUp(_) => false,
            Action::JumpDown(_) => false,
            Action::PageUp(_) => false,
            Action::PageDown(_) => false,
            Action::MoveFocusedLineToTop => false,
            Action::MoveFocusedLineToCenter => false,
            Action::MoveFocusedLineToBottom => false,
            Action::Click(_) => true,
            Action::CollapseNodeAndSiblings => true,
            Action::ExpandNodeAndSiblings => true,
            Action::ToggleMode => false,
            Action::ResizeViewerDimensions(_) => true,
            _ => false,
        }
    }

    fn should_reset_desired_depth(action: &Action) -> bool {
        !matches!(
            action,
            Action::NoOp
                | Action::FocusPrevSibling(_)
                | Action::FocusNextSibling(_)
                | Action::ScrollUp(_)
                | Action::ScrollDown(_)
                | Action::MoveFocusedLineToTop
                | Action::MoveFocusedLineToCenter
                | Action::MoveFocusedLineToBottom
                | Action::ToggleMode
                | Action::ResizeViewerDimensions(_)
        )
    }

    fn move_up(&mut self, rows: usize) {
        let mut row = self.focused_row;

        for _ in 0..rows {
            let prev_row = match self.mode {
                Mode::Line => self.flatjson.prev_visible_row(row),
                Mode::Data => self.flatjson.prev_item(row),
            };

            match prev_row {
                OptionIndex::Nil => break,
                OptionIndex::Index(prev_row_index) => {
                    row = prev_row_index;
                }
            }
        }

        self.focused_row = row;
    }

    fn move_down(&mut self, rows: usize) {
        let mut row = self.focused_row;

        for _ in 0..rows {
            let next_row = match self.mode {
                Mode::Line => self.flatjson.next_visible_row(row),
                Mode::Data => self.flatjson.next_item(row),
            };

            match next_row {
                OptionIndex::Nil => break,
                OptionIndex::Index(next_row_index) => {
                    row = next_row_index;
                }
            }
        }

        self.focused_row = row;
    }

    fn move_right(&mut self) {
        let focused_row = &self.flatjson[self.focused_row];
        if focused_row.is_primitive() {
            return;
        }

        if focused_row.is_collapsed() {
            self.flatjson.expand(self.focused_row);
            return;
        }

        if focused_row.is_opening_of_container() {
            self.focused_row = focused_row.first_child().unwrap();
        } else {
            debug_assert!(
                self.mode == Mode::Line,
                "Can't be focused on closing char in Data mode"
            );
            self.focused_row = self.flatjson.prev_visible_row(self.focused_row).unwrap();
        }
    }

    fn move_left(&mut self) {
        if self.flatjson[self.focused_row].is_container()
            && self.flatjson[self.focused_row].is_expanded()
        {
            self.flatjson.collapse(self.focused_row);
            if self.flatjson[self.focused_row].is_closing_of_container() {
                self.focused_row = self.flatjson[self.focused_row].pair_index().unwrap();
            }
            return;
        }

        self.focus_parent();
    }

    fn move_up_until_depth_change(&mut self) {
        let mut row = self.focused_row;
        let mut current_depth = self.flatjson[row].depth;

        // We *will* change depths if the very next row is
        // at a different depth.
        let mut moved_yet = false;

        loop {
            let prev_row = match self.mode {
                Mode::Line => self.flatjson.prev_visible_row(row),
                Mode::Data => self.flatjson.prev_item(row),
            };

            match prev_row {
                OptionIndex::Nil => break,
                OptionIndex::Index(prev_row_index) => {
                    let prev_row_depth = self.flatjson[prev_row_index].depth;
                    if prev_row_depth != current_depth {
                        // If the line immediately above the starting one
                        // is at greater depth, then we won't stop immediately.
                        // We will keep moving along the new depth until it changes.
                        //
                        // This makes sure that this action acts like the inverse
                        // of move_down_until_depth_change, which will focus
                        // focus to a less indented line, but not a more indented one.
                        if !moved_yet && prev_row_depth > current_depth {
                            current_depth = prev_row_depth;
                        } else {
                            // If we're not in the above special case, then we do
                            // want to stop because the depth changed. The only
                            // question then is whether we keep the focus at the
                            // current depth or choose to focus the line with the
                            // different depth. We will only focus the line with
                            // the different depth if we haven't moved yet to ensure
                            // we move at least one row.
                            if !moved_yet {
                                row = prev_row_index;
                            }
                            break;
                        }
                    }
                    row = prev_row_index;
                    moved_yet = true;
                }
            }
        }

        self.focused_row = row;
    }

    fn move_down_until_depth_change(&mut self) {
        let mut row = self.focused_row;
        let current_depth = self.flatjson[row].depth;

        // We *will* move down into a child if that's the
        // very next line.
        let mut moved_yet = false;

        loop {
            let next_row = match self.mode {
                Mode::Line => self.flatjson.next_visible_row(row),
                Mode::Data => self.flatjson.next_item(row),
            };

            match next_row {
                OptionIndex::Nil => break,
                OptionIndex::Index(next_row_index) => {
                    let next_row_depth = self.flatjson[next_row_index].depth;
                    if next_row_depth != current_depth {
                        // Do move to parent nodes, but don't move into
                        // child nodes (unless we haven't moved yet).
                        if next_row_depth < current_depth || !moved_yet {
                            row = next_row_index;
                        }
                        break;
                    }
                    row = next_row_index;
                    moved_yet = true;
                }
            }
        }

        self.focused_row = row;
    }

    fn focus_parent(&mut self) {
        if let OptionIndex::Index(parent) = self.flatjson[self.focused_row].parent {
            self.focused_row = parent;
        }
    }

    fn focus_prev_sibling(&mut self, rows: usize) {
        for _ in 0..rows {
            // The user is trying to move up in the file, but stay at the desired depth, so we just
            // move up once, and then, if we're focused on a node that's nested deeper than the
            // desired depth, move up the node's parents until we get to the right depth.
            self.move_up(1);
            let mut focused_row = &self.flatjson[self.focused_row];
            while focused_row.depth > self.desired_depth {
                self.focused_row = focused_row.parent.unwrap();
                focused_row = &self.flatjson[self.focused_row];
            }
        }
    }

    fn focus_next_sibling(&mut self, rows: usize) {
        for _ in 0..rows {
            // The user is trying to move down in the file, but stay at the desired depth.
            // If we just move down once, this will accomplish what the user wants, unless
            // they are already at the correct depth and currently focused on a opening
            // of an expanded container. If this is the case, we just want to jump past the
            // contents to the closing brace. If we're in Data mode, since the closing brace
            // can't be focused, we'll still want to go past it to the next visible item.
            let current_row = &self.flatjson[self.focused_row];

            if current_row.depth == self.desired_depth
                && current_row.is_opening_of_container()
                && current_row.is_expanded()
            {
                let closing_brace = current_row.pair_index().unwrap();
                self.focused_row = if self.mode == Mode::Data {
                    match self.flatjson.next_item(closing_brace) {
                        // If there's no item after the closing brace, then we don't actually
                        // want to move the focus at all.
                        OptionIndex::Nil => self.focused_row,
                        OptionIndex::Index(i) => i,
                    }
                } else {
                    closing_brace
                }
            } else {
                self.move_down(1);
            }
        }
    }

    fn focus_first_sibling(&mut self) {
        match &self.flatjson[self.focused_row].parent {
            OptionIndex::Index(parent_index) => {
                self.focused_row = self.flatjson[*parent_index].first_child().unwrap();
            }
            // If node has no parent, then we're at the top level and want to focus
            // the first element, which is the top of the file.
            OptionIndex::Nil => self.focus_top(),
        }
    }

    fn focus_last_sibling(&mut self) {
        match &self.flatjson[self.focused_row].parent {
            OptionIndex::Index(parent_index) => {
                let closing_parent_index = self.flatjson[*parent_index].pair_index().unwrap();
                self.focused_row = self.flatjson[closing_parent_index].last_child().unwrap();
            }
            // If node has no parent, then we're at the top level and want to focus
            // the last element. If this last element is a container though, we want to
            // make sure to focus on the _start_ of the container.
            OptionIndex::Nil => {
                let last_index = self.flatjson.last_visible_index();
                if self.flatjson[last_index].is_container() {
                    self.focused_row = self.flatjson[last_index].pair_index().unwrap();
                } else {
                    self.focused_row = last_index;
                }
            }
        }
    }

    fn focus_top(&mut self) {
        self.top_row = 0;
        self.focused_row = 0;
    }

    fn focus_bottom(&mut self) {
        self.focused_row = match self.mode {
            Mode::Line => self.flatjson.last_visible_index(),
            Mode::Data => self.flatjson.last_visible_item(),
        };
    }

    fn focus_matching_pair(&mut self) {
        if self.mode == Mode::Data {
            return;
        }
        let current_row = &self.flatjson[self.focused_row];
        if current_row.is_collapsed() {
            return;
        }

        match current_row.pair_index() {
            // Do nothing; focused element isn't a container
            OptionIndex::Nil => {}
            OptionIndex::Index(matching_pair_index) => {
                self.focused_row = matching_pair_index;
            }
        }
    }

    fn scroll_up(&mut self, rows: usize) {
        self.top_row = self.count_n_lines_before(self.top_row, rows, self.mode);
        let max_focused_row = self.count_n_lines_past(
            self.top_row,
            (self.dimensions.height - self.scrolloff() - 1) as usize,
            self.mode,
        );

        if self.focused_row > max_focused_row {
            self.focused_row = max_focused_row;
        }
    }

    fn scroll_down(&mut self, rows: usize) {
        self.top_row = self.count_n_lines_past(self.top_row, rows, self.mode);
        let first_focusable_row =
            self.count_n_lines_past(self.top_row, self.scrolloff() as usize, self.mode);

        if self.focused_row < first_focusable_row {
            self.focused_row = first_focusable_row;
        }
    }

    fn jump_up(&mut self, distance: Option<usize>) {
        let lines = self.determine_jump_distance(distance);

        let original_top_row = self.top_row;
        let num_visible_before_focused = self.index_of_focused_row_on_screen();

        self.top_row = self.count_n_lines_before(self.top_row, lines, self.mode);

        // If the viewing window moved at all, then keep the focused line in the
        // same place vertically. But if we're at the top of the file, then move
        // the focused line by the expected amount. This prevents the viewing
        // window and the focused line from both changing, but by different amounts.
        if original_top_row != self.top_row {
            self.focused_row = self.count_n_lines_past(
                self.top_row,
                num_visible_before_focused as usize,
                self.mode,
            );
        } else {
            self.focused_row = self.count_n_lines_before(self.focused_row, lines, self.mode);
        }
    }

    fn jump_down(&mut self, distance: Option<usize>) {
        let lines = self.determine_jump_distance(distance);

        let original_top_row = self.top_row;
        let num_visible_before_focused = self.index_of_focused_row_on_screen();

        self.top_row = self.count_n_lines_past(self.top_row, lines, self.mode);

        let last_line = match self.mode {
            Mode::Line => self.flatjson.last_visible_index(),
            Mode::Data => self.flatjson.last_visible_item(),
        };
        let top_row_if_last_row_is_at_bottom =
            self.count_n_lines_before(last_line, self.dimensions.height as usize - 1, self.mode);

        // When jumping, we won't show lines past EOF, unless we already
        // are showing lines past EOF.
        if self.top_row > top_row_if_last_row_is_at_bottom {
            self.top_row = top_row_if_last_row_is_at_bottom.max(original_top_row);
        }

        // If the viewing window moved at all, then keep the focused line in the
        // same place vertically. But if we're at the bottom of the file, then move
        // the focused line by the expected amount. This prevents the viewing
        // window and the focused line from both changing, but by different amounts.
        if original_top_row != self.top_row {
            self.focused_row = self.count_n_lines_past(
                self.top_row,
                num_visible_before_focused as usize,
                self.mode,
            );
        } else {
            self.focused_row = self.count_n_lines_past(self.focused_row, lines, self.mode);
        }
    }

    // If the user provided a count to a jump command, sets that as the the new
    // jump distance. Otherwise, use the stored jump distance, or if none has
    // been set yet, use the default of half a window size.
    fn determine_jump_distance(&mut self, distance: Option<usize>) -> usize {
        self.jump_distance = distance.or(self.jump_distance);

        match self.jump_distance {
            Some(n) => n,
            None => (self.dimensions.height as usize / 2).max(1),
        }
    }

    fn move_focused_line_to_top(&mut self) {
        let padding = self.scrolloff() as usize;
        self.top_row = self.count_n_lines_before(self.focused_row, padding, self.mode);
    }

    fn move_focused_line_to_center(&mut self) {
        let padding = (self.dimensions.height / 2) as usize;
        self.top_row = self.count_n_lines_before(self.focused_row, padding, self.mode);
    }

    fn move_focused_line_to_bottom(&mut self) {
        let padding = (self.dimensions.height - self.scrolloff() - 1) as usize;
        self.top_row = self.count_n_lines_before(self.focused_row, padding, self.mode);
    }

    fn click_row(&mut self, row: u16) {
        self.focused_row = self.count_n_lines_past(self.top_row, (row - 1) as usize, self.mode);
        if self.flatjson[self.focused_row].is_opening_of_container() {
            self.toggle_collapsed();
        }
    }

    fn toggle_collapsed(&mut self) {
        let focused_row = &mut self.flatjson[self.focused_row];
        if focused_row.is_primitive() {
            return;
        }

        if focused_row.is_closing_of_container() {
            debug_assert!(
                focused_row.is_expanded(),
                "Focused on closing char when row is collapsed",
            );
            self.focused_row = self.flatjson[self.focused_row].pair_index().unwrap();
        }

        self.flatjson.toggle_collapsed(self.focused_row);
    }

    fn collapse_node_and_siblings(&mut self) {
        // If we're collapsing a node, make sure we're focused on the open.
        let focused_row = &mut self.flatjson[self.focused_row];
        if focused_row.is_closing_of_container() {
            debug_assert!(
                focused_row.is_expanded(),
                "Focused on closing char when row is collapsed",
            );
            self.focused_row = self.flatjson[self.focused_row].pair_index().unwrap();
        }

        self.set_collapse_state_on_node_and_siblings(true);
    }

    fn expand_node_and_siblings(&mut self) {
        self.set_collapse_state_on_node_and_siblings(false);
    }

    fn set_collapse_state_on_node_and_siblings(&mut self, collapsed: bool) {
        let first_sibling =
            if let OptionIndex::Index(parent) = self.flatjson[self.focused_row].parent {
                self.flatjson[parent].first_child().unwrap()
            } else {
                // If we don't have parent, that means we're at the top level, so the first
                // sibling is the very first element.
                0
            };

        let mut next_sibling = OptionIndex::Index(first_sibling);

        while let OptionIndex::Index(next) = next_sibling {
            if collapsed {
                self.flatjson.collapse(next);
            } else {
                self.flatjson.expand(next);
            }
            next_sibling = self.flatjson[next].next_sibling;
        }
    }

    fn toggle_mode(&mut self) {
        let index_of_focused_row = self.index_of_focused_row_on_screen();

        // If we're transitioning from line mode to focused mode, and we're focused on
        // the closing of a container, we need to move the focuse.
        if self.mode == Mode::Line && self.flatjson[self.focused_row].is_closing_of_container() {
            // We'll move focus to the next item, unless we're at the end of
            // the file and have to move focus backwards.
            //
            // By focusing the next item, and ensuring that the focus stays in the
            // same place on the screen, it will look the surrounding data is getting
            // "pulled" towards the focused line.
            if let OptionIndex::Index(next) = self.flatjson.next_item(self.focused_row) {
                self.focused_row = next;
            } else {
                self.focused_row = self.flatjson.prev_item(self.focused_row).unwrap();
            }
        }

        // Toggle the mode.
        self.mode = match self.mode {
            Mode::Line => Mode::Data,
            Mode::Data => Mode::Line,
        };

        // Ensure focused line stays in same place on the screen.
        self.top_row =
            self.count_n_lines_before(self.focused_row, index_of_focused_row as usize, self.mode);
    }

    fn scrolloff(&self) -> u16 {
        self.scrolloff_setting.min((self.dimensions.height - 1) / 2)
    }

    // This is called after moving the cursor up or down (or other operations that
    // change where the focused row is) and makes sure that it isn't within SCROLLOFF
    // lines of the top or bottom of the screen.
    fn ensure_focused_row_is_visible(&mut self) {
        // First make sure that the top row is visible. It may no longer be visible
        // after performing an action like CollapseNodeAndSiblings.
        self.ensure_top_row_is_visible();

        // height; scrolloff; actual scrolloff; max_padding
        //   100       3              3            96
        //   15        7              7             7
        //   15        8              7             7
        //   16        8              7             8
        let scrolloff = self.scrolloff();
        // Max padding is max number of rows that can be visible between the focused
        // row and the top or bottom of the screen.
        let max_padding = self.dimensions.height - scrolloff - 1;

        // Normally as the user moves down the the file we'll keep the focused line
        // scrolloff lines from the bottom of the screen.
        //
        // But if the user jumps well past the end of the screen, rather than leaving
        // the cursor scrolloff lines from the bottom, we'll put it closer to the
        // middle, so they see more context, matching similar behavior in vim.
        //
        // In vim, the re-centering behavior occurs when you jump roughly half a screen
        // past the bottom of the current visible screen. The exact point where it
        // switches between leaving the cursor at the bottom of the screen vs.
        // recentering works out so that there are no lines in common between the
        // lines displayed before the jump and the lines displayed after the jump.
        //
        // We'll make the assumption that in JSON the context provided by previous lines
        // is less helpful. When we refocus the screen we'll put the focused line 1/3
        // of the way from the top, so we need to have moved 1 and 1/3 screen lengths
        // past the top line for there to not be any overlap in the lines visible on the
        // screen.
        //
        // We anticipate that users will also jump using FocusNextSibling frequently,
        // which means that the focused line is a natural starting point of a large
        // object, so showing more lines after the focused line than before makes
        // sense.
        //
        // This might make less sense if they arrived there after a random jump or
        // text search. Perhaps we could do something more intelligent where we try
        // to make sure that the parent is visible, but this works for now.
        //
        // Because of the assumption that lines after the focused line are more relevant,
        // we don't recenter the focused line when moving far up in the file.
        let recenter_distance = self.dimensions.height + (self.dimensions.height / 3);

        // Note that this will return 0 if focused_row < top_row.
        let num_visible_before_focused = self.count_visible_rows_before(
            self.top_row,
            self.focused_row,
            // Add 1 so we can differentiate between == recenter_distance and > recenter_distance
            recenter_distance + 1,
            self.mode,
        );

        // Handle focused line too close to or past the top of the screen.
        if self.focused_row < self.top_row || num_visible_before_focused < scrolloff {
            self.top_row =
                self.count_n_lines_before(self.focused_row, scrolloff as usize, self.mode);
        } else if num_visible_before_focused > max_padding {
            // Handle focused line too close to or past the bottom of the screen.

            // If the user moved well past the bottom of the screen, we will refocus
            // the cursor in the middle of the screen, rather than at the bottom of
            // the screen.
            //
            // Note this is padding from the _bottom_ of the screen.
            let refocus_padding = if num_visible_before_focused > recenter_distance {
                let bottom_padding = self.dimensions.height * 2 / 3;
                // Make sure to still obey scrolloff on the top if scrolloff > 1/3 of height.
                bottom_padding.min(max_padding)
            } else {
                scrolloff
            };

            // We need to figure out where the last line is because we won't
            // show any empty lines past the end of the file (unless the
            // user explicitly scrolls past the end of the file).
            //
            // This overrides the scrolloff setting.
            let last_line = match self.mode {
                Mode::Line => self.flatjson.last_visible_index(),
                Mode::Data => self.flatjson.last_visible_item(),
            };
            let lines_visible_before_eof = self.count_visible_rows_before(
                self.focused_row,
                last_line,
                refocus_padding + 1,
                self.mode,
            );

            // Clamp the refocus padding at the number of lines visible before EOF
            // so that we don't show anything past EOF.
            let bottom_padding = refocus_padding.min(lines_visible_before_eof);
            self.top_row = self.count_n_lines_before(
                self.focused_row,
                (self.dimensions.height - bottom_padding - 1) as usize,
                self.mode,
            );
        }
    }

    // Makes sure that the top row is visible. If not, the top row will be updated
    // to the first visible parent of the top row.
    //
    // We need to consider both the case that a parent of the top row has been
    // collapsed, and the case that the top row is the closing brace of a container
    // that has been collapsed.
    //
    // In this second (much less likely) case, we'll set the top row to the opening
    // of the container (but then still make sure all of its parents are visible).
    fn ensure_top_row_is_visible(&mut self) {
        // Check rare case that top row is closing of container that is now collapsed.
        if self.flatjson[self.top_row].is_closing_of_container() {
            let opening = self.flatjson[self.top_row].pair_index().unwrap();
            if self.flatjson[opening].is_collapsed() {
                self.top_row = opening;
            }
        }

        // Now make sure all ancestors are visible.
        let mut ancestor = self.top_row;
        while let OptionIndex::Index(ancestor_index) = self.flatjson[ancestor].parent {
            if self.flatjson[ancestor_index].is_collapsed() {
                self.top_row = ancestor_index;
            }

            ancestor = ancestor_index;
        }
    }

    fn count_n_lines_before(&self, mut start: Index, mut lines: usize, mode: Mode) -> Index {
        while lines != 0 && start != 0 {
            start = match mode {
                Mode::Line => self.flatjson.prev_visible_row(start).unwrap(),
                Mode::Data => self.flatjson.prev_item(start).unwrap(),
            };
            lines -= 1;
        }
        start
    }

    fn count_n_lines_past(&self, mut start: Index, mut lines: usize, mode: Mode) -> Index {
        while lines != 0 {
            let next = match mode {
                Mode::Line => self.flatjson.next_visible_row(start),
                Mode::Data => self.flatjson.next_item(start),
            };

            match next {
                OptionIndex::Nil => break,
                OptionIndex::Index(n) => start = n,
            };

            lines -= 1;
        }

        start
    }

    // Counts how many visible lines/items (depending on mode) there are between start and end.
    //
    // start is counted as visible, and end is not counted as visible.
    //
    // If start == end, we return 0.
    //
    // We won't count more than max lines past start. If we still haven't gotten to end,
    // we'll return max.
    fn count_visible_rows_before(&self, mut start: Index, end: Index, max: u16, mode: Mode) -> u16 {
        let mut num_visible: u16 = 0;
        while start < end && num_visible < max {
            num_visible += 1;
            start = match mode {
                Mode::Line => self.flatjson.next_visible_row(start).unwrap(),
                Mode::Data => self.flatjson.next_item(start).unwrap(),
            };
        }
        num_visible
    }

    // Returns the index of the focused row within the actual viewing window.
    fn index_of_focused_row_on_screen(&self) -> u16 {
        self.count_visible_rows_before(
            self.top_row,
            self.focused_row,
            self.dimensions.height,
            self.mode,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::flatjson::{parse_top_level_json, NIL};

    impl OptionIndex {
        pub fn to_usize(&self) -> usize {
            match self {
                OptionIndex::Nil => NIL,
                OptionIndex::Index(i) => *i,
            }
        }
    }

    const OBJECT: &str = r#"{
        "1": 1,
        "2": [
            3,
            "4"
        ],
        "6": {
            "7": null,
            "8": true,
            "9": 9
        },
        "11": 11
    }"#;

    // Same object as DATA, but formatted as it would appear in data mode
    const DATA_OBJECT: &str = r#"{
        "1": 1,
        "2": [
            3,
            "4"],
        "6": {
            "7": null,
            "8": true,
            "9": 9},
        "11": 11}"#;

    #[test]
    fn test_move_up_down_line_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        assert_movements(
            &mut viewer,
            vec![(Action::MoveDown(1), 1), (Action::MoveDown(2), 3)],
        );

        viewer.flatjson.collapse(6);
        viewer.focused_row = 6;

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveDown(1), 11),
                (Action::MoveDown(1), 12),
                (Action::MoveDown(1), 12),
            ],
        );

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveUp(2), 6),
                (Action::MoveUp(1), 5),
                (Action::MoveUp(5), 0),
                (Action::MoveUp(2), 0),
            ],
        );

        viewer.flatjson.collapse(0);
        assert_movements(
            &mut viewer,
            vec![(Action::MoveUp(1), 0), (Action::MoveDown(1), 0)],
        );
    }

    #[test]
    fn test_move_up_down_data_mode() {
        let fj = parse_top_level_json(DATA_OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Data);

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveDown(1), 1),
                (Action::MoveDown(3), 4),
                (Action::MoveDown(1), 6),
            ],
        );

        viewer.flatjson.collapse(6);

        assert_movements(
            &mut viewer,
            vec![(Action::MoveDown(1), 11), (Action::MoveDown(1), 11)],
        );

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveUp(1), 6),
                (Action::MoveUp(3), 2),
                (Action::MoveUp(1), 1),
                (Action::MoveUp(1), 0),
                (Action::MoveUp(1), 0),
            ],
        );
    }

    #[test]
    fn test_move_left_right_line_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveRight, 1),
                (Action::MoveRight, 1),
                (Action::MoveDown(1), 2),
                (Action::MoveRight, 3),
                (Action::MoveLeft, 2),
                (Action::MoveLeft, 2),
            ],
        );

        assert!(viewer.flatjson[2].is_collapsed());

        viewer.focused_row = 10;
        assert_movements(
            &mut viewer,
            vec![
                // Right on closing brace takes you to previous line
                (Action::MoveRight, 9),
                (Action::MoveLeft, 6),
                (Action::MoveDown(4), 10),
                // Collapsing while on closing brace takes you to opening brace
                (Action::MoveLeft, 6),
            ],
        );

        assert!(viewer.flatjson[6].is_collapsed());

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveLeft, 0),
                (Action::MoveLeft, 0),
                (Action::MoveDown(1), 0),
            ],
        );

        assert!(viewer.flatjson[0].is_collapsed());
        assert_movements(&mut viewer, vec![(Action::MoveRight, 0)]);

        assert!(viewer.flatjson[0].is_expanded());
        assert_movements(&mut viewer, vec![(Action::MoveRight, 1)]);
    }

    #[test]
    fn test_move_left_right_data_mode() {
        let fj = parse_top_level_json(DATA_OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Data);

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveRight, 1),
                (Action::MoveRight, 1),
                (Action::MoveDown(5), 7),
                (Action::MoveLeft, 6),
                (Action::MoveLeft, 6),
            ],
        );

        assert!(viewer.flatjson[6].is_collapsed());

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveLeft, 0),
                (Action::MoveRight, 1),
                (Action::MoveLeft, 0),
                (Action::MoveLeft, 0),
            ],
        );

        assert!(viewer.flatjson[0].is_collapsed());
        assert_movements(
            &mut viewer,
            vec![(Action::MoveDown(1), 0), (Action::MoveRight, 0)],
        );

        assert!(viewer.flatjson[0].is_expanded());
        assert_movements(&mut viewer, vec![(Action::MoveLeft, 0)]);
    }

    #[test]
    fn test_move_up_down_until_depth_change_line_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveDownUntilDepthChange, 1),
                (Action::MoveDownUntilDepthChange, 2),
                (Action::MoveDownUntilDepthChange, 3),
                (Action::MoveDownUntilDepthChange, 5),
                (Action::MoveDownUntilDepthChange, 6),
                (Action::MoveDownUntilDepthChange, 7),
                (Action::MoveDownUntilDepthChange, 10),
                (Action::MoveDownUntilDepthChange, 12),
                (Action::MoveDownUntilDepthChange, 12),
                (Action::MoveUpUntilDepthChange, 10),
                (Action::MoveUpUntilDepthChange, 7),
                (Action::MoveUpUntilDepthChange, 6),
                (Action::MoveUpUntilDepthChange, 5),
                (Action::MoveUpUntilDepthChange, 3),
                (Action::MoveUpUntilDepthChange, 2),
                (Action::MoveUpUntilDepthChange, 1),
                (Action::MoveUpUntilDepthChange, 0),
                (Action::MoveUpUntilDepthChange, 0),
            ],
        );
    }

    #[test]
    fn test_move_up_down_until_depth_change_data_mode() {
        let fj = parse_top_level_json(DATA_OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Data);

        assert_movements(
            &mut viewer,
            vec![
                (Action::MoveDownUntilDepthChange, 1),
                (Action::MoveDownUntilDepthChange, 2),
                (Action::MoveDownUntilDepthChange, 3),
                (Action::MoveDownUntilDepthChange, 6),
                (Action::MoveDownUntilDepthChange, 7),
                (Action::MoveDownUntilDepthChange, 11),
                (Action::MoveDownUntilDepthChange, 11),
                (Action::MoveUpUntilDepthChange, 7),
                (Action::MoveUpUntilDepthChange, 6),
                (Action::MoveUpUntilDepthChange, 3),
                (Action::MoveUpUntilDepthChange, 2),
                (Action::MoveUpUntilDepthChange, 1),
                (Action::MoveUpUntilDepthChange, 0),
                (Action::MoveUpUntilDepthChange, 0),
            ],
        );
    }

    #[track_caller]
    fn assert_movements(viewer: &mut JsonViewer, actions_and_focuses: Vec<(Action, Index)>) {
        for (i, (action, expected_focused_row)) in actions_and_focuses.into_iter().enumerate() {
            viewer.perform_action(action);
            assert_eq!(
                viewer.focused_row,
                expected_focused_row,
                "expected row {} to be focused after {} actions (last action: {:?})",
                expected_focused_row,
                i + 1,
                action,
            );
        }
    }

    #[test]
    fn test_ensure_focused_line_is_visible_in_line_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);
        viewer.dimensions.height = 8;
        viewer.scrolloff_setting = 2;

        viewer.ensure_focused_row_is_visible();
        assert_eq!(viewer.top_row, 0);

        // Test pushing past bottom
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(1), 0, 1),
                (Action::MoveDown(5), 1, 6),
                (Action::MoveDown(1), 2, 7),
            ],
        );

        // Test pushing past top
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveUp(1), 2, 6),
                (Action::MoveUp(3), 1, 3),
                (Action::MoveUp(1), 0, 2),
                // Top is now top of file
                (Action::MoveUp(1), 0, 1),
            ],
        );

        // Test pushing past bottom with scrolloff == 0
        viewer.scrolloff_setting = 0;
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(6), 0, 7),
                (Action::MoveDown(1), 1, 8),
                (Action::MoveDown(4), 5, 12),
                (Action::MoveDown(1), 5, 12),
            ],
        );

        // Test pushing past top with scrolloff == 0
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveUp(7), 5, 5),
                (Action::MoveUp(1), 4, 4),
                (Action::MoveUp(5), 0, 0),
            ],
        );

        viewer.top_row = 0;
        viewer.focused_row = 1;
        viewer.scrolloff_setting = 2;

        // Test pushing past bottom at end of file
        assert_window_tracking(
            &mut viewer,
            vec![
                // Move to bottom of file
                (Action::MoveDown(9), 5, 10),
                // Push past bottom
                (Action::MoveDown(1), 5, 11),
                (Action::MoveDown(1), 5, 12),
            ],
        );

        // Put bottom of file on top of screen
        viewer.top_row = 8;
        viewer.focused_row = 10;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(1), 8, 11),
                (Action::MoveDown(1), 8, 12),
                (Action::MoveUp(2), 8, 10),
                (Action::MoveUp(1), 7, 9),
            ],
        );

        viewer.top_row = 0;
        viewer.focused_row = 0;
        viewer.dimensions.height = 6;
        viewer.flatjson.collapse(2);

        // Test with collapsed items
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(3), 0, 6),
                (Action::MoveDown(1), 1, 7),
                (Action::MoveDown(1), 2, 8),
                (Action::MoveDown(1), 6, 9),
                // Back up now
                (Action::MoveUp(2), 2, 7),
                (Action::MoveUp(1), 1, 6),
                (Action::MoveUp(1), 0, 2),
            ],
        );
    }

    #[test]
    fn test_ensure_focused_line_is_visible_in_data_mode() {
        let fj = parse_top_level_json(DATA_OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Data);
        viewer.dimensions.height = 7;
        viewer.scrolloff_setting = 2;

        viewer.ensure_focused_row_is_visible();
        assert_eq!(viewer.top_row, 0);

        // Test pushing past bottom
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(1), 0, 1),
                (Action::MoveDown(4), 1, 6),
                (Action::MoveDown(1), 2, 7),
            ],
        );

        // Test pushing past top
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveUp(1), 2, 6),
                (Action::MoveUp(2), 1, 3),
                (Action::MoveUp(1), 0, 2),
                // Top is now top of file
                (Action::MoveUp(1), 0, 1),
            ],
        );

        // Test pushing past bottom at end of file
        assert_window_tracking(
            &mut viewer,
            vec![
                // Move to bottom of file
                (Action::MoveDown(6), 3, 8),
                // Push past bottom
                (Action::MoveDown(1), 3, 9),
                (Action::MoveDown(1), 3, 11),
            ],
        );

        // Put bottom of file on top of screen
        viewer.top_row = 6;
        viewer.focused_row = 8;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(1), 6, 9),
                (Action::MoveDown(1), 6, 11),
                (Action::MoveUp(2), 6, 8),
                (Action::MoveUp(1), 4, 7),
            ],
        );

        viewer.top_row = 0;
        viewer.focused_row = 0;
        viewer.dimensions.height = 5;
        viewer.flatjson.collapse(2);

        // Test with collapsed items
        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(2), 0, 2),
                (Action::MoveDown(1), 1, 6),
                (Action::MoveDown(1), 2, 7),
                (Action::MoveDown(1), 6, 8),
                // Back up now
                (Action::MoveUp(1), 2, 7),
                (Action::MoveUp(1), 1, 6),
                (Action::MoveUp(1), 0, 2),
            ],
        );
    }

    const TALL_OBJECT: &str = "[1,2,3,4,5,6,7,8,9,10,11,12,13,14,15,16,17,18,19,20]";

    #[test]
    fn test_ensure_focused_line_is_visible_centers_focus_line_after_big_jump() {
        let fj = parse_top_level_json(TALL_OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);
        viewer.dimensions.height = 9;
        viewer.scrolloff_setting = 2;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveDown(8), 2, 8),
                (Action::FocusTop, 0, 0),
                (Action::MoveDown(9), 3, 9),
                (Action::FocusTop, 0, 0),
                (Action::MoveDown(12), 6, 12),
                (Action::FocusTop, 0, 0),
                // Jumped far, so focused line will be 1/3 from top.
                (Action::MoveDown(13), 11, 13),
            ],
        );

        // Have to be careful to still obey top scrolloff setting though.
        viewer.scrolloff_setting = 3;
        assert_window_tracking(
            &mut viewer,
            vec![(Action::FocusTop, 0, 0), (Action::MoveDown(13), 10, 13)],
        );
    }

    #[test]
    fn test_scroll() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);
        viewer.dimensions.height = 8;
        viewer.scrolloff_setting = 2;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::ScrollDown(1), 1, 3),
                (Action::ScrollDown(1), 2, 4),
                (Action::ScrollDown(3), 5, 7),
                // Can scroll so end of file is in middle of screen
                (Action::ScrollDown(1), 6, 8),
                (Action::ScrollDown(4), 10, 12),
                // Can scoll past scrolloff padding
                (Action::ScrollDown(1), 11, 12),
                (Action::ScrollDown(1), 12, 12),
                // Can't scroll past last line
                (Action::ScrollDown(1), 12, 12),
                // Can scroll one up
                (Action::ScrollUp(1), 11, 12),
                (Action::ScrollDown(1), 12, 12),
                // But moving up activates scrolloff
                (Action::MoveUp(1), 9, 11),
            ],
        );

        viewer.top_row = 12;
        viewer.focused_row = 12;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::ScrollUp(1), 11, 12),
                (Action::ScrollUp(1), 10, 12),
                (Action::ScrollUp(4), 6, 11),
                (Action::ScrollUp(1), 5, 10),
                // Can't scroll up past top of file
                (Action::ScrollUp(6), 0, 5),
            ],
        );
    }

    #[test]
    fn test_jump() {
        const TALL_OBJECT: &str = r#"{
            "1": [2],
            "4": [5],
            "7": [8],
            "10": [11],
            "13": [14],
            "16": [17],
        }"#; // 19

        let fj = parse_top_level_json(TALL_OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);
        viewer.dimensions.height = 5;
        viewer.scrolloff_setting = 0;
        viewer.focused_row = 3;

        assert_window_tracking(
            &mut viewer,
            vec![
                // Moves by half-screen rounded down.
                (Action::JumpDown(None), 2, 5),
                (Action::JumpDown(None), 4, 7),
                (Action::JumpUp(None), 2, 5),
                // Count moves by that many lines
                (Action::JumpDown(Some(4)), 6, 9),
                // And count is remembered
                (Action::JumpDown(None), 10, 13),
                // ... by both up and down
                (Action::JumpUp(None), 6, 9),
            ],
        );

        // Prioritize keeping focused line in same place, but once we're
        // at the top or bottom of the file, we will move it.
        viewer.dimensions.height = 8;
        viewer.top_row = 3;
        viewer.focused_row = 7;
        viewer.scrolloff_setting = 3;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::JumpUp(Some(2)), 1, 5),
                (Action::JumpUp(None), 0, 4),
                (Action::JumpUp(None), 0, 2),
                (Action::JumpUp(None), 0, 0),
                // Scrolloff is ignored.
                (Action::JumpDown(None), 2, 2),
            ],
        );

        viewer.dimensions.height = 8;
        viewer.top_row = 9;
        viewer.focused_row = 11;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::JumpDown(Some(2)), 11, 13),
                (Action::JumpDown(None), 12, 14),
                (Action::JumpDown(None), 12, 16),
                (Action::JumpDown(None), 12, 18),
                (Action::JumpDown(None), 12, 19),
                // Scrolloff is ignored.
                (Action::JumpUp(None), 10, 17),
            ],
        );

        // We won't move past the end of the file (tested above),
        // unless we're already past it.
        viewer.top_row = 14;
        viewer.focused_row = 15;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::JumpDown(Some(2)), 14, 17),
                (Action::JumpDown(None), 14, 19),
            ],
        );
    }

    #[test]
    fn test_move_focus() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);
        viewer.dimensions.height = 5;
        viewer.scrolloff_setting = 1;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveFocusedLineToTop, 0, 0),
                (Action::MoveFocusedLineToCenter, 0, 0),
                (Action::MoveFocusedLineToBottom, 0, 0),
            ],
        );

        viewer.top_row = 10;
        viewer.focused_row = 12;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveFocusedLineToTop, 11, 12),
                (Action::MoveFocusedLineToCenter, 10, 12),
                (Action::MoveFocusedLineToBottom, 9, 12),
            ],
        );

        viewer.top_row = 4;
        viewer.focused_row = 6;
        viewer.dimensions.height = 7;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveFocusedLineToTop, 5, 6),
                (Action::MoveFocusedLineToCenter, 3, 6),
                (Action::MoveFocusedLineToBottom, 1, 6),
            ],
        );

        viewer.dimensions.height = 8;

        assert_window_tracking(
            &mut viewer,
            vec![
                (Action::MoveFocusedLineToTop, 5, 6),
                (Action::MoveFocusedLineToCenter, 2, 6),
                (Action::MoveFocusedLineToBottom, 0, 6),
            ],
        );
    }

    #[test]
    fn test_click_row() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);
        viewer.dimensions.height = 7;
        viewer.scrolloff_setting = 3;

        // Clicked on closing brace; doesn't collapse object
        assert_window_tracking(&mut viewer, vec![(Action::Click(6), 2, 5)]);
        assert!(viewer.flatjson[5].is_expanded());

        assert_window_tracking(&mut viewer, vec![(Action::Click(1), 0, 2)]);
        assert!(viewer.flatjson[2].is_collapsed());

        assert_window_tracking(&mut viewer, vec![(Action::Click(3), 0, 2)]);
        assert!(viewer.flatjson[2].is_expanded());

        assert_window_tracking(&mut viewer, vec![(Action::Click(5), 1, 4)]);
    }

    #[test]
    fn test_focus_prev_next_sibling_line_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        viewer.focused_row = 0;
        viewer.desired_depth = 0;
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusNextSibling(1), 12),
                (Action::FocusNextSibling(1), 12),
                (Action::FocusPrevSibling(1), 0),
                (Action::FocusPrevSibling(1), 0),
            ],
        );

        viewer.flatjson.collapse(0);
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusNextSibling(1), 0),
                (Action::FocusPrevSibling(1), 0),
            ],
        );

        viewer.flatjson.expand(0);

        viewer.focused_row = 0;
        viewer.desired_depth = 1;
        assert_movements(
            &mut viewer,
            vec![
                // Go all the way down
                (Action::FocusNextSibling(1), 1),
                (Action::FocusNextSibling(2), 5),
                (Action::FocusNextSibling(1), 6),
                (Action::FocusNextSibling(1), 10),
                (Action::FocusNextSibling(2), 12),
                (Action::FocusNextSibling(1), 12),
                // And all the way back up
                (Action::FocusPrevSibling(3), 6),
                (Action::FocusPrevSibling(4), 0),
                (Action::FocusPrevSibling(1), 0),
            ],
        );

        viewer.focused_row = 2;
        viewer.flatjson.collapse(2);
        assert_movements(
            &mut viewer,
            vec![
                // Skip closing brace of 2
                (Action::FocusNextSibling(1), 6),
                (Action::FocusNextSibling(1), 10),
                // And all the way back up
                (Action::FocusPrevSibling(1), 6),
                (Action::FocusPrevSibling(1), 2),
            ],
        );
    }

    #[test]
    fn test_focus_prev_next_sibling_data_mode() {
        let fj = parse_top_level_json(DATA_OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Data);

        viewer.focused_row = 0;
        viewer.desired_depth = 0;
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusNextSibling(1), 0),
                (Action::FocusPrevSibling(1), 0),
            ],
        );

        viewer.flatjson.collapse(0);
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusNextSibling(1), 0),
                (Action::FocusPrevSibling(1), 0),
            ],
        );

        viewer.flatjson.expand(0);

        viewer.focused_row = 0;
        viewer.desired_depth = 1;
        assert_movements(
            &mut viewer,
            vec![
                // Go all the way down
                (Action::FocusNextSibling(3), 6),
                (Action::FocusNextSibling(1), 11),
                (Action::FocusNextSibling(1), 11),
                // And all the way back up
                (Action::FocusPrevSibling(1), 6),
                (Action::FocusPrevSibling(3), 0),
                (Action::FocusPrevSibling(1), 0),
            ],
        );

        viewer.focused_row = 2;
        viewer.flatjson.collapse(2);
        assert_movements(
            &mut viewer,
            vec![
                // Skip closing brace of 2
                (Action::FocusNextSibling(1), 6),
                (Action::FocusNextSibling(1), 11),
                (Action::FocusPrevSibling(1), 6),
                (Action::FocusPrevSibling(1), 2),
            ],
        );
    }

    #[test]
    fn test_focus_first_last_sibling() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        // Check top level navigation.
        viewer.focused_row = 0;
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusFirstSibling, 0),
                (Action::FocusLastSibling, 0),
                (Action::FocusFirstSibling, 0),
            ],
        );

        viewer.focused_row = 2;
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusFirstSibling, 1),
                (Action::FocusLastSibling, 11),
                (Action::FocusFirstSibling, 1),
            ],
        );
        viewer.focused_row = 2;
        assert_movements(&mut viewer, vec![(Action::FocusLastSibling, 11)]);

        viewer.focused_row = 8;
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusLastSibling, 9),
                (Action::FocusFirstSibling, 7),
            ],
        );
    }

    #[test]
    fn test_focus_top_and_bottom() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);
        viewer.dimensions.height = 8;

        assert_window_tracking(
            &mut viewer,
            vec![(Action::FocusBottom, 5, 12), (Action::FocusTop, 0, 0)],
        );

        viewer.mode = Mode::Data;

        assert_window_tracking(
            &mut viewer,
            vec![(Action::FocusBottom, 2, 11), (Action::FocusTop, 0, 0)],
        );
    }

    #[test]
    fn test_focus_bottom_newline_delimited_json() {
        let nd_json = r#"
            0
            1
            2
            {
                "a": 4
            }
        "#;

        let fj = parse_top_level_json(nd_json.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        assert_window_tracking(
            &mut viewer,
            vec![(Action::FocusBottom, 0, 5), (Action::FocusTop, 0, 0)],
        );
        viewer.flatjson.collapse(3);
        assert_window_tracking(
            &mut viewer,
            vec![(Action::FocusBottom, 0, 3), (Action::FocusTop, 0, 0)],
        );

        viewer.mode = Mode::Data;
        viewer.flatjson.expand(3);

        assert_window_tracking(
            &mut viewer,
            vec![(Action::FocusBottom, 0, 4), (Action::FocusTop, 0, 0)],
        );
        viewer.flatjson.collapse(3);
        assert_window_tracking(&mut viewer, vec![(Action::FocusBottom, 0, 3)]);
    }

    #[test]
    fn test_focus_matching_pair() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        viewer.focused_row = 0;
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusMatchingPair, 12),
                (Action::FocusMatchingPair, 0),
            ],
        );

        viewer.focused_row = 5;
        assert_movements(
            &mut viewer,
            vec![
                (Action::FocusMatchingPair, 2),
                (Action::FocusMatchingPair, 5),
            ],
        );

        // Don't jump to closing brace if current node is collapsed.
        viewer.flatjson.collapse(6);
        viewer.focused_row = 6;
        assert_movements(&mut viewer, vec![(Action::FocusMatchingPair, 6)]);
    }

    const LOTS_OF_OBJECTS: &str = r#"{
        "1": {
            "2": 2
        },
        "4": [
            {
                "6": 6
            },
            {
                "9": 9
            }
        ],
        "12": {
            "13": 13
        }
    }"#;

    #[test]
    fn test_collapse_and_expand_node_and_siblings() {
        let fj = parse_top_level_json(LOTS_OF_OBJECTS.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        viewer.dimensions.height = 8;
        viewer.scrolloff_setting = 1;

        // This top_row will become invisible after collapsing.
        viewer.top_row = 6;
        viewer.focused_row = 8;

        viewer.perform_action(Action::ExpandNodeAndSiblings);
        assert!(viewer.flatjson[5].is_expanded());
        assert!(viewer.flatjson[8].is_expanded());

        viewer.focused_row = 10;
        viewer.perform_action(Action::CollapseNodeAndSiblings);
        // Make sure top_row gets updated to a visible row.
        assert_eq!(5, viewer.top_row);
        assert_eq!(8, viewer.focused_row);
        assert!(viewer.flatjson[5].is_collapsed());
        assert!(viewer.flatjson[8].is_collapsed());

        viewer.flatjson.collapse(12);

        // This top_row is the closing brace of a node that is
        // about to be collapsed.
        viewer.top_row = 3;

        viewer.focused_row = 12;
        viewer.perform_action(Action::CollapseNodeAndSiblings);
        assert_eq!(1, viewer.top_row);
        assert!(viewer.flatjson[1].is_collapsed());
        assert!(viewer.flatjson[4].is_collapsed());
        assert!(viewer.flatjson[12].is_collapsed());

        viewer.perform_action(Action::ExpandNodeAndSiblings);
        assert!(viewer.flatjson[1].is_expanded());
        assert!(viewer.flatjson[4].is_expanded());
        assert!(viewer.flatjson[12].is_expanded());
    }

    #[test]
    fn test_toggle_mode() {
        let fj = parse_top_level_json(LOTS_OF_OBJECTS.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, Mode::Line);

        viewer.dimensions.height = 5;
        viewer.scrolloff_setting = 1;

        let tests = vec![
            (Mode::Data, 0, 0, 0, 0),
            (Mode::Line, 0, 0, 0, 0),
            (Mode::Data, 2, 4, 3, 4), // Closing brace appears above focus
            (Mode::Line, 2, 4, 1, 4), // Closing brace disappears above focus
            // Focused on a closing brace
            (Mode::Line, 7, 11, 5, 12),
            // Focused on a closing brace at end of file
            (Mode::Line, 12, 15, 8, 13),
        ];

        for (i, (mode, start_top, start_focused, end_top, end_focused)) in
            tests.into_iter().enumerate()
        {
            viewer.mode = mode;
            viewer.top_row = start_top;
            viewer.focused_row = start_focused;
            viewer.perform_action(Action::ToggleMode);

            assert_eq!(
                viewer.focused_row,
                end_focused,
                "Incorrect focused_row after test {}",
                i + 1
            );
            assert_eq!(
                viewer.top_row,
                end_top,
                "Incorrect top_row after test {}",
                i + 1
            );
        }
    }

    #[track_caller]
    fn assert_window_tracking(
        viewer: &mut JsonViewer,
        actions_and_rows: Vec<(Action, usize, usize)>,
    ) {
        for (i, (action, top_row, focused_row)) in actions_and_rows.into_iter().enumerate() {
            viewer.perform_action(action);
            assert_eq!(
                viewer.focused_row,
                focused_row,
                "Incorrect focused_row after {} actions",
                i + 1
            );
            assert_eq!(
                viewer.top_row,
                top_row,
                "Incorrect top_row after {} actions",
                i + 1
            );
        }
    }
}
