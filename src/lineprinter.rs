use std::collections::hash_map::Entry;
use std::fmt;
use std::fmt::Write;
use std::iter::Peekable;
use std::ops::Range;

use regex::Regex;

use crate::flatjson::{FlatJson, OptionIndex, Row, Value};
use crate::highlighting;
use crate::highlighting::{ColorPrinter, PrintStyle};
use crate::search::MatchRangeIter;
use crate::truncate::TruncationResult::{DoesntFit, NoTruncation, Truncated};
use crate::truncate::{min_required_columns_for_str, truncate_right_to_fit};
use crate::truncatedstrview::TruncatedStrView;
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

lazy_static::lazy_static! {
    pub static ref JS_IDENTIFIER: Regex = Regex::new("^[_$a-zA-Z][_$a-zA-Z0-9]*$").unwrap();
}

pub enum LineLabel<'a> {
    Key { key: &'a str },
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

    fn width(&self) -> isize {
        match self {
            LabelStyle::None => 0,
            _ => 2,
        }
    }
}

#[derive(Debug)]
pub enum LineValue<'a> {
    Container {
        flatjson: &'a FlatJson,
        row: &'a Row,
    },
    Value {
        s: &'a str,
        quotes: bool,
        color: Color,
    },
}

pub struct LinePrinter<'a, 'b, 'c, TUI: TUIControl> {
    pub mode: Mode,
    pub tui: TUI,
    pub printer: ColorPrinter<'c, TUI, String>,

    // Do I need these?
    pub depth: usize,
    pub width: usize,

    pub tab_size: usize,

    // Line-by-line formatting options
    pub focused: bool,
    pub focused_because_matching_container_pair: bool,
    pub trailing_comma: bool,

    // Stuff to actually print out
    pub label: Option<LineLabel<'a>>,
    pub label_range: &'a Option<Range<usize>>,
    pub value: LineValue<'a>,
    pub value_range: &'a Range<usize>,

    pub search_matches: Option<Peekable<MatchRangeIter<'b>>>,

    pub cached_formatted_value: Option<Entry<'a, usize, TruncatedStrView>>,
}

impl<'a, 'b, 'c, TUI: TUIControl> LinePrinter<'a, 'b, 'c, TUI> {
    pub fn print_line(&mut self) -> fmt::Result {
        self.tui.reset_style(self.printer.buf)?;

        self.print_focus_and_container_indicators()?;

        let label_depth = INDICATOR_WIDTH + self.depth * self.tab_size;
        self.tui
            .position_cursor(self.printer.buf, (1 + label_depth) as u16)?;

        let mut available_space = self.width as isize - label_depth as isize;

        let space_used_for_label = self.fill_in_label(available_space)?;

        available_space -= space_used_for_label;

        if self.label.is_some() && space_used_for_label == 0 {
            self.print_truncated_indicator()?;
        } else {
            let space_used_for_value = self.fill_in_value(available_space)?;

            if space_used_for_value == 0 {
                self.print_truncated_indicator()?;
            }
        }

        Ok(())
    }

    fn print_focus_and_container_indicators(&mut self) -> fmt::Result {
        match self.mode {
            Mode::Line => self.print_focused_line_indicator(),
            Mode::Data => self.print_container_indicator(),
        }
    }

    fn print_focused_line_indicator(&mut self) -> fmt::Result {
        if self.focused {
            self.tui.position_cursor(self.printer.buf, 1)?;
            write!(self.printer.buf, "{}", FOCUSED_LINE)?;
        }

        Ok(())
    }

    fn print_container_indicator(&mut self) -> fmt::Result {
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
            .position_cursor(self.printer.buf, container_indicator_col as u16)?;

        let indicator = match (self.focused, collapsed) {
            (true, true) => FOCUSED_COLLAPSED_CONTAINER,
            (true, false) => FOCUSED_EXPANDED_CONTAINER,
            (false, true) => COLLAPSED_CONTAINER,
            (false, false) => EXPANDED_CONTAINER,
        };

        write!(self.printer.buf, "{}", indicator)?;

        Ok(())
    }

