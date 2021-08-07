use super::flatjson::{FlatJson, Index, OptionIndex};

#[derive(PartialEq, Eq, Copy, Clone, Debug)]
pub enum Mode {
    Line,
    Data,
}

pub struct JsonViewer {
    flatjson: FlatJson,
    top_row: Index,
    focused_row: Index,

    height: usize,
    mode: Mode,
}

pub enum Action {
    Up(usize),
    Down(usize),
    Left,
    Right,

    ToggleCollapsed,

    FocusFirstElem,
    FocusLastElem,
    FocusTop,
    FocusBottom,

    ScrollUp(usize),
    ScrollDown(usize),

    ToggleMode,
}

impl JsonViewer {
    // NOTE: Does NOT update top_row to ensure focused_row is visible.
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

    // NOTE: Does NOT update top_row to ensure focused_row is visible.
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

        if let OptionIndex::Index(parent) = self.flatjson[self.focused_row].parent {
            self.focused_row = parent;
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
}
