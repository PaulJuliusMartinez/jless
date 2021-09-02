use std::fmt;
use std::fmt::Write;

use crate::flatjson::{FlatJson, Index, Row};
use crate::truncate::truncate_right_to_fit;
use crate::truncate::TruncationResult::{DoesntFit, NoTruncation, Truncated};
use crate::tuicontrol::{Color, TUIControl};
use crate::viewer::Mode;

// # Printing out individual lines
//
// 1. Compute depth
//   - Need to consider indentation_reduction
//
// [LINE MODE ONLY]
// 1.5. Print focus mode indicator
//
// 2. Position cursor after indentation
//
// [DATA MODE ONLY]
// 2.5 Print expanded/collapsed icon for containers
//
// 3. Print Object key, if it exists
//
// [DATA MODE ONLY]
// 3.5 Print array indices
//
// 4. Print actual object
//   - Need to print previews of collapsed arrays/objects
//
// [LINE MODE ONLY]
// 4.5 Print trailing comma
//
//
//
// Truncate long value first:
//     key: "long text her…" |
//
// Truncate long key so we can still show some of value
//
//     really_rea…: "long …" |
//
// Should always show trailing '{' after long keys of objects
//
//     really_long_key_h…: { |
//
// If something is just totally off the screen, show >
//
//                   a: [    |
//                     [38]:>|
//                       d: >|
//                         X:|
//                          >|
//                          >|
//                     […]: >|    Use ellipsis in array index?
//                   abcd…: >|
//
//
// Required characters:
// '"' before and after key (0 or 2)
// "[…]" Array index (>= 3)
// ": " after key (== 2)
// '{' / '[' opening brace (1)
// ',' in line mode (1)
//
//
// […]          :       "hello"
// abc…         :        123        ,
// "de…"                  {         ,
// KEY/INDEX  <chars>  OBJECT   <chars>
//
// Don't abbreviate true/false/null ?
//
// v [1]: >
// v a…: >
//
//                  abc: tr… |
//                  a…: true |
//
// First truncate value down to 5 + ellipsis characters
// - Then truncate key down to 3 + ellipsis characters
//   - Or index down to just ellipsis
// Then truncate value down to 1 + ellipsis
// Then pop sections off, and slap " >" on it once it fits
//
//
//
//
// Sections:
//
// Key { quoted: bool, key: &str }
// Index { index: &str }
//
// KVColon
//
// OpenBraceValue(ch)
// CloseBraceValue(ch)
// Null
// True,
// False,
// Number,
// StringValue,
//
// TrailingComma
//
// Line {
//   label: Option<Key { quoted: bool, key: &str } | Index { index: usize }>
//   value:
//     ContainerChar(ch) |
//     Value {
//       v: &str,
//       ellipsis: bool,
//       quotes: bool
//     }
//     Preview {} // Build these up starting at ...
//   trailing_comma: bool
// }
//
// truncate(value, min_length: 5)
// truncate(label, min_length: 3)
// truncate(value, min_length: 1)
//
// print label if available_space
// print KVColon if available_space
// print " >"

const FOCUSED_LINE: &'static str = "▶ ";
const FOCUSED_COLLAPSED_CONTAINER: &'static str = "▶ ";
const FOCUSED_EXPANDED_CONTAINER: &'static str = "▼ ";
const COLLAPSED_CONTAINER: &'static str = "▷ ";
const EXPANDED_CONTAINER: &'static str = "▽ ";
const INDICATOR_WIDTH: usize = 2;

const LABEL_COLOR: Color = Color::LightBlue;
const FOCUSED_LABEL_COLOR: Color = Color::Blue;
const FOCUSED_LABEL_BG_COLOR: Color = Color::LightWhite;
const DIMMED: Color = Color::LightBlack;