    pub fn fill_in_label(&mut self, mut available_space: isize) -> Result<isize, fmt::Error> {
        let label_style: LabelStyle;
        let label_ref: &str;

        let mut used_space = 0;

        let style: &PrintStyle;
        let highlighted_style: &PrintStyle;
        let mut dummy_search_matches = None;
        let matches_iter;

        match self.label {
            None => return Ok(0),
            Some(LineLabel::Key { key }) => {
                // Quote keys in line mode or if they're not valid JS identifiers.
                let should_be_quoted = self.mode == Mode::Line || !JS_IDENTIFIER.is_match(key);
                label_style = if should_be_quoted {
                    LabelStyle::Quote
                } else {
                    LabelStyle::None
                };

                label_ref = key;

                if self.focused {
                    style = &highlighting::INVERTED_BOLD_BLUE_STYLE;
                    highlighted_style = &highlighting::BOLD_INVERTED_STYLE;
                } else {
                    style = &highlighting::BLUE_STYLE;
                    highlighted_style = &highlighting::SEARCH_MATCH_HIGHLIGHTED;
                }
                matches_iter = &mut self.search_matches;
            }
            Some(LineLabel::Index { index }) => {
                label_style = LabelStyle::Square;
                label_ref = index;

                if self.focused {
                    style = &highlighting::BOLD_STYLE;
                } else {
                    style = &highlighting::GRAY_STYLE;
                }

                // No match highlighting for index labels.
                matches_iter = &mut dummy_search_matches;
                highlighted_style = &highlighting::DEFAULT_STYLE;
            }
        }

        // Remove two characters for either "" or [].
        available_space -= label_style.width();

        // Remove two characters for ": "
        available_space -= 2;

        // Remove one character for either ">" or a single character
        // of the value.
        available_space -= 1;

        let truncated_view = TruncatedStrView::init_start(label_ref, available_space);
        let space_used_for_label = truncated_view.used_space();
        if space_used_for_label.is_none() {
            return Ok(0);
        }
        let space_used_for_label = space_used_for_label.unwrap();

        used_space += space_used_for_label;

        let mut label_open_delimiter_range_start = None;
        let mut label_range_start = None;
        let mut label_close_delimiter_range_start = None;
        let mut object_separator_range_start = None;

        if let Some(range) = self.label_range {
            label_open_delimiter_range_start = Some(range.start);
            label_range_start = Some(range.start + 1);
            label_close_delimiter_range_start = Some(range.end - 1);
            object_separator_range_start = Some(range.end);
        }

        let mut matches = matches_iter.as_mut();
        // Print out start of label
        highlighting::highlight_matches(
            &mut self.printer,
            label_style.left(),
            label_open_delimiter_range_start,
            style,
            highlighted_style,
            &mut matches,
        )?;

        // Print out the label itself
        highlighting::highlight_truncated_str_view(
            &mut self.printer,
            label_ref,
            &truncated_view,
            label_range_start,
            style,
            highlighted_style,
            &mut matches,
        )?;

        // Print out end of label
        highlighting::highlight_matches(
            &mut self.printer,
            label_style.right(),
            label_close_delimiter_range_start,
            style,
            highlighted_style,
            &mut matches,
        )?;

        // Print out separator between label and value
        highlighting::highlight_matches(
            &mut self.printer,
            ": ",
            object_separator_range_start,
            &highlighting::DEFAULT_STYLE,
            &highlighting::SEARCH_MATCH_HIGHLIGHTED,
            &mut matches,
        )?;

        used_space += label_style.width();
        used_space += 2;

        Ok(used_space)
    }

    fn fill_in_value(&mut self, mut available_space: isize) -> Result<isize, fmt::Error> {
        // Object values are sufficiently complicated that we'll handle them
        // in a separate function.
        if let LineValue::Container { flatjson, row } = self.value {
            return self.fill_in_container_value(available_space, flatjson, row);
        }

        let value_ref: &str;
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
            available_space -= 2;
        }

        if self.trailing_comma {
            available_space -= 1;
        }

        // Option<Entry<>>
        // - if no entry, then init_start
        // - if entry, but vacant, then init_start
        // - if entry with value, then value and resize
        let truncated_view = self
            .cached_formatted_value
            .take()
            .map(|entry| {
                entry
                    .and_modify(|tsv| {
                        *tsv = tsv.resize(value_ref, available_space);
                    })
                    .or_insert_with(|| TruncatedStrView::init_start(value_ref, available_space))
                    .clone()
            })
            .unwrap_or_else(|| TruncatedStrView::init_start(value_ref, available_space));

        let space_used_for_value = truncated_view.used_space();
        if space_used_for_value.is_none() {
            return Ok(0);
        }
        let space_used_for_value = space_used_for_value.unwrap();
        used_space += space_used_for_value;

        // If we are just going to show a single ellipsis, we want
        // to show a '>' instead.
        if truncated_view.is_completely_elided() && !quoted && !self.trailing_comma {
            return Ok(0);
        }

        // Print out the value.
        let style = PrintStyle {
            fg: color,
            ..PrintStyle::default()
        };

