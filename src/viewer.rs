use super::flatjson::{parse_top_level_json, FlatJson, Index, OptionIndex};

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

impl JsonViewer {
    fn new(flatjson: FlatJson, height: usize, mode: Mode) -> JsonViewer {
        JsonViewer {
            flatjson,
            top_row: 0,
            focused_row: 0,
            height,
            mode,
        }
    }
}

#[derive(Debug, Copy, Clone)]
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
    fn perform_action(&mut self, action: Action) {
        match action {
            Action::Up(n) => self.move_up(n),
            Action::Down(n) => self.move_down(n),
            Action::Left => self.move_left(),
            Action::Right => self.move_right(),
            Action::ToggleMode => self.toggle_mode(),
            _ => {}
        }
    }
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

    fn toggle_mode(&mut self) {
        self.mode = match self.mode {
            Mode::Line => Mode::Data,
            Mode::Data => Mode::Line,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: &'static str = r#"{
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

    #[test]
    fn test_move_up_down_line_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, 10, Mode::Line);

        assert_movements(
            &mut viewer,
            vec![(Action::Down(1), 1), (Action::Down(2), 3)],
        );

        viewer.flatjson.collapse(6);
        viewer.focused_row = 6;

        assert_movements(
            &mut viewer,
            vec![
                (Action::Down(1), 11),
                (Action::Down(1), 12),
                (Action::Down(1), 12),
            ],
        );

        assert_movements(
            &mut viewer,
            vec![
                (Action::Up(2), 6),
                (Action::Up(1), 5),
                (Action::Up(5), 0),
                (Action::Up(2), 0),
            ],
        );

        viewer.flatjson.collapse(0);
        assert_movements(&mut viewer, vec![(Action::Up(1), 0), (Action::Down(1), 0)]);
    }

    #[test]
    fn test_move_up_down_data_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, 10, Mode::Data);

        assert_movements(
            &mut viewer,
            vec![
                (Action::Down(1), 1),
                (Action::Down(3), 4),
                (Action::Down(1), 6),
            ],
        );

        viewer.flatjson.collapse(6);

        assert_movements(
            &mut viewer,
            vec![(Action::Down(1), 11), (Action::Down(1), 11)],
        );

        assert_movements(
            &mut viewer,
            vec![
                (Action::Up(1), 6),
                (Action::Up(3), 2),
                (Action::Up(1), 1),
                (Action::Up(1), 0),
                (Action::Up(1), 0),
            ],
        );
    }

    #[test]
    fn test_move_left_right_line_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, 10, Mode::Line);

        assert_movements(
            &mut viewer,
            vec![
                (Action::Right, 1),
                (Action::Right, 1),
                (Action::Down(1), 2),
                (Action::Right, 3),
                (Action::Left, 2),
                (Action::Left, 2),
            ],
        );

        assert!(viewer.flatjson[2].is_collapsed());

        viewer.focused_row = 10;
        assert_movements(
            &mut viewer,
            vec![
                // Right on closing brace takes you to previous line
                (Action::Right, 9),
                (Action::Left, 6),
                (Action::Down(4), 10),
                // Collapsing while on closing brace takes you to opening brace
                (Action::Left, 6),
            ],
        );

        assert!(viewer.flatjson[6].is_collapsed());

        assert_movements(
            &mut viewer,
            vec![(Action::Left, 0), (Action::Left, 0), (Action::Down(1), 0)],
        );

        assert!(viewer.flatjson[0].is_collapsed());
        assert_movements(&mut viewer, vec![(Action::Right, 0)]);

        assert!(viewer.flatjson[0].is_expanded());
        assert_movements(&mut viewer, vec![(Action::Right, 1)]);
    }

    #[test]
    fn test_move_left_right_data_mode() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut viewer = JsonViewer::new(fj, 10, Mode::Data);

        assert_movements(
            &mut viewer,
            vec![
                (Action::Right, 1),
                (Action::Right, 1),
                (Action::Down(5), 7),
                (Action::Left, 6),
                (Action::Left, 6),
            ],
        );

        assert!(viewer.flatjson[6].is_collapsed());

        assert_movements(
            &mut viewer,
            vec![
                (Action::Left, 0),
                (Action::Right, 1),
                (Action::Left, 0),
                (Action::Left, 0),
            ],
        );

        assert!(viewer.flatjson[0].is_collapsed());
        assert_movements(&mut viewer, vec![(Action::Down(1), 0), (Action::Right, 0)]);

        assert!(viewer.flatjson[0].is_expanded());
        assert_movements(&mut viewer, vec![(Action::Left, 0)]);
    }

    fn assert_movements(viewer: &mut JsonViewer, actions_and_focuses: Vec<(Action, Index)>) {
        for (i, (action, expected_focused_row)) in actions_and_focuses.into_iter().enumerate() {
            viewer.perform_action(action);
            assert_eq!(
                viewer.focused_row, expected_focused_row,
                "expected row {} to be focused after {} actions (last action: {:?})",
                expected_focused_row, i, action,
            );
        }
    }
}