pub enum LineLabel<'a> {
    Key { quoted: bool, key: &'a str },
    Index { index: &'a str },
}

#[derive(Eq, PartialEq)]
enum LabelStyle {
    None,
    Quote,
    Square,
}

impl LabelStyle {
    fn left(&self) -> &'static str {
        match self {
            LabelStyle::None => "",
            LabelStyle::Quote => "\"",
            LabelStyle::Square => "[",
        }
    }

    fn right(&self) -> &'static str {
        match self {
            LabelStyle::None => "",
            LabelStyle::Quote => "\"",
            LabelStyle::Square => "]",
        }
    }

    fn width(&self) -> usize {
        match self {
            LabelStyle::None => 0,
            _ => 2,
        }
    }
}

pub enum LineValue<'a> {
    Container {
        flatjson: &'a FlatJson,
        row: &'a Row,
        index: Index,
    },
    Value {
        s: &'a str,
        quotes: bool,
        color: Color,
    },
}

pub struct LinePrinter<'a, TUI: TUIControl> {
    pub mode: Mode,
    pub tui: TUI,

    // Do I need these?
    pub depth: usize,
    pub width: usize,

    pub tab_size: usize,

    // Line-by-line formatting options
    pub focused: bool,
    pub secondarily_focused: bool,
    pub trailing_comma: bool,

    // Stuff to actually print out
    pub label: Option<LineLabel<'a>>,
    pub value: LineValue<'a>,
}

impl<'a, TUI: TUIControl> LinePrinter<'a, TUI> {
    pub fn print_line<W: Write>(&self, buf: &mut W) -> fmt::Result {
        self.tui.reset_style(buf)?;

        self.print_focus_and_container_indicators(buf)?;

        let label_depth = INDICATOR_WIDTH + self.depth * self.tab_size;
        self.tui.position_cursor(buf, (1 + label_depth) as u16)?;

        let mut available_space = self.width.saturating_sub(label_depth);

        let space_used_for_label = self.fill_in_label(buf, available_space)?;

        available_space = available_space.saturating_sub(space_used_for_label);

        if self.label.is_some() && space_used_for_label == 0 {
            self.print_truncated_indicator(buf)?;
        } else {
            let space_used_for_value = self.fill_in_value(buf, available_space)?;

            if space_used_for_value == 0 {
                self.print_truncated_indicator(buf)?;
            }
        }

        Ok(())
    }

    fn print_focus_and_container_indicators<W: Write>(&self, buf: &mut W) -> fmt::Result {
        match self.mode {
            Mode::Line => self.print_focused_line_indicator(buf),
            Mode::Data => self.print_container_indicator(buf),
        }
    }

    fn print_focused_line_indicator<W: Write>(&self, buf: &mut W) -> fmt::Result {
        if self.focused {
            self.tui.position_cursor(buf, 1)?;
            write!(buf, "{}", FOCUSED_LINE)?;
        }

        Ok(())
    }

    fn print_container_indicator<W: Write>(&self, buf: &mut W) -> fmt::Result {
        // let-else would be better here.
        let collapsed = match &self.value {
            LineValue::Container { row, .. } => {
                debug_assert!(row.is_opening_of_container());
                row.is_collapsed()
            }
            _ => return Ok(()),
        };

        // Make sure there's enough room for the indicator
        if self.width <= INDICATOR_WIDTH + self.depth * self.tab_size {
            return Ok(());
        }

        let container_indicator_col = 1 + self.depth * self.tab_size;
        self.tui
            .position_cursor(buf, container_indicator_col as u16)?;

        match (self.focused, collapsed) {
            (true, true) => write!(buf, "{}", FOCUSED_COLLAPSED_CONTAINER)?,
            (true, false) => write!(buf, "{}", FOCUSED_EXPANDED_CONTAINER)?,
            (false, true) => write!(buf, "{}", COLLAPSED_CONTAINER)?,
            (false, false) => write!(buf, "{}", EXPANDED_CONTAINER)?,
        }

        Ok(())
    }