        let value_open_quote_range_start = self.value_range.start;
        let mut value_range_start = self.value_range.start;
        let value_close_quote_range_start = self.value_range.end - 1;
        let trailing_comma_range_start = self.value_range.end;

        if quoted {
            used_space += 1;
            value_range_start += 1;
            highlighting::highlight_matches(
                &mut self.printer,
                "\"",
                Some(value_open_quote_range_start),
                &style,
                &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                &mut self.search_matches.as_mut(),
            )?;
        }

        highlighting::highlight_truncated_str_view(
            &mut self.printer,
            value_ref,
            &truncated_view,
            Some(value_range_start),
            &style,
            &highlighting::SEARCH_MATCH_HIGHLIGHTED,
            &mut self.search_matches.as_mut(),
        )?;

        if quoted {
            used_space += 1;
            highlighting::highlight_matches(
                &mut self.printer,
                "\"",
                Some(value_close_quote_range_start),
                &style,
                &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                &mut self.search_matches.as_mut(),
            )?;
        }

        if self.trailing_comma {
            used_space += 1;
            highlighting::highlight_matches(
                &mut self.printer,
                ",",
                Some(trailing_comma_range_start),
                &highlighting::DEFAULT_STYLE,
                &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                &mut self.search_matches.as_mut(),
            )?;
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
    fn fill_in_container_value(
        &mut self,
        available_space: isize,
        flatjson: &FlatJson,
        row: &Row,
    ) -> Result<isize, fmt::Error> {
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
            (LINE, OPEN, EXPANDED) => self.fill_in_container_open_char(available_space, row),
            (LINE, CLOSE, EXPANDED) => self.fill_in_container_close_char(available_space, row),
            (LINE, OPEN, COLLAPSED) | (DATA, OPEN, EXPANDED) | (DATA, OPEN, COLLAPSED) => {
                self.fill_in_container_preview(available_space, flatjson, row)
            }
            // Impossible states
            (LINE, CLOSE, COLLAPSED) => panic!("Can't focus closing of collapsed container"),
            (DATA, CLOSE, _) => panic!("Can't focus closing of container in Data mode"),
        }
    }

    fn fill_in_container_open_char(
        &mut self,
        available_space: isize,
        row: &Row,
    ) -> Result<isize, fmt::Error> {
        if available_space > 0 {
            let style = if self.focused || self.focused_because_matching_container_pair {
                &highlighting::BOLD_STYLE
            } else {
                &highlighting::DEFAULT_STYLE
            };

            highlighting::highlight_matches(
                &mut self.printer,
                row.value.container_type().unwrap().open_str(),
                Some(self.value_range.start),
                style,
                &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                &mut self.search_matches.as_mut(),
            )?;

            Ok(1)
        } else {
            Ok(0)
        }
    }

    fn fill_in_container_close_char(
        &mut self,
        available_space: isize,
        row: &Row,
    ) -> Result<isize, fmt::Error> {
        let needed_space = if self.trailing_comma { 2 } else { 1 };

        if available_space >= needed_space {
            let style = if self.focused || self.focused_because_matching_container_pair {
                &highlighting::BOLD_STYLE
            } else {
                &highlighting::DEFAULT_STYLE
            };

            highlighting::highlight_matches(
                &mut self.printer,
                row.value.container_type().unwrap().close_str(),
                Some(self.value_range.start),
                style,
                &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                &mut self.search_matches.as_mut(),
            )?;

            if self.trailing_comma {
                highlighting::highlight_matches(
                    &mut self.printer,
                    ",",
                    Some(self.value_range.end),
                    &highlighting::DEFAULT_STYLE,
                    &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                    &mut self.search_matches.as_mut(),
                )?;
            }

            Ok(needed_space)
        } else {
            Ok(0)
        }
    }

    fn fill_in_container_preview(
        &mut self,
        mut available_space: isize,
        flatjson: &FlatJson,
        row: &Row,
    ) -> Result<isize, fmt::Error> {
        if self.trailing_comma {
            available_space -= 1;
        }

        let quoted_object_keys = self.mode == Mode::Line;
        let mut used_space =
            self.generate_container_preview(flatjson, row, available_space, quoted_object_keys)?;

        if self.trailing_comma {
            used_space += 1;
            if self.trailing_comma {
                highlighting::highlight_matches(
                    &mut self.printer,
                    ",",
                    Some(self.value_range.end),
                    &highlighting::DEFAULT_STYLE,
                    &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                    &mut self.search_matches.as_mut(),
                )?;
            }
        }

        Ok(used_space)
    }

