use std::fmt;
use std::fmt::Write;

use crate::flatjson::Value;
use crate::richformatter::{Color, RichFormatter};
use crate::truncate::truncate_right_to_fit;
use crate::truncate::TruncationResult::{DoesntFit, NoTruncation, Truncated};
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
    ContainerChar {
        ch: char,
        collapsed: bool,
    },
    Value {
        s: &'a str,
        quotes: bool,
        color: Color,
    },
}

impl<'a> LineValue<'a> {
    pub fn from_json_value(value: &'a Value) -> LineValue<'a> {
        unimplemented!();
    }
}

pub struct Line<'a, F: RichFormatter> {
    pub mode: Mode,
    pub formatter: F, // RichFormatter

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

impl<'a, F: RichFormatter> Line<'a, F> {
    pub fn print_line<W: Write>(&self, buf: &mut W) -> fmt::Result {
        self.print_focus_and_container_indicators(buf)?;

        let label_depth = INDICATOR_WIDTH + self.depth * self.tab_size;
        self.formatter
            .position_cursor(buf, (1 + label_depth) as u16)?;

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
            self.formatter.position_cursor(buf, 1)?;
            write!(buf, "{}", FOCUSED_LINE)?;
        }

        Ok(())
    }

    fn print_container_indicator<W: Write>(&self, buf: &mut W) -> fmt::Result {
        // let-else would be better here.
        let collapsed = match &self.value {
            LineValue::ContainerChar { collapsed: c, .. } => c,
            _ => return Ok(()),
        };

        // Make sure there's enough room for the indicator
        if self.width <= INDICATOR_WIDTH + self.depth * self.tab_size {
            return Ok(());
        }

        let container_indicator_col = 1 + self.depth * self.tab_size;
        self.formatter
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
                    fg_label_color = Some(Color::Blue);
                    bg_label_color = Some(Color::LightWhite);
                } else {
                    fg_label_color = Some(Color::LightBlue);
                }
            }
            Some(LineLabel::Index { index }) => {
                label_style = LabelStyle::Square;
                label_ref = index;

                if self.focused {
                    label_bolded = true;
                } else {
                    fg_label_color = Some(Color::LightBlack);
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
        self.formatter.maybe_fg_color(buf, fg_label_color)?;
        self.formatter.maybe_bg_color(buf, bg_label_color)?;
        if label_bolded {
            self.formatter.bold(buf)?;
        }

        write!(buf, "{}", label_style.left())?;
        write!(buf, "{}", label_ref)?;
        if label_truncated {
            write!(buf, "…")?;
        }
        write!(buf, "{}", label_style.right())?;

        self.formatter.reset_style(buf)?;
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
        let mut value_ref: &str;
        let mut value_truncated = false;
        let quoted: bool;
        let color: Color;

        match self.value {
            LineValue::ContainerChar { .. } => return Ok(0),
            LineValue::Value {
                s,
                quotes,
                color: c,
            } => {
                value_ref = s;
                quoted = quotes;
                color = c;
            }
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
        self.formatter.fg_color(buf, color)?;
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

        // Be a good citizen and reset the style.
        self.formatter.reset_style(buf)?;

        if self.trailing_comma {
            used_space += 1;
            buf.write_char(',')?;
        }

        Ok(used_space)
    }

    fn print_truncated_indicator<W: Write>(&self, buf: &mut W) -> fmt::Result {
        self.formatter.position_cursor(buf, self.width as u16)?;
        self.formatter.fg_color(buf, Color::LightBlack)?;
        write!(buf, ">")
    }
}

#[cfg(test)]
mod tests {
    use crate::richformatter::test::NoFormatting;

    use super::*;

    impl<'a, F: Default + RichFormatter> Default for Line<'a, F> {
        fn default() -> Line<'a, F> {
            Line {
                mode: Mode::Data,
                formatter: F::default(),
                depth: 0,
                width: 100,
                tab_size: 2,
                focused: false,
                secondarily_focused: false,
                trailing_comma: false,
                label: None,
                value: LineValue::ContainerChar {
                    ch: '{',
                    collapsed: false,
                },
            }
        }
    }

    #[test]
    #[ignore]
    fn test_focus_indicators() {
        let line: Line<'_, NoFormatting> = Line {
            mode: Mode::Line,
            ..Line::default()
        };
        let mut buf = String::new();
        line.print_line(&mut buf);

        assert_eq!(format!("{}{{", FOCUSED_LINE), buf);
    }

    #[test]
    fn test_fill_label_basic() -> std::fmt::Result {
        let mut line: Line<'_, NoFormatting> = Line { ..Line::default() };
        line.label = Some(LineLabel::Key {
            quoted: true,
            key: "hello",
        });

        let mut buf = String::new();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!("\"hello\": ", buf);
        assert_eq!(9, used_space);

        line.label = Some(LineLabel::Key {
            quoted: false,
            key: "hello",
        });

        buf.clear();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!("hello: ", buf);
        assert_eq!(7, used_space);

        line.label = Some(LineLabel::Index { index: "12345" });

        buf.clear();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!("[12345]: ", buf);
        assert_eq!(9, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_label_not_enough_space() -> std::fmt::Result {
        let mut line: Line<'_, NoFormatting> = Line { ..Line::default() };
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
        let mut line: Line<'_, NoFormatting> = Line { ..Line::default() };
        let color = Color::Black;

        line.value = LineValue::Value {
            s: "hello",
            quotes: true,
            color,
        };

        let mut buf = String::new();
        let used_space = line.fill_in_value(&mut buf, 100)?;

        assert_eq!("\"hello\"", buf);
        assert_eq!(7, used_space);

        line.trailing_comma = true;
        line.value = LineValue::Value {
            s: "null",
            quotes: false,
            color,
        };

        buf.clear();
        let used_space = line.fill_in_value(&mut buf, 100)?;

        assert_eq!("null,", buf);
        assert_eq!(5, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_value_not_enough_space() -> std::fmt::Result {
        let mut line: Line<'_, NoFormatting> = Line { ..Line::default() };
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