    pub fn fill_in_label<W: Write>(
        &self,
        buf: &mut W,
        mut available_space: usize,
    ) -> Result<usize, fmt::Error> {
        let label_style: LabelStyle;
        let mut label_ref: &str;
        let mut label_truncated = false;

        let mut fg_label_color = None;
        let mut bg_label_color = None;
        let mut label_bolded = false;

        let mut used_space = 0;

        match self.label {
            None => return Ok(0),
            Some(LineLabel::Key { quoted, key }) => {
                label_style = if quoted {
                    LabelStyle::Quote
                } else {
                    LabelStyle::None
                };
                label_ref = key;

                if self.focused {
                    fg_label_color = Some(FOCUSED_LABEL_COLOR);
                    bg_label_color = Some(FOCUSED_LABEL_BG_COLOR);
                } else {
                    fg_label_color = Some(LABEL_COLOR);
                }
            }
            Some(LineLabel::Index { index }) => {
                label_style = LabelStyle::Square;
                label_ref = index;

                if self.focused {
                    label_bolded = true;
                } else {
                    fg_label_color = Some(DIMMED);
                }
            }
        }

        // Remove two characters for either "" or [].
        if label_style != LabelStyle::None {
            available_space = available_space.saturating_sub(2);
        }

        // Remove two characters for ": "
        available_space = available_space.saturating_sub(2);

        // Remove one character for either ">" or a single character
        // of the value.
        available_space = available_space.saturating_sub(1);

        match truncate_right_to_fit(label_ref, available_space, "…") {
            NoTruncation(width) => {
                used_space += width;
            }
            Truncated(label_prefix, width) => {
                used_space += width;
                label_ref = label_prefix;
                label_truncated = true;
            }
            DoesntFit => {
                return Ok(0);
            }
        }

        // Actually print out the label!
        self.tui.maybe_fg_color(buf, fg_label_color)?;
        self.tui.maybe_bg_color(buf, bg_label_color)?;
        if label_bolded {
            self.tui.bold(buf)?;
        }

        write!(buf, "{}", label_style.left())?;
        write!(buf, "{}", label_ref)?;
        if label_truncated {
            write!(buf, "…")?;
        }
        write!(buf, "{}", label_style.right())?;

        self.tui.reset_style(buf)?;
        write!(buf, ": ")?;

        used_space += label_style.width();
        used_space += 2;

        Ok(used_space)
    }

    fn fill_in_value<W: Write>(
        &self,
        buf: &mut W,
        mut available_space: usize,
    ) -> Result<usize, fmt::Error> {
        // Object values are sufficiently complicated that we'll handle them
        // in a separate function.
        if let LineValue::Container {
            flatjson,
            index,
            row,
        } = self.value
        {
            return self.fill_in_container_value(buf, available_space, flatjson, index, row);
        }

        let mut value_ref: &str;
        let mut value_truncated = false;
        let quoted: bool;
        let color: Color;

        match self.value {
            LineValue::Value {
                s,
                quotes,
                color: c,
            } => {
                value_ref = s;
                quoted = quotes;
                color = c;
            }
            LineValue::Container { .. } => panic!("We just eliminated the Container case above"),
        }

        let mut used_space = 0;

        if quoted {
            available_space = available_space.saturating_sub(2);
        }

        if self.trailing_comma {
            available_space = available_space.saturating_sub(1);
        }

        match truncate_right_to_fit(value_ref, available_space, "…") {
            NoTruncation(width) => {
                used_space += width;
            }
            Truncated(value_prefix, width) => {
                used_space += width;
                value_ref = value_prefix;
                value_truncated = true;
            }
            DoesntFit => {
                return Ok(0);
            }
        }

        // Print out the value.
        self.tui.fg_color(buf, color)?;
        if quoted {
            used_space += 1;
            buf.write_char('"')?;
        }
        write!(buf, "{}", value_ref)?;
        if value_truncated {
            buf.write_char('…')?;
        }
        if quoted {
            used_space += 1;
            buf.write_char('"')?;
        }

        if self.trailing_comma {
            used_space += 1;
            self.tui.reset_style(buf)?;
            buf.write_char(',')?;
        }

        Ok(used_space)
    }