    fn generate_container_preview(
        &mut self,
        flatjson: &FlatJson,
        row: &Row,
        mut available_space: isize,
        quoted_object_keys: bool,
    ) -> Result<isize, fmt::Error> {
        debug_assert!(row.is_opening_of_container());

        // Minimum amount of space required == 3: […]
        if available_space < 3 {
            return Ok(0);
        }

        let container_type = row.value.container_type().unwrap();
        available_space -= 2;
        let mut num_printed = 0;

        // Create a copy of self.search_matches
        let original_search_matches = self.search_matches.clone();

        highlighting::highlight_matches(
            &mut self.printer,
            container_type.open_str(),
            Some(self.value_range.start),
            &highlighting::GRAY_STYLE,
            &highlighting::GRAY_INVERTED_STYLE,
            &mut self.search_matches.as_mut(),
        )?;

        num_printed += 1;

        let mut next_sibling = row.first_child();
        let mut is_first_child = true;
        while let OptionIndex::Index(child) = next_sibling {
            next_sibling = flatjson[child].next_sibling;

            // If there are still more elements, we'll print out ", …" at the end,
            let space_needed_at_end_of_container = if next_sibling.is_some() { 3 } else { 0 };
            let space_available_for_elem = available_space - space_needed_at_end_of_container;
            let is_only_child = is_first_child && next_sibling.is_nil();

            let used_space = self.fill_in_container_elem_preview(
                flatjson,
                &flatjson[child],
                space_available_for_elem,
                quoted_object_keys,
                is_only_child,
            )?;

            if used_space == 0 {
                // No room for anything else, let's close out the object.
                // If we're not the first child, the previous elem will have
                // printed the ", " separator.
                highlighting::highlight_matches(
                    &mut self.printer,
                    "…",
                    None,
                    &highlighting::GRAY_STYLE,
                    &highlighting::GRAY_INVERTED_STYLE,
                    &mut self.search_matches.as_mut(),
                )?;
                available_space -= 1;
                num_printed += 1;
                break;
            } else {
                // Successfully printed elem out, let's print a separator.
                if next_sibling.is_some() {
                    highlighting::highlight_matches(
                        &mut self.printer,
                        ", ",
                        Some(flatjson[child].range.end),
                        &highlighting::GRAY_STYLE,
                        &highlighting::GRAY_INVERTED_STYLE,
                        &mut self.search_matches.as_mut(),
                    )?;
                    available_space -= 2;
                    num_printed += 2;
                }
            }

            available_space -= used_space;
            num_printed += used_space;

            is_first_child = false;
        }

        highlighting::highlight_matches(
            &mut self.printer,
            container_type.close_str(),
            Some(self.value_range.end - 1),
            &highlighting::GRAY_STYLE,
            &highlighting::GRAY_INVERTED_STYLE,
            &mut self.search_matches.as_mut(),
        )?;
        num_printed += 1;

        self.search_matches = original_search_matches;

        Ok(num_printed)
    }

    // {a…: …, …}
    //
    // [a, …]
    fn fill_in_container_elem_preview(
        &mut self,
        flatjson: &FlatJson,
        row: &Row,
        mut available_space: isize,
        quoted_object_keys: bool,
        is_only_child: bool,
    ) -> Result<isize, fmt::Error> {
        let mut used_space = 0;

        if let Some(key_range) = &row.key_range {
            let key_ref = &flatjson.1[key_range.start + 1..key_range.end - 1];
            // Need at least one character for value, and two characters for ": "
            let mut space_available_for_key = available_space - 3;
            let mut quoted_object_key = quoted_object_keys;

            if quoted_object_keys || !JS_IDENTIFIER.is_match(key_ref) {
                space_available_for_key -= 2;
                quoted_object_key = true;
            }

            let truncated_view = TruncatedStrView::init_start(key_ref, space_available_for_key);
            let space_used_for_label = truncated_view.used_space();
            if space_used_for_label.is_none() || truncated_view.is_completely_elided() {
                return Ok(0);
            }

            let space_used_for_label = space_used_for_label.unwrap();
            used_space += space_used_for_label;
            available_space -= space_used_for_label;

            if quoted_object_key {
                used_space += 1;
                available_space -= 1;
                highlighting::highlight_matches(
                    &mut self.printer,
                    "\"",
                    Some(key_range.start),
                    &highlighting::GRAY_STYLE,
                    &highlighting::GRAY_INVERTED_STYLE,
                    &mut self.search_matches.as_mut(),
                )?;
            }

            highlighting::highlight_truncated_str_view(
                &mut self.printer,
                key_ref,
                &truncated_view,
                Some(key_range.start + 1),
                &highlighting::GRAY_STYLE,
                &highlighting::GRAY_INVERTED_STYLE,
                &mut self.search_matches.as_mut(),
            )?;

            if quoted_object_key {
                used_space += 1;
                available_space -= 1;
                highlighting::highlight_matches(
                    &mut self.printer,
                    "\"",
                    Some(key_range.end - 1),
                    &highlighting::GRAY_STYLE,
                    &highlighting::GRAY_INVERTED_STYLE,
                    &mut self.search_matches.as_mut(),
                )?;
            }

            used_space += 2;
            available_space -= 2;
            highlighting::highlight_matches(
                &mut self.printer,
                ": ",
                Some(key_range.end),
                &highlighting::GRAY_STYLE,
                &highlighting::GRAY_INVERTED_STYLE,
                &mut self.search_matches.as_mut(),
            )?;
        }

        let space_used_for_value = if is_only_child && row.value.is_container() {
            self.generate_container_preview(flatjson, &row, available_space, quoted_object_keys)?
        } else {
            self.fill_in_value_preview(&flatjson.1, &row, available_space)?
        };
        used_space += space_used_for_value;

        // Make sure to print out ellipsis for the value if we printed out an
        // object key, but couldn't print out the value. Space was already
        // allocated for this at the start of the function.
        if row.key_range.is_some() && space_used_for_value == 0 {
            self.printer.buf.write_char('…')?;
            used_space += 1;
        }

        Ok(used_space)
    }

    fn fill_in_value_preview(
        &mut self,
        pretty_printed_json: &str,
        row: &Row,
        mut available_space: isize,
    ) -> Result<isize, fmt::Error> {
        let mut quoted = false;
        let mut can_be_truncated = true;
        let mut showing_collapsed_preview = false;

        let value_ref = match &row.value {
            Value::OpenContainer { container_type, .. } => {
                can_be_truncated = false;
                showing_collapsed_preview = true;
                container_type.collapsed_preview()
            }
            Value::CloseContainer { .. } => panic!("CloseContainer cannot be child value."),
            Value::String => {
                quoted = true;
                let range = row.range.clone();
                &pretty_printed_json[range.start + 1..range.end - 1]
            }
            _ => &pretty_printed_json[row.range.clone()],
        };

        if quoted {
            available_space -= 2;
        }

        let space_used_for_quotes = if quoted { 2 } else { 0 };

        let truncated_view = TruncatedStrView::init_start(value_ref, available_space);
        let space_used_for_value = truncated_view.used_space();

        // TODO: We SHOULD check if is_completely_elided, but only for Array values.
        if space_used_for_value.is_none() {
            return Ok(0);
        }

        if !can_be_truncated && truncated_view.range.unwrap().is_truncated(value_ref) {
            return Ok(0);
        }

        let value_open_quote_range_start = row.range.start;
        let mut value_range_start = row.range.start;
        let value_close_quote_range_start = row.range.end - 1;

        if quoted {
            value_range_start += 1;
            highlighting::highlight_matches(
                &mut self.printer,
                "\"",
                Some(value_open_quote_range_start),
                &highlighting::GRAY_STYLE,
                &highlighting::GRAY_INVERTED_STYLE,
                &mut self.search_matches.as_mut(),
            )?;
        }

        highlighting::highlight_truncated_str_view(
            &mut self.printer,
            value_ref,
            &truncated_view,
            // Technically could try to highlight open and close delimiters
            // of the collapsed container, but not really worth it right now.
            if showing_collapsed_preview {
                None
            } else {
                Some(value_range_start)
            },
            &highlighting::GRAY_STYLE,
            &highlighting::GRAY_INVERTED_STYLE,
            &mut self.search_matches.as_mut(),
        )?;

        if quoted {
            highlighting::highlight_matches(
                &mut self.printer,
                "\"",
                Some(value_close_quote_range_start),
                &highlighting::GRAY_STYLE,
                &highlighting::GRAY_INVERTED_STYLE,
                &mut self.search_matches.as_mut(),
            )?;
        }

        Ok(space_used_for_quotes + space_used_for_value.unwrap())
    }