    // Print out an object value on a line. There are three main variables at
    // play here that determine what we should print out: the viewer mode,
    // whether we're at the start or end of the container, and whether the
    // container is expanded or collapsed. Whether the line is focused also
    // determines the style in which the line is printed, but doesn't affect
    // what actually gets printed.
    //
    // These are the 8 cases:
    //
    // Mode | Start/End |   State   |     Displayed
    // -----+-----------+-----------+---------------------------
    // Line |   Start   | Expanded  | Open char
    // Line |   Start   | Collapsed | Preview + trailing comma?
    // Line |    End    | Expanded  | Close char + trailing comma?
    // Line |    End    | Collapsed | IMPOSSIBLE
    // Data |   Start   | Expanded  | Preview
    // Data |   Start   | Collapsed | Preview + trailing comma?
    // Data |    End    | Expanded  | IMPOSSIBLE
    // Data |    End    | Collapsed | IMPOSSIBLE
    fn fill_in_container_value<W: Write>(
        &self,
        buf: &mut W,
        mut available_space: usize,
        flatjson: &FlatJson,
        index: Index,
        row: &Row,
    ) -> Result<usize, fmt::Error> {
        debug_assert!(row.is_container());

        let mode = self.mode;
        let side = row.is_opening_of_container();
        let expanded_state = row.is_expanded();

        const LINE: Mode = Mode::Line;
        const DATA: Mode = Mode::Data;
        const OPEN: bool = true;
        const CLOSE: bool = false;
        const EXPANDED: bool = true;
        const COLLAPSED: bool = false;

        match (mode, side, expanded_state) {
            (LINE, OPEN, EXPANDED) => self.fill_in_container_open_char(buf, available_space, row),
            (LINE, CLOSE, EXPANDED) => self.fill_in_container_close_char(buf, available_space, row),
            (LINE, OPEN, COLLAPSED) | (DATA, OPEN, EXPANDED) | (DATA, OPEN, COLLAPSED) => {
                self.fill_in_container_preview(buf, available_space, flatjson, index, row)
            }
            // Impossible states
            (LINE, CLOSE, COLLAPSED) => panic!("Can't focus closing of collapsed container"),
            (DATA, CLOSE, _) => panic!("Can't focus closing of container in Data mode"),
        }
    }

    fn fill_in_container_open_char<W: Write>(
        &self,
        buf: &mut W,
        available_space: usize,
        row: &Row,
    ) -> Result<usize, fmt::Error> {
        if available_space > 0 {
            if self.focused || self.secondarily_focused {
                self.tui.bold(buf)?;
            }
            buf.write_char(row.value.container_type().unwrap().open_char())?;
            Ok(1)
        } else {
            Ok(0)
        }
    }

    fn fill_in_container_close_char<W: Write>(
        &self,
        buf: &mut W,
        available_space: usize,
        row: &Row,
    ) -> Result<usize, fmt::Error> {
        let needed_space = if self.trailing_comma { 2 } else { 1 };

        if available_space >= needed_space {
            if self.focused || self.secondarily_focused {
                self.tui.bold(buf)?;
            }
            buf.write_char(row.value.container_type().unwrap().close_char())?;

            if self.trailing_comma {
                self.tui.reset_style(buf)?;
                buf.write_char(',')?;
            }

            Ok(needed_space)
        } else {
            Ok(0)
        }
    }

    fn fill_in_container_preview<W: Write>(
        &self,
        buf: &mut W,
        available_space: usize,
        _flatjson: &FlatJson,
        _index: Index,
        row: &Row,
    ) -> Result<usize, fmt::Error> {
        let type_str = row.value.container_type().unwrap().type_str();
        let mut needed_space = type_str.len();

        if self.trailing_comma {
            needed_space += 1;
        }

        if available_space >= needed_space {
            if !self.focused {
                self.tui.fg_color(buf, DIMMED)?;
            }

            write!(buf, "{}", type_str)?;

            if self.trailing_comma {
                self.tui.reset_style(buf)?;
                buf.write_char(',')?;
            }

            Ok(needed_space)
        } else {
            Ok(0)
        }
    }

    fn print_truncated_indicator<W: Write>(&self, buf: &mut W) -> fmt::Result {
        self.tui.position_cursor(buf, self.width as u16)?;
        if self.focused {
            self.tui.reset_style(buf)?;
            self.tui.bold(buf)?;
        } else {
            self.tui.fg_color(buf, DIMMED)?;
        }
        write!(buf, ">")
    }
}

#[cfg(test)]
mod tests {
    use crate::flatjson::parse_top_level_json;
    use crate::tuicontrol::test::{EmptyControl, VisibleEscapes};

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

    impl<'a, TUI: Default + TUIControl> Default for LinePrinter<'a, TUI> {
        fn default() -> LinePrinter<'a, TUI> {
            LinePrinter {
                mode: Mode::Data,
                tui: TUI::default(),
                depth: 0,
                width: 100,
                tab_size: 2,
                focused: false,
                secondarily_focused: false,
                trailing_comma: false,
                label: None,
                value: LineValue::Value {
                    s: "hello",
                    quotes: true,
                    color: Color::White,
                },
            }
        }
    }

    #[test]
    fn test_line_mode_focus_indicators() -> std::fmt::Result {
        let mut line: LinePrinter<'_, VisibleEscapes> = LinePrinter {
            mode: Mode::Line,
            tui: VisibleEscapes::position_only(),
            depth: 1,
            value: LineValue::Value {
                s: "null",
                quotes: false,
                color: Color::White,
            },
            ..LinePrinter::default()
        };

        let mut buf = String::new();
        line.print_line(&mut buf)?;

        assert_eq!(format!("_C(5)_null"), buf);

        line.focused = true;
        line.depth = 3;
        line.tab_size = 1;

        buf.clear();
        line.print_line(&mut buf)?;

        assert_eq!(format!("_C(1)_{}_C(6)_null", FOCUSED_LINE), buf);

        Ok(())
    }

    #[test]
    fn test_data_mode_focus_indicators() -> std::fmt::Result {
        let mut fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        let mut line: LinePrinter<'_, VisibleEscapes> = LinePrinter {
            tui: VisibleEscapes::position_only(),
            value: LineValue::Container {
                flatjson: &fj,
                index: 0,
                row: &fj[0],
            },
            ..LinePrinter::default()
        };

        let mut buf = String::new();
        line.depth = 1;
        line.print_line(&mut buf)?;

        assert_eq!(format!("_C(3)_{}_C(5)_Object", EXPANDED_CONTAINER), buf);

        line.focused = true;

        buf.clear();
        line.print_line(&mut buf)?;

        assert_eq!(
            format!("_C(3)_{}_C(5)_Object", FOCUSED_EXPANDED_CONTAINER),
            buf
        );

        fj.collapse(0);
        // Need to create a new LinePrinter so I can modify fj on the line above.
        line = LinePrinter {
            tui: VisibleEscapes::position_only(),
            depth: 2,
            tab_size: 4,
            value: LineValue::Container {
                flatjson: &fj,
                index: 0,
                row: &fj[0],
            },
            ..LinePrinter::default()
        };

        buf.clear();
        line.print_line(&mut buf)?;

        assert_eq!(format!("_C(9)_{}_C(11)_Object", COLLAPSED_CONTAINER), buf);

        line.focused = true;

        buf.clear();
        line.print_line(&mut buf)?;

        assert_eq!(
            format!("_C(9)_{}_C(11)_Object", FOCUSED_COLLAPSED_CONTAINER),
            buf
        );

        Ok(())
    }

    #[test]
    fn test_fill_label_basic() -> std::fmt::Result {
        let mut line: LinePrinter<'_, VisibleEscapes> = LinePrinter {
            label: Some(LineLabel::Key {
                quoted: true,
                key: "hello",
            }),
            ..LinePrinter::default()
        };

        let mut buf = String::new();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!(format!("_FG({:?})_\"hello\"_R_: ", LABEL_COLOR), buf);
        assert_eq!(9, used_space);

        line.focused = true;
        line.label = Some(LineLabel::Key {
            quoted: false,
            key: "hello",
        });

        buf.clear();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!(
            format!(
                "_FG({:?})__BG({:?})_hello_R_: ",
                FOCUSED_LABEL_COLOR, FOCUSED_LABEL_BG_COLOR
            ),
            buf
        );
        assert_eq!(7, used_space);

        line.label = Some(LineLabel::Index { index: "12345" });

        buf.clear();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!("_B_[12345]_R_: ", buf);
        assert_eq!(9, used_space);

        line.focused = false;
        buf.clear();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!(format!("_FG({:?})_[12345]_R_: ", DIMMED), buf);
        assert_eq!(9, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_label_not_enough_space() -> std::fmt::Result {
        let mut line: LinePrinter<'_, EmptyControl> = LinePrinter::default();
        line.label = Some(LineLabel::Key {
            quoted: true,
            key: "hello",
        });

        // QUOTED STRING KEY

        // Minimum space is: '"h…": ', which has a length of 6, plus extra space for value char.
        let mut buf = String::new();

        let used_space = line.fill_in_label(&mut buf, 7)?;
        assert_eq!("\"h…\": ", buf);
        assert_eq!(6, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_label(&mut buf, 6)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        // UNQUOTED STRING KEY

        // Minimum space is: "h…: ", which has a length of 4, plus extra space for value char.
        line.label = Some(LineLabel::Key {
            quoted: false,
            key: "hello",
        });

        buf.clear();

        let used_space = line.fill_in_label(&mut buf, 5)?;
        assert_eq!("h…: ", buf);
        assert_eq!(4, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_label(&mut buf, 4)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        // ARRAY INDEX

        // Minimum space is: "[…5]: ", which has a length of 6, plus extra space for value char.
        line.label = Some(LineLabel::Index { index: "12345" });

        buf.clear();

        let used_space = line.fill_in_label(&mut buf, 7)?;
        assert_eq!("[1…]: ", buf);
        assert_eq!(6, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_label(&mut buf, 6)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_value_basic() -> std::fmt::Result {
        let color = Color::Black;
        let mut line: LinePrinter<'_, VisibleEscapes> = LinePrinter {
            value: LineValue::Value {
                s: "hello",
                quotes: true,
                color,
            },
            ..LinePrinter::default()
        };

        let mut buf = String::new();
        let used_space = line.fill_in_value(&mut buf, 100)?;

        assert_eq!("_FG(Black)_\"hello\"", buf);
        assert_eq!(7, used_space);

        line.trailing_comma = true;
        line.value = LineValue::Value {
            s: "null",
            quotes: false,
            color,
        };

        buf.clear();
        let used_space = line.fill_in_value(&mut buf, 100)?;

        assert_eq!("_FG(Black)_null_R_,", buf);
        assert_eq!(5, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_value_not_enough_space() -> std::fmt::Result {
        let mut line: LinePrinter<'_, EmptyControl> = LinePrinter::default();
        let color = Color::Black;

        // QUOTED VALUE

        // Minimum space is: '"h…"', which has a length of 4.
        line.value = LineValue::Value {
            s: "hello",
            quotes: true,
            color,
        };

        let mut buf = String::new();

        let used_space = line.fill_in_value(&mut buf, 4)?;
        assert_eq!("\"h…\"", buf);
        assert_eq!(4, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_value(&mut buf, 3)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        // UNQUOTED VALUE, TRAILING COMMA

        // Minimum space is: 't…,', which has a length of 3.
        line.trailing_comma = true;
        line.value = LineValue::Value {
            s: "true",
            quotes: false,
            color,
        };

        let mut buf = String::new();

        let used_space = line.fill_in_value(&mut buf, 3)?;
        assert_eq!("t…,", buf);
        assert_eq!(3, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_value(&mut buf, 2)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        Ok(())
    }
}