    fn print_truncated_indicator(&mut self) -> fmt::Result {
        self.tui
            .position_cursor(self.printer.buf, self.width as u16)?;
        if self.focused {
            self.tui.reset_style(self.printer.buf)?;
            self.tui.bold(self.printer.buf)?;
        } else {
            self.tui.fg_color(self.printer.buf, DIMMED)?;
        }
        write!(self.printer.buf, ">")
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use crate::flatjson::parse_top_level_json2;
    use crate::tuicontrol::test::{EmptyControl, VisibleEscapes};

    use super::*;

    type I<'a> = std::slice::Iter<'a, Range<usize>>;

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

    impl<'a, 'b, TUI: Default + TUIControl> Default for LinePrinter<'a, 'b, TUI> {
        fn default() -> LinePrinter<'a, 'b, TUI> {
            LinePrinter {
                mode: Mode::Data,
                tui: TUI::default(),
                depth: 0,
                width: 100,
                tab_size: 2,
                focused: false,
                focused_because_matching_container_pair: false,
                trailing_comma: false,
                label: None,
                label_start_index: 0,
                value: LineValue::Value {
                    s: "hello",
                    quotes: true,
                    color: Color::White,
                },
                value_start_index: 0,
                search_matches: None,
                cached_formatted_value: None,
            }
        }
    }

    #[test]
    fn test_line_mode_focus_indicators() -> std::fmt::Result {
        let mut line: LinePrinter<'_, '_, VisibleEscapes> = LinePrinter {
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
        let mut fj = parse_top_level_json2(OBJECT.to_owned()).unwrap();
        let mut line: LinePrinter<'_, '_, VisibleEscapes> = LinePrinter {
            tui: VisibleEscapes::position_only(),
            value: LineValue::Container {
                flatjson: &fj,
                row: &fj[0],
            },
            ..LinePrinter::default()
        };

        let mut buf = String::new();
        line.depth = 1;
        line.print_line(&mut buf)?;

        let expected_prefix = format!("_C(3)_{}_C(5)_{{", EXPANDED_CONTAINER);
        assert_starts_with(&buf, &expected_prefix);

        line.focused = true;

        buf.clear();
        line.print_line(&mut buf)?;

        let expected_prefix = format!("_C(3)_{}_C(5)_{{", FOCUSED_EXPANDED_CONTAINER);
        assert_starts_with(&buf, &expected_prefix);

        fj.collapse(0);
        // Need to create a new LinePrinter so I can modify fj on the line above.
        line = LinePrinter {
            tui: VisibleEscapes::position_only(),
            depth: 2,
            tab_size: 4,
            value: LineValue::Container {
                flatjson: &fj,
                row: &fj[0],
            },
            ..LinePrinter::default()
        };

        buf.clear();
        line.print_line(&mut buf)?;

        let expected_prefix = format!("_C(9)_{}_C(11)_{{", COLLAPSED_CONTAINER);
        assert_starts_with(&buf, &expected_prefix);

        line.focused = true;

        buf.clear();
        line.print_line(&mut buf)?;

        let expected_prefix = format!("_C(9)_{}_C(11)_{{", FOCUSED_COLLAPSED_CONTAINER);
        assert_starts_with(&buf, &expected_prefix);

        Ok(())
    }

    #[test]
    fn test_fill_key_label_basic() -> std::fmt::Result {
        let mut line: LinePrinter<'_, '_, VisibleEscapes> = LinePrinter {
            mode: Mode::Line,
            label: Some(LineLabel::Key { key: "hello" }),
            ..LinePrinter::default()
        };

        let mut buf = String::new();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!(format!("_FG({:?})_\"hello\"_R_: ", LABEL_COLOR), buf);
        assert_eq!(9, used_space);

        line.focused = true;
        line.mode = Mode::Data;
        line.label = Some(LineLabel::Key { key: "hello" });

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

        // Non JS identifiers (including empty-string) get quoted.
        line.label = Some(LineLabel::Key { key: "" });

        buf.clear();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!(
            format!(
                "_FG({:?})__BG({:?})_\"\"_R_: ",
                FOCUSED_LABEL_COLOR, FOCUSED_LABEL_BG_COLOR
            ),
            buf
        );
        assert_eq!(4, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_index_label_basic() -> std::fmt::Result {
        let mut line: LinePrinter<'_, '_, VisibleEscapes> = LinePrinter {
            label: Some(LineLabel::Index { index: "12345" }),
            ..LinePrinter::default()
        };

        let mut buf = String::new();

        let used_space = line.fill_in_label(&mut buf, 100)?;
        assert_eq!(format!("_FG({:?})_[12345]_R_: ", DIMMED), buf);
        assert_eq!(9, used_space);

        line.focused = true;
        buf.clear();
        let used_space = line.fill_in_label(&mut buf, 100)?;

        assert_eq!("_B_[12345]_R_: ", buf);
        assert_eq!(9, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_label_not_enough_space() -> std::fmt::Result {
        let mut line: LinePrinter<'_, '_, EmptyControl> = LinePrinter::default();
        line.mode = Mode::Line;
        line.label = Some(LineLabel::Key { key: "hello" });

        // QUOTED STRING KEY

        // Minimum space is: '"h…": ', which has a length of 6, plus extra space for value char.
        let mut buf = String::new();

        let used_space = line.fill_in_label(&mut buf, 7)?;
        assert_eq!("\"h…\": ", buf);
        assert_eq!(6, used_space);

        buf.clear();

        // Elide the whole key
        let used_space = line.fill_in_label(&mut buf, 6)?;
        assert_eq!("\"…\": ", buf);
        assert_eq!(5, used_space);

        buf.clear();

        // Can't fit at all
        let used_space = line.fill_in_label(&mut buf, 5)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        // UNQUOTED STRING KEY

        // Minimum space is: "h…: ", which has a length of 4, plus extra space for value char.
        line.mode = Mode::Data;
        line.label = Some(LineLabel::Key { key: "hello" });

        buf.clear();

        let used_space = line.fill_in_label(&mut buf, 5)?;
        assert_eq!("h…: ", buf);
        assert_eq!(4, used_space);

        buf.clear();

        // Elide the whole key.
        let used_space = line.fill_in_label(&mut buf, 4)?;
        assert_eq!("…: ", buf);
        assert_eq!(3, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_label(&mut buf, 3)?;
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
        assert_eq!("[…]: ", buf);
        assert_eq!(5, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_label(&mut buf, 5)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_value_basic() -> std::fmt::Result {
        let color = Color::White;
        let mut line: LinePrinter<'_, '_, VisibleEscapes> = LinePrinter {
            value: LineValue::Value {
                s: "hello",
                quotes: true,
                color,
            },
            ..LinePrinter::default()
        };

        let mut buf = String::new();
        let used_space = line.fill_in_value(&mut buf, 100)?;

        assert_eq!("_FG(White)__BG(Black)_\"hello\"", buf);
        assert_eq!(7, used_space);

        line.trailing_comma = true;
        line.value = LineValue::Value {
            s: "null",
            quotes: false,
            color,
        };

        buf.clear();
        let used_space = line.fill_in_value(&mut buf, 100)?;

        assert_eq!("_FG(White)__BG(Black)_null_R_,", buf);
        assert_eq!(5, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_value_not_enough_space() -> std::fmt::Result {
        let mut line: LinePrinter<'_, '_, EmptyControl> = LinePrinter::default();
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
        assert_eq!("\"…\"", buf);
        assert_eq!(3, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_value(&mut buf, 2)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        // QUOTED EMPTY STRING

        line.value = LineValue::Value {
            s: "",
            quotes: true,
            color,
        };

        buf.clear();
        let used_space = line.fill_in_value(&mut buf, 2)?;
        assert_eq!("\"\"", buf);
        assert_eq!(2, used_space);

        buf.clear();
        let used_space = line.fill_in_value(&mut buf, 1)?;
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

        buf.clear();

        let used_space = line.fill_in_value(&mut buf, 3)?;
        assert_eq!("t…,", buf);
        assert_eq!(3, used_space);

        buf.clear();

        let used_space = line.fill_in_value(&mut buf, 2)?;
        assert_eq!("…,", buf);
        assert_eq!(2, used_space);

        buf.clear();

        // Not enough room, returns 0.
        let used_space = line.fill_in_value(&mut buf, 1)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        // UNQUOTED VALUE, NO TRAILING COMMA
        line.trailing_comma = false;

        buf.clear();

        // Don't print just an ellipsis, print '>' instead.
        let used_space = line.fill_in_value(&mut buf, 1)?;
        assert_eq!("", buf);
        assert_eq!(0, used_space);

        Ok(())
    }

    #[test]
    fn test_generate_object_preview() -> std::fmt::Result {
        let json = r#"{"a": 1, "d": {"x": true}, "b c": null}"#;
        //            {"a": 1, "d": {…}, "b c": null}
        //           01234567890123456789012345678901 (31 characters)
        //            {a: 1, d: {…}, "b c": null}
        //           0123456789012345678901234567 (27 characters)
        let fj = parse_top_level_json2(json.to_owned()).unwrap();

        for (available_space, used_space, quoted_object_keys, expected) in vec![
            (50, 31, true, r#"{"a": 1, "d": {…}, "b c": null}"#),
            (50, 27, false, r#"{a: 1, d: {…}, "b c": null}"#),
            (26, 26, false, r#"{a: 1, d: {…}, "b c": nu…}"#),
            (25, 25, false, r#"{a: 1, d: {…}, "b c": n…}"#),
            (24, 24, false, r#"{a: 1, d: {…}, "b c": …}"#),
            (23, 23, false, r#"{a: 1, d: {…}, "b…": …}"#),
            (22, 17, false, r#"{a: 1, d: {…}, …}"#),
            (16, 15, false, r#"{a: 1, d: …, …}"#),
            (14, 9, false, r#"{a: 1, …}"#),
            (8, 3, false, r#"{…}"#),
            (2, 0, false, r#""#),
        ]
        .into_iter()
        {
            let (buf, used) = generate_container_preview(&fj, available_space, quoted_object_keys)?;
            assert_eq!(
                expected,
                buf,
                "expected preview with {} available columns (used up {} columns)",
                available_space,
                UnicodeWidthStr::width(buf.as_str()),
            );
            assert_eq!(used_space, used);
        }

        Ok(())
    }

    #[test]
    fn test_generate_array_preview() -> fmt::Result {
        let json = r#"[1, {"x": true}, null, "hello", true]"#;
        //            [1, {…}, null, "hello", true]
        //           012345678901234567890123456789 (29 characters)
        let fj = parse_top_level_json2(json.to_owned()).unwrap();

        for (available_space, used_space, expected) in vec![
            (50, 29, r#"[1, {…}, null, "hello", true]"#),
            (28, 28, r#"[1, {…}, null, "hello", tr…]"#),
            (27, 27, r#"[1, {…}, null, "hello", t…]"#),
            (26, 26, r#"[1, {…}, null, "hello", …]"#),
            (25, 25, r#"[1, {…}, null, "hel…", …]"#),
            (24, 24, r#"[1, {…}, null, "he…", …]"#),
            (23, 23, r#"[1, {…}, null, "h…", …]"#),
            (22, 17, r#"[1, {…}, null, …]"#),
            (16, 16, r#"[1, {…}, nu…, …]"#),
            (15, 15, r#"[1, {…}, n…, …]"#),
            (14, 11, r#"[1, {…}, …]"#),
            (10, 6, r#"[1, …]"#),
            (5, 3, r#"[…]"#),
            (2, 0, r#""#),
        ]
        .into_iter()
        {
            let quoted_object_keys = false;
            let (buf, used) = generate_container_preview(&fj, available_space, quoted_object_keys)?;
            assert_eq!(
                expected,
                buf,
                "expected preview with {} available columns (used up {} columns)",
                available_space,
                UnicodeWidthStr::width(buf.as_str()),
            );
            assert_eq!(used_space, used);
        }

        Ok(())
    }

    #[test]
    fn test_generate_container_preview_single_container_child() -> fmt::Result {
        let json = r#"{"a": [1, {"x": true}, null, "hello", true]}"#;
        //            {a: [1, {…}, null, "hello", true]}
        //           01234567890123456789012345678901234 (34 characters)
        let fj = parse_top_level_json2(json.to_owned()).unwrap();

        let (buf, used) = generate_container_preview(&fj, 34, false)?;
        assert_eq!(r#"{a: [1, {…}, null, "hello", true]}"#, buf);
        assert_eq!(34, used);

        let (buf, used) = generate_container_preview(&fj, 33, false)?;
        assert_eq!(r#"{a: [1, {…}, null, "hello", tr…]}"#, buf);
        assert_eq!(33, used);

        let json = r#"[{"a": 1, "d": {"x": true}, "b c": null}]"#;
        //            [{a: 1, d: {…}, "b c": null}]
        //           012345678901234567890123456789 (29 characters)
        let fj = parse_top_level_json2(json.to_owned()).unwrap();

        let (buf, used) = generate_container_preview(&fj, 29, false)?;
        assert_eq!(r#"[{a: 1, d: {…}, "b c": null}]"#, buf);
        assert_eq!(29, used);

        let (buf, used) = generate_container_preview(&fj, 28, false)?;
        assert_eq!(r#"[{a: 1, d: {…}, "b c": nu…}]"#, buf);
        assert_eq!(28, used);

        Ok(())
    }

    fn generate_container_preview(
        flatjson: &FlatJson,
        available_space: isize,
        quoted_object_keys: bool,
    ) -> Result<(String, isize), fmt::Error> {
        let mut buf = String::new();
        let used = LinePrinter::<EmptyControl>::generate_container_preview(
            &mut buf,
            flatjson,
            &flatjson[0],
            available_space,
            quoted_object_keys,
        )?;
        Ok((buf, used))
    }

    #[track_caller]
    fn assert_starts_with(s: &str, prefix: &str) {
        assert!(
            s.starts_with(prefix),
            "Expected {} to start with {}",
            s,
            prefix,
        );
    }
}
