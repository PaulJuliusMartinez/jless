use std::collections::hash_map::Entry;
use std::fmt;
use std::fmt::Write;
use std::iter::Peekable;
use std::ops::Range;

use regex::Regex;

use crate::flatjson::{FlatJson, OptionIndex, Row, Value};
use crate::highlighting;
use crate::search::MatchRangeIter;
use crate::terminal;
use crate::terminal::{Color, Style, Terminal};
use crate::truncatedstrview::TruncatedStrView;
use crate::viewer::Mode;

// This module is responsible for printing single lines of JSON to
// the screen, complete with syntax highlighting and highlighting
// of search matches.
//
// A single line is one of the following:
// - The start of a non-empty object or array
// - The end of a non-empty object or array
// - A key/value pair of an object
// - An element of an array
// - Or a top-level primitive
//
// Objects and containers can be collapsed, and at certain times
// we show previews next to containers.
//
// The viewer can be in one of two modes: Line mode, or Data mode.
// In Line mode, the goal is that the text on the screen is mostly
// valid JSON. In Data mode, we try to present a "cleaner" version
// of the data, by for example, not showing quotes around object keys
// or trailing commas.
//
// Here's the main set of differences:
//                                  Line Mode                 Data Mode
// Quotes around object keys:         Yes         Only when not a valid JS identifier
// Trailing commas:                   Yes                       No
// Object and Array previews: Only when collapsed               Always
// Array Indexes:                     No                        Yes
// Open delimiters:                   Yes                       No
// Line for closing delimiters:       Yes                       No
//
// In addition to the above behavior that depends on the current
// viewer mode, when rendering a line we also apply syntax
// highlighting and highlight search results. The currently focused
// search result is also displayed slightly differently.
//
// Great care is taken to highlight every character that is actually
// part of a match, including quotes around keys and string values,
// and even other syntax (colons, commas, and spaces). It is
// difficult to keep track of everything as the text displayed on
// the screen doesn't exactly match the source JSON. Beware of
// off-by-one errors.
//
//
// Naturally, there may be cases where an entire line does not fit
// on the screen without wrapping. Rather than implement line
// wrapping (which seems difficult), we truncate values and show
// ellipses to indicate truncated content. When printing out multiple
// values, such as the key and value of an Object entry, the index
// and element of an array, or the many container elements in an
// object preview, we fill in the available space from left to right
// while keeping track of what still needs to be displayed so we
// can show appropriate truncation indicators.
//
// Here are some examples:
//
//     key: "long text her…" |
//     medium_length_key: t… |
//
// If something is totally off the screen we just show a '>':
//
//     really_long_key_h…: > |
//                   [10…]: >|
//                     "d": >|
//                          >|

const FOCUSED_LINE: &str = "▶ ";
const NOT_FOCUSED_LINE: &str = "  ";
const FOCUSED_COLLAPSED_CONTAINER: &str = "▶ ";
const FOCUSED_EXPANDED_CONTAINER: &str = "▼ ";
const COLLAPSED_CONTAINER: &str = "▷ ";
const EXPANDED_CONTAINER: &str = "▽ ";
const INDICATOR_WIDTH: isize = 2;
const NO_FOCUSED_MATCH: Range<usize> = 0..0;

lazy_static::lazy_static! {
    pub static ref JS_IDENTIFIER: Regex = Regex::new("^[_$a-zA-Z][_$a-zA-Z0-9]*$").unwrap();
}

enum LabelType {
    Key,
    Index,
}

#[derive(Eq, PartialEq)]
enum DelimiterPair {
    None,
    Quote,
    Square,
}

impl DelimiterPair {
    fn left(&self) -> &'static str {
        match self {
            DelimiterPair::None => "",
            DelimiterPair::Quote => "\"",
            DelimiterPair::Square => "[",
        }
    }

    fn right(&self) -> &'static str {
        match self {
            DelimiterPair::None => "",
            DelimiterPair::Quote => "\"",
            DelimiterPair::Square => "]",
        }
    }

    fn width(&self) -> isize {
        match self {
            DelimiterPair::None => 0,
            _ => 2,
        }
    }
}

// What line number should be displayed
#[derive(Copy, Clone)]
pub struct LineNumber {
    pub absolute: Option<usize>,
    pub relative: Option<usize>,
    pub max_width: isize,
}

pub struct LinePrinter<'a, 'b> {
    pub mode: Mode,
    pub terminal: &'a mut dyn Terminal,

    // The entire FlatJson data structure and the specific line
    // we're printing out.
    pub flatjson: &'a FlatJson,
    pub row: &'a Row,
    pub line_number: LineNumber,

    // Width of the terminal and how much we should indent the line.
    pub width: isize,
    pub indentation: isize,

    // Line-by-line formatting options
    pub focused: bool,
    pub focused_because_matching_container_pair: bool,
    pub trailing_comma: bool,

    // For highlighting
    pub search_matches: Option<Peekable<MatchRangeIter<'b>>>,
    pub focused_search_match: &'a Range<usize>,

    // It's unfortunate that this has to be exposed publicly; it's only
    // used internally to disable the special syntax highlighting for
    // the current focused match in container previews.
    pub emphasize_focused_search_match: bool,

    // For remembering horizontal scroll positions of long lines.
    pub cached_truncated_value: Option<Entry<'a, usize, TruncatedStrView>>,
}

impl<'a, 'b> LinePrinter<'a, 'b> {
    pub fn print_line(&mut self) -> fmt::Result {
        self.terminal.reset_style()?;

        let mut available_space = self.width;

        let space_used_for_line_number = self.print_line_number(available_space)?;
        available_space -= space_used_for_line_number;

        let expected_space_used_for_indicators = INDICATOR_WIDTH + self.indentation;
        let space_used_for_indicators =
            self.print_focus_and_container_indicators(available_space)?;

        if space_used_for_indicators == expected_space_used_for_indicators {
            available_space -= space_used_for_indicators;

            let space_used_for_label = self.fill_in_label(available_space)?;
            available_space -= space_used_for_label;

            if self.has_label() && space_used_for_label == 0 {
                self.print_truncated_indicator()?;
            } else {
                let space_used_for_value = self.fill_in_value(available_space)?;

                if space_used_for_value == 0 {
                    self.print_truncated_indicator()?;
                }
            }
        } else {
            self.print_truncated_indicator()?;
        }

        Ok(())
    }

    // Absolute | Relative | Focused | Format
    // ---------+----------+---------+--------
    //     N    |     N    |    -    | Nothing
    //     Y    |     N    |    N    | Right aligned, dimmed
    //     Y    |     N    |    Y    | Right aligned, yellow
    //     N    |     Y    |    N    | Right aligned, dimmed
    //     N    |     Y    |    Y    | Right aligned, yellow
    //     Y    |     Y    |    N    | Relative, right aligned, dimmed
    //     Y    |     Y    |    Y    | Absolute, left aligned, yellow
    fn print_line_number(&mut self, available_space: isize) -> Result<isize, fmt::Error> {
        let LineNumber {
            absolute,
            relative,
            max_width,
        } = self.line_number;

        // If the line number is going to fill up all the available space (or overfill it)
        // then don't print the line number.
        if max_width + 1 >= available_space {
            return Ok(0);
        }

        let (n, style, right_aligned) = match (absolute, relative, self.focused) {
            (None, None, _) => return Ok(0),
            (Some(n), None, false) | (None, Some(n), false) | (Some(_), Some(n), false) => {
                (n, &highlighting::DIMMED_STYLE, true)
            }
            (Some(n), None, true) | (None, Some(n), true) => {
                (n, &highlighting::CURRENT_LINE_NUMBER, true)
            }
            (Some(n), Some(_), true) => (n, &highlighting::CURRENT_LINE_NUMBER, false),
        };

        self.terminal.set_style(style)?;

        if right_aligned {
            write!(self.terminal, "{: >1$}", n, max_width as usize)?;
        } else {
            write!(self.terminal, "{: <1$}", n, max_width as usize)?;
        }
        self.terminal.reset_style()?;
        write!(self.terminal, " ")?;

        Ok(max_width + 1)
    }

    fn print_focus_and_container_indicators(
        &mut self,
        mut available_space: isize,
    ) -> Result<isize, fmt::Error> {
        let mut used_space = 0;

        match self.mode {
            Mode::Line => {
                if available_space >= INDICATOR_WIDTH + 1 {
                    if self.focused {
                        write!(self.terminal, "{FOCUSED_LINE}")?;
                    } else {
                        write!(self.terminal, "{NOT_FOCUSED_LINE}")?;
                    }
                    used_space += INDICATOR_WIDTH;
                    available_space -= INDICATOR_WIDTH;

                    let space_available_for_indentation = self.indentation.min(available_space - 1);
                    used_space += space_available_for_indentation;
                    self.print_n_spaces(space_available_for_indentation)?;
                }
            }
            Mode::Data => {
                let space_available_for_indentation =
                    self.indentation.min(available_space - 1 - INDICATOR_WIDTH);
                used_space += space_available_for_indentation;
                self.print_n_spaces(space_available_for_indentation)?;

                if space_available_for_indentation == self.indentation {
                    if self.row.is_primitive() {
                        if self.focused {
                            write!(self.terminal, "{FOCUSED_LINE}")?;
                        } else {
                            write!(self.terminal, "{NOT_FOCUSED_LINE}")?;
                        }
                    } else {
                        self.print_container_indicator()?;
                    }
                    used_space += 2;
                }
            }
        }

        Ok(used_space)
    }

    fn print_n_spaces(&mut self, n: isize) -> fmt::Result {
        for _ in 0..n {
            write!(self.terminal, " ")?;
        }

        Ok(())
    }

    fn print_container_indicator(&mut self) -> fmt::Result {
        debug_assert!(self.row.is_opening_of_container());

        let collapsed = self.row.is_collapsed();

        let indicator = match (self.focused, collapsed) {
            (true, true) => FOCUSED_COLLAPSED_CONTAINER,
            (true, false) => FOCUSED_EXPANDED_CONTAINER,
            (false, true) => COLLAPSED_CONTAINER,
            (false, false) => EXPANDED_CONTAINER,
        };

        write!(self.terminal, "{indicator}")
    }

    pub fn fill_in_label(&mut self, mut available_space: isize) -> Result<isize, fmt::Error> {
        if !self.has_label() {
            return Ok(0);
        }

        let mut index_label_buffer = String::new();
        let (label_ref, label_range, delimiter) =
            self.get_label_range_and_delimiter(&mut index_label_buffer, &self.flatjson.1);

        let mut used_space = 0;
        let mut dummy_search_matches = None;

        let (style, highlighted_style) = self.get_label_styles();
        let matches_iter = if self.row.key_range.is_some() {
            &mut self.search_matches
        } else {
            &mut dummy_search_matches
        };

        // Remove two characters for either "" or [].
        available_space -= delimiter.width();

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

        if let Some(range) = label_range {
            label_open_delimiter_range_start = Some(range.start);
            label_range_start = Some(range.start + 1);
            label_close_delimiter_range_start = Some(range.end - 1);
            object_separator_range_start = Some(range.end);
        }

        let mut matches = matches_iter.as_mut();

        // Print out start of label
        highlighting::highlight_matches(
            self.terminal,
            delimiter.left(),
            label_open_delimiter_range_start,
            style,
            highlighted_style,
            &mut matches,
            self.focused_search_match,
        )?;

        // Print out the label itself
        highlighting::highlight_truncated_str_view(
            self.terminal,
            label_ref,
            &truncated_view,
            label_range_start,
            style,
            highlighted_style,
            &mut matches,
            self.focused_search_match,
        )?;

        // Print out end of label
        highlighting::highlight_matches(
            self.terminal,
            delimiter.right(),
            label_close_delimiter_range_start,
            style,
            highlighted_style,
            &mut matches,
            self.focused_search_match,
        )?;

        // Print out separator between label and value
        highlighting::highlight_matches(
            self.terminal,
            ": ",
            object_separator_range_start,
            &highlighting::DEFAULT_STYLE,
            &highlighting::SEARCH_MATCH_HIGHLIGHTED,
            &mut matches,
            self.focused_search_match,
        )?;

        used_space += delimiter.width();
        used_space += 2;

        Ok(used_space)
    }

    // Check if a line has a label. A line has a label if it has
    // a key, or if we are in data mode and we have a parent.
    fn has_label(&self) -> bool {
        self.row.key_range.is_some() || (self.mode == Mode::Data && self.row.parent.is_some())
    }

    // Get the type of a label, either Key or Index.
    fn label_type(&self) -> LabelType {
        debug_assert!(self.has_label());

        if self.row.key_range.is_some() {
            LabelType::Key
        } else {
            LabelType::Index
        }
    }

    fn get_label_range_and_delimiter<'l, 'fj: 'l>(
        &self,
        label: &'l mut String,
        pretty_printed: &'fj str,
    ) -> (&'l str, Option<Range<usize>>, DelimiterPair) {
        debug_assert!(self.has_label());

        if let Some(key_range) = &self.row.key_range {
            let key_without_delimiter = &pretty_printed[key_range.start + 1..key_range.end - 1];
            let key_open_delimiter = &pretty_printed[key_range.start..key_range.start + 1];

            let mut delimiter = DelimiterPair::None;

            if key_open_delimiter == "[" {
                delimiter = DelimiterPair::Square;
            } else if self.mode == Mode::Line || !JS_IDENTIFIER.is_match(key_without_delimiter) {
                delimiter = DelimiterPair::Quote;
            }

            (key_without_delimiter, Some(key_range.clone()), delimiter)
        } else {
            let parent = self.row.parent.unwrap();
            debug_assert!(self.flatjson[parent].is_array());

            write!(label, "{}", self.row.index_in_parent).unwrap();

            (label.as_str(), None, DelimiterPair::Square)
        }
    }

    fn get_label_styles(&self) -> (&'static Style, &'static Style) {
        match self.label_type() {
            LabelType::Key => {
                if self.focused {
                    (
                        &highlighting::INVERTED_BOLD_BLUE_STYLE,
                        &highlighting::BOLD_INVERTED_STYLE,
                    )
                } else {
                    (
                        &highlighting::BLUE_STYLE,
                        &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                    )
                }
            }
            LabelType::Index => {
                let style = if self.focused {
                    &highlighting::BOLD_STYLE
                } else {
                    &highlighting::DIMMED_STYLE
                };

                // No match highlighting for index labels.
                (style, &highlighting::DEFAULT_STYLE)
            }
        }
    }

    fn fill_in_value(&mut self, mut available_space: isize) -> Result<isize, fmt::Error> {
        // Object values are sufficiently complicated that we'll handle them
        // in a separate function.
        if self.row.is_container() {
            return self.fill_in_container_value(available_space, self.row);
        }

        let mut value_ref = &self.flatjson.1[self.row.range.clone()];
        let mut quoted = false;
        let color = Self::color_for_value_type(&self.row.value);

        // Strip quotes from strings.
        if self.row.is_string() {
            value_ref = &value_ref[1..value_ref.len() - 1];
            quoted = true;
        }

        let mut used_space = 0;

        if quoted {
            available_space -= 2;
        }

        if self.trailing_comma {
            available_space -= 1;
        }

        let truncated_view = self.initialize_value_truncated_view_or_update_cached(available_space);

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
        let style = Style {
            fg: color,
            ..Style::default()
        };

        let delimiter = if quoted {
            DelimiterPair::Quote
        } else {
            DelimiterPair::None
        };

        if quoted {
            used_space += 2;
        }

        self.highlight_delimited_and_truncated_item(
            delimiter,
            value_ref,
            &truncated_view,
            Some(self.row.range.clone()),
            (&style, &highlighting::SEARCH_MATCH_HIGHLIGHTED),
        )?;

        if self.trailing_comma {
            used_space += 1;
            self.highlight_str(
                ",",
                Some(self.row.range.end),
                (
                    &highlighting::DEFAULT_STYLE,
                    &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                ),
            )?;
        }

        Ok(used_space)
    }

    // We use TruncatedStrViews to manage truncating values when they
    // are too long for the screen, and also to handle scrolling
    // horizontally through those long values.
    //
    // The scroll state needs to be persisted across renders, so the
    // ScreenWriter keeps a HashMap of TruncatedStrViews, and passes
    // in an Entry for the LinePrinter to modify.
    //
    // (Since setting up this map is a pain for testing, it's passed
    // in as an Option.)
    //
    // If we are rendering a line for the first time, most of the
    // time we will initialize the TruncatedStrView from the start of
    // the string. BUT, if we just jumped to a search result on this
    // line, then we want to initialize the TruncatedStrView focused
    // on the search result.
    //
    // If we've already rendered a line, the available space for the
    // line may have updated, so, we will resize the TruncatedStrView.
    fn initialize_value_truncated_view_or_update_cached(
        &mut self,
        available_space: isize,
    ) -> TruncatedStrView {
        debug_assert!(self.row.is_primitive());

        let mut value_ref = &self.flatjson.1[self.row.range.clone()];
        let mut value_range = self.row.range.clone();

        // Strip quotes from strings.
        if self.row.is_string() {
            value_ref = &value_ref[1..value_ref.len() - 1];
            value_range.start += 1;
            value_range.end -= 1;
        }

        self.cached_truncated_value
            .take()
            .map(|entry| {
                *entry
                    .and_modify(|tsv| {
                        *tsv = tsv.resize(value_ref, available_space);
                    })
                    .or_insert_with(|| {
                        let tsv = TruncatedStrView::init_start(value_ref, available_space);

                        // If we're showing a line for the first time, we might
                        // need to focus on a search match that we just jumped to.
                        let no_overlap = self.focused_search_match.end <= value_range.start
                            || value_range.end <= self.focused_search_match.start;

                        // NOTE: If the focused search match starts at the closing
                        // quote of a string, maybe we should use init_back so that
                        // you can see the end of the string and it's more explicit
                        // that the middle of the string isn't part of the search
                        // match.

                        if no_overlap {
                            return tsv;
                        }

                        let offset_focused_range = Range {
                            start: self
                                .focused_search_match
                                .start
                                .saturating_sub(value_range.start),
                            end: (self.focused_search_match.end - value_range.start)
                                .min(value_ref.len()),
                        };

                        tsv.focus(value_ref, &offset_focused_range)
                    })
            })
            .unwrap_or_else(|| TruncatedStrView::init_start(value_ref, available_space))
    }

    fn color_for_value_type(value: &Value) -> Color {
        debug_assert!(value.is_primitive());

        match value {
            Value::Null => terminal::LIGHT_BLACK,
            Value::Boolean => terminal::YELLOW,
            Value::Number => terminal::MAGENTA,
            Value::String => terminal::GREEN,
            Value::EmptyObject => terminal::WHITE,
            Value::EmptyArray => terminal::WHITE,
            _ => unreachable!(),
        }
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
                // Don't highlight the current focused match in the preview.
                //
                // When the container is expanded, it's confusing because two things are
                // highlighted and you're not sure which is focused.
                //
                // When the container is collapsed, it's misleading because the first match
                // isn't really "focused", and hitting 'n' won't jump to the next one in
                // the preview (if more than one is visible).
                self.emphasize_focused_search_match = false;
                let result = self.fill_in_container_preview(available_space, row);
                self.emphasize_focused_search_match = true;
                result
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

            self.highlight_str(
                row.value.container_type().unwrap().open_str(),
                Some(self.row.range.start),
                (style, &highlighting::SEARCH_MATCH_HIGHLIGHTED),
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

            self.highlight_str(
                row.value.container_type().unwrap().close_str(),
                Some(self.row.range.start),
                (style, &highlighting::SEARCH_MATCH_HIGHLIGHTED),
            )?;

            if self.trailing_comma {
                self.highlight_str(
                    ",",
                    Some(self.row.range.end),
                    (
                        &highlighting::DEFAULT_STYLE,
                        &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                    ),
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
        row: &Row,
    ) -> Result<isize, fmt::Error> {
        if self.trailing_comma {
            available_space -= 1;
        }

        let always_quote_string_object_keys = self.mode == Mode::Line;
        let is_nested = false;
        let mut used_space = self.generate_container_preview(
            row,
            available_space,
            is_nested,
            always_quote_string_object_keys,
        )?;

        if self.trailing_comma {
            used_space += 1;
            if self.trailing_comma {
                self.highlight_str(
                    ",",
                    Some(self.row.range.end),
                    (
                        &highlighting::DEFAULT_STYLE,
                        &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                    ),
                )?;
            }
        }

        Ok(used_space)
    }

    fn size_of_container_and_num_digits_required(&self, row: &Row) -> (isize, isize) {
        let container_size = {
            let close_container = &self.flatjson[row.pair_index().unwrap()];
            let last_child_index = close_container.last_child().unwrap();
            (self.flatjson[last_child_index].index_in_parent as isize) + 1
        };

        // We are assuming container_size is never 0.
        let space_needed_for_size = (isize::ilog10(container_size) as isize) + 1;

        (container_size, space_needed_for_size)
    }

    fn generate_container_preview(
        &mut self,
        row: &Row,
        mut available_space: isize,
        is_nested: bool,
        always_quote_string_object_keys: bool,
    ) -> Result<isize, fmt::Error> {
        debug_assert!(row.is_opening_of_container());

        let (container_size, space_needed_for_container_size) =
            self.size_of_container_and_num_digits_required(row);

        // Minimum amount of space required:
        // - top level: (123) […]
        // - nested: […]
        let mut min_space_needed = 3;
        if !is_nested {
            min_space_needed += 3 + space_needed_for_container_size;
        }

        if available_space < min_space_needed {
            return Ok(0);
        }

        let mut num_printed = 0;

        if !is_nested {
            self.terminal.set_fg(terminal::LIGHT_BLACK)?;
            write!(self.terminal, "({container_size}) ")?;
            available_space -= 3 + space_needed_for_container_size;
            num_printed += 3 + space_needed_for_container_size;
        }

        let container_type = row.value.container_type().unwrap();
        available_space -= 2;

        // Create a copy of self.search_matches
        let original_search_matches = self.search_matches.clone();

        self.highlight_str(
            container_type.open_str(),
            Some(self.row.range.start),
            highlighting::PREVIEW_STYLES,
        )?;

        num_printed += 1;

        let mut next_sibling = row.first_child();
        let mut is_first_child = true;
        while let OptionIndex::Index(child) = next_sibling {
            next_sibling = self.flatjson[child].next_sibling;

            // If there are still more elements, we'll print out ", …" at the end,
            let space_needed_at_end_of_container = if next_sibling.is_some() { 3 } else { 0 };
            let space_available_for_elem = available_space - space_needed_at_end_of_container;
            let is_only_child = is_first_child && next_sibling.is_nil();

            let used_space = self.fill_in_container_elem_preview(
                &self.flatjson[child],
                space_available_for_elem,
                always_quote_string_object_keys,
                is_only_child,
            )?;

            if used_space == 0 {
                // No room for anything else, let's close out the object.
                // If we're not the first child, the previous elem will have
                // printed the ", " separator.
                self.highlight_str("…", None, highlighting::PREVIEW_STYLES)?;

                // This variable isn't used again, but if it were, we'd need this
                // line for correctness. Unfortunately Cargo check complains about it,
                // so we'll just leave it here commented out in case code moves around
                // and we need it.
                // available_space -= 1;

                num_printed += 1;
                break;
            } else {
                // Successfully printed elem out, let's print a separator.
                if next_sibling.is_some() {
                    self.highlight_str(
                        ", ",
                        Some(self.flatjson[child].range.end),
                        highlighting::PREVIEW_STYLES,
                    )?;
                    available_space -= 2;
                    num_printed += 2;
                }
            }

            available_space -= used_space;
            num_printed += used_space;

            is_first_child = false;
        }

        self.highlight_str(
            container_type.close_str(),
            Some(self.row.range.end - 1),
            highlighting::PREVIEW_STYLES,
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
        row: &Row,
        mut available_space: isize,
        always_quote_string_object_keys: bool,
        is_only_child: bool,
    ) -> Result<isize, fmt::Error> {
        let mut used_space = 0;

        if let Some(key_range) = &row.key_range {
            let key_without_delimiter_range = key_range.start + 1..key_range.end - 1;
            let key_ref = &self.flatjson.1[key_without_delimiter_range];

            let key_open_delimiter = &self.flatjson.1[key_range.start..key_range.start + 1];
            let mut delimiter = DelimiterPair::None;

            if key_open_delimiter == "[" {
                delimiter = DelimiterPair::Square;
            } else if always_quote_string_object_keys || !JS_IDENTIFIER.is_match(key_ref) {
                delimiter = DelimiterPair::Quote;
            }

            // Need at least one character for value, and two characters for ": "
            let mut space_available_for_key = available_space - 3;

            space_available_for_key -= delimiter.width();

            let truncated_view = TruncatedStrView::init_start(key_ref, space_available_for_key);
            let space_used_for_label = truncated_view.used_space();
            if space_used_for_label.is_none() || truncated_view.is_completely_elided() {
                return Ok(0);
            }

            let space_used_for_label = space_used_for_label.unwrap();
            used_space += space_used_for_label;
            available_space -= space_used_for_label;

            used_space += delimiter.width();
            available_space -= delimiter.width();

            self.highlight_delimited_and_truncated_item(
                delimiter,
                key_ref,
                &truncated_view,
                Some(key_range.clone()),
                highlighting::PREVIEW_STYLES,
            )?;

            used_space += 2;
            available_space -= 2;
            self.highlight_str(": ", Some(key_range.end), highlighting::PREVIEW_STYLES)?;
        }

        let space_used_for_value = if is_only_child && row.value.is_container() {
            let is_nested = true;
            self.generate_container_preview(
                row,
                available_space,
                is_nested,
                always_quote_string_object_keys,
            )?
        } else {
            self.fill_in_value_preview(row, available_space)?
        };
        used_space += space_used_for_value;

        // Make sure to print out ellipsis for the value if we printed out an
        // object key, but couldn't print out the value. Space was already
        // allocated for this at the start of the function.
        if row.key_range.is_some() && space_used_for_value == 0 {
            self.terminal.write_char('…')?;
            used_space += 1;
        }

        Ok(used_space)
    }

    fn fill_in_value_preview(
        &mut self,
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
                &self.flatjson.1[range.start + 1..range.end - 1]
            }
            _ => &self.flatjson.1[row.range.clone()],
        };

        if quoted {
            available_space -= 2;
        }

        let space_used_for_quotes = if quoted { 2 } else { 0 };

        let truncated_view = TruncatedStrView::init_start(value_ref, available_space);
        let space_used_for_value = truncated_view.used_space();

        if space_used_for_value.is_none() || truncated_view.is_completely_elided() {
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
            self.highlight_str(
                "\"",
                Some(value_open_quote_range_start),
                highlighting::PREVIEW_STYLES,
            )?;
        }

        let focused_search_match = if self.emphasize_focused_search_match {
            self.focused_search_match
        } else {
            &NO_FOCUSED_MATCH
        };

        highlighting::highlight_truncated_str_view(
            self.terminal,
            value_ref,
            &truncated_view,
            // Technically could try to highlight open and close delimiters
            // of the collapsed container, but not really worth it right now.
            if showing_collapsed_preview {
                None
            } else {
                Some(value_range_start)
            },
            &highlighting::DIMMED_STYLE,
            &highlighting::GRAY_INVERTED_STYLE,
            &mut self.search_matches.as_mut(),
            focused_search_match,
        )?;

        if quoted {
            self.highlight_str(
                "\"",
                Some(value_close_quote_range_start),
                highlighting::PREVIEW_STYLES,
            )?;
        }

        Ok(space_used_for_quotes + space_used_for_value.unwrap())
    }

    fn print_truncated_indicator(&mut self) -> fmt::Result {
        self.terminal.position_cursor_col(self.width as u16)?;
        if self.focused {
            self.terminal.reset_style()?;
            self.terminal.set_bold(true)?;
        } else {
            self.terminal.set_fg(terminal::LIGHT_BLACK)?;
        }
        write!(self.terminal, ">")
    }

    // A helper to print out a TruncatedStrView that may be
    // surrounded by a delimiter.
    //
    // The passed in str, s, should not include the delimiter.
    // If passed in, str_range *should* include the delimiter.
    fn highlight_delimited_and_truncated_item(
        &mut self,
        delimiter: DelimiterPair,
        s: &str,
        truncated_view: &TruncatedStrView,
        str_range: Option<Range<usize>>,
        styles: (&Style, &Style),
    ) -> fmt::Result {
        let mut str_open_delimiter_range_start = None;
        let mut str_range_start = None;
        let mut str_close_delimiter_range_start = None;

        if let Some(range) = str_range {
            str_open_delimiter_range_start = Some(range.start);
            str_range_start = Some(range.start + delimiter.left().len());
            str_close_delimiter_range_start = Some(range.end - delimiter.right().len());
        }

        self.highlight_str(delimiter.left(), str_open_delimiter_range_start, styles)?;

        let focused_search_match = if self.emphasize_focused_search_match {
            self.focused_search_match
        } else {
            &NO_FOCUSED_MATCH
        };

        highlighting::highlight_truncated_str_view(
            self.terminal,
            s,
            truncated_view,
            str_range_start,
            styles.0,
            styles.1,
            &mut self.search_matches.as_mut(),
            focused_search_match,
        )?;

        self.highlight_str(delimiter.right(), str_close_delimiter_range_start, styles)?;

        Ok(())
    }

    // A helper to print out a simple string that may be highlighted.
    fn highlight_str(
        &mut self,
        s: &str,
        str_range_start: Option<usize>,
        styles: (&Style, &Style),
    ) -> fmt::Result {
        let focused_search_match = if self.emphasize_focused_search_match {
            self.focused_search_match
        } else {
            &NO_FOCUSED_MATCH
        };

        highlighting::highlight_matches(
            self.terminal,
            s,
            str_range_start,
            styles.0,
            styles.1,
            &mut self.search_matches.as_mut(),
            focused_search_match,
        )
    }
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use crate::flatjson::{parse_top_level_json, parse_top_level_yaml};
    use crate::terminal::test::{TextOnlyTerminal, VisibleEscapesTerminal};
    use crate::terminal::{BLUE, LIGHT_BLUE};

    use super::*;

    const DUMMY_RANGE: Range<usize> = 0..0;

    fn default_line_printer<'a>(
        terminal: &'a mut dyn Terminal,
        flatjson: &'a FlatJson,
        index: usize,
    ) -> LinePrinter<'a, 'a> {
        LinePrinter {
            mode: Mode::Data,
            terminal,
            flatjson,
            row: &flatjson[index],
            line_number: LineNumber {
                absolute: None,
                relative: None,
                max_width: 4,
            },
            indentation: 0,
            width: 100,
            focused: false,
            focused_because_matching_container_pair: false,
            trailing_comma: false,
            search_matches: None,
            focused_search_match: &DUMMY_RANGE,
            emphasize_focused_search_match: true,
            cached_truncated_value: None,
        }
    }

    #[test]
    fn test_line_numbers() -> std::fmt::Result {
        const JSON: &str = r#"{
            "hello": 1,
            "2": [
                3,
            ],
        }"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();

        let mut term = VisibleEscapesTerminal::new(true, false);
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 3);
        line.indentation = 4;

        let abs = Some(14);
        let rel = Some(6);
        let f_line = FOCUSED_LINE;
        let n_line = NOT_FOCUSED_LINE;

        for (absolute, relative, focused, expected) in vec![
            (None, None, false, format!("    {n_line}[0]: 3")),
            (None, None, true, format!("    {f_line}[0]: 3")),
            (abs, None, false, format!("  14     {n_line}[0]: 3")),
            (abs, None, true, format!("  14     {f_line}[0]: 3")),
            (None, rel, false, format!("   6     {n_line}[0]: 3")),
            (None, rel, true, format!("   6     {f_line}[0]: 3")),
            (abs, rel, false, format!("   6     {n_line}[0]: 3")),
            (abs, rel, true, format!("14       {f_line}[0]: 3")),
        ]
        .into_iter()
        {
            line.terminal.clear_output();
            line.line_number.absolute = absolute;
            line.line_number.relative = relative;
            line.focused = focused;

            line.print_line()?;
            assert_eq!(
                expected,
                line.terminal.output(),
                "expected output for abs: {absolute:?}, rel: {relative:?}, focused: {focused} in data mode",
            );
        }

        line.mode = Mode::Line;
        for (absolute, relative, focused, expected) in vec![
            (None, None, false, format!("{n_line}    3")),
            (None, None, true, format!("{f_line}    3")),
            (abs, None, false, format!("  14 {n_line}    3")),
            (abs, None, true, format!("  14 {f_line}    3")),
            (None, rel, false, format!("   6 {n_line}    3")),
            (None, rel, true, format!("   6 {f_line}    3")),
            (abs, rel, false, format!("   6 {n_line}    3")),
            (abs, rel, true, format!("14   {f_line}    3")),
        ]
        .into_iter()
        {
            line.terminal.clear_output();
            line.line_number.absolute = absolute;
            line.line_number.relative = relative;
            line.focused = focused;

            line.print_line()?;
            assert_eq!(
                expected,
                line.terminal.output(),
                "expected output for abs: {absolute:?}, rel: {relative:?}, focused: {focused} in line mode",
            );
        }

        Ok(())
    }

    #[test]
    fn test_print_line_tracks_available_space() -> std::fmt::Result {
        const JSON: &str = r#"{
            "hello": 1,
            "key_2": {
                "key_3": "value",
                "key_4": "value2",
            },
        }"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();

        let mut term = VisibleEscapesTerminal::new(true, false);
        // ### __> key_2: (2) {key_3: "value", key_4: "value2"}
        // 1234567890123456789012345678901234567890123456789012
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 2);
        line.indentation = 2;
        line.line_number.max_width = 3;

        line.width = 48;
        line.print_line()?;
        assert_eq!(
            format!(r#"  {EXPANDED_CONTAINER}key_2: (2) {{key_3: "value", key_4: "value2"}}"#),
            line.terminal.output(),
        );
        line.terminal.clear_output();

        line.width = 47;
        line.print_line()?;
        assert_eq!(
            format!(r#"  {EXPANDED_CONTAINER}key_2: (2) {{key_3: "value", key_4: "valu…"}}"#),
            line.terminal.output(),
        );
        line.terminal.clear_output();

        line.width = 52;
        line.line_number.absolute = Some(2);
        line.print_line()?;
        assert_eq!(
            format!(r#"  2   {EXPANDED_CONTAINER}key_2: (2) {{key_3: "value", key_4: "value2"}}"#),
            line.terminal.output(),
        );
        line.terminal.clear_output();

        line.width = 51;
        line.print_line()?;
        assert_eq!(
            format!(r#"  2   {EXPANDED_CONTAINER}key_2: (2) {{key_3: "value", key_4: "valu…"}}"#),
            line.terminal.output(),
        );

        Ok(())
    }

    #[test]
    fn test_line_mode_focus_indicators() -> std::fmt::Result {
        const JSON: &str = r#"{ "1": 1 }"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();

        // Line mode either focused or not.
        let mut term = VisibleEscapesTerminal::new(true, false);
        let mut line: LinePrinter = LinePrinter {
            mode: Mode::Line,
            indentation: 4,
            ..default_line_printer(&mut term, &fj, 1)
        };

        // Not focused; no indicator.
        line.print_focus_and_container_indicators(100)?;
        assert_eq!("      ", line.terminal.output());
        line.terminal.clear_output();

        line.focused = true;

        line.print_focus_and_container_indicators(100)?;
        assert_eq!(format!("{FOCUSED_LINE}    "), line.terminal.output());
        line.terminal.clear_output();

        line.print_focus_and_container_indicators(3)?;
        assert_eq!(format!("{FOCUSED_LINE}"), line.terminal.output());
        line.terminal.clear_output();

        line.print_focus_and_container_indicators(2)?;
        assert_eq!("", line.terminal.output());
        line.terminal.clear_output();

        Ok(())
    }

    #[test]
    fn test_data_mode_focus_indicators() -> std::fmt::Result {
        const JSON: &str = r#"{
            "1": 1,
        }
        3
        {
            "5": { "6": 6 }
        }"#;
        let mut fj = parse_top_level_json(JSON.to_owned()).unwrap();
        fj.collapse(5);

        let mut term = VisibleEscapesTerminal::new(true, false);
        let mut line: LinePrinter = LinePrinter {
            indentation: 0,
            ..default_line_printer(&mut term, &fj, 0)
        };

        line.print_focus_and_container_indicators(100)?;
        assert_eq!(format!("{EXPANDED_CONTAINER}"), line.terminal.output());
        line.terminal.clear_output();

        line.focused = true;

        line.print_focus_and_container_indicators(100)?;
        assert_eq!(
            format!("{FOCUSED_EXPANDED_CONTAINER}"),
            line.terminal.output()
        );
        line.terminal.clear_output();

        line.row = &line.flatjson[3];
        line.indentation = 2;

        line.print_focus_and_container_indicators(100)?;
        assert_eq!(format!("  {FOCUSED_LINE}"), line.terminal.output());
        line.terminal.clear_output();

        line.row = &line.flatjson[5];
        line.indentation = 4;

        line.print_focus_and_container_indicators(7)?;
        assert_eq!(
            format!("    {FOCUSED_COLLAPSED_CONTAINER}"),
            line.terminal.output()
        );
        line.terminal.clear_output();

        line.print_focus_and_container_indicators(6)?;
        assert_eq!("   ", line.terminal.output());
        line.terminal.clear_output();

        line.focused = false;

        line.print_focus_and_container_indicators(100)?;
        assert_eq!(format!("    {COLLAPSED_CONTAINER}"), line.terminal.output());

        Ok(())
    }

    #[test]
    fn test_fill_key_label_basic() -> std::fmt::Result {
        const JSON: &str = r#"{
            "hello": 1,
            "french fry": 2,
            "": 3,
        }"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();

        let mut term = VisibleEscapesTerminal::new(false, true);
        let mut line: LinePrinter = LinePrinter {
            mode: Mode::Line,
            ..default_line_printer(&mut term, &fj, 1)
        };

        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_FG({LIGHT_BLUE})_\"hello\"_FG(Default)_: "),
            line.terminal.output()
        );
        assert_eq!(9, used_space);

        line.mode = Mode::Data;

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_FG({LIGHT_BLUE})_hello_FG(Default)_: "),
            line.terminal.output()
        );
        assert_eq!(7, used_space);

        line.focused = true;

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_BG({BLUE})__INV__B_hello_BG(Default)__!INV__!B_: "),
            line.terminal.output(),
        );
        assert_eq!(7, used_space);

        line.focused = false;

        // Non JS identifiers get quoted.
        line.row = &line.flatjson[2];

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_FG({LIGHT_BLUE})_\"french fry\"_FG(Default)_: "),
            line.terminal.output(),
        );
        assert_eq!(14, used_space);

        // Empty strings aren't valid JS identifiers either
        line.row = &line.flatjson[3];

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_FG({LIGHT_BLUE})_\"\"_FG(Default)_: "),
            line.terminal.output(),
        );
        assert_eq!(4, used_space);

        Ok(())
    }

    // Currently we incorrectly print quotes around all of these.
    #[test]
    fn test_fill_key_non_scalar_keys() -> std::fmt::Result {
        const YAML: &str = r#"{
            [one]: 1,
            [t, w, o]: 2,
            3: 3,
            null: 4,
        }"#;
        let fj = parse_top_level_yaml(YAML.to_owned()).unwrap();

        let mut term = VisibleEscapesTerminal::new(false, false);
        let mut line: LinePrinter = LinePrinter {
            mode: Mode::Line,
            ..default_line_printer(&mut term, &fj, 1)
        };

        let used_space = line.fill_in_label(100)?;

        assert_eq!(r#"[["one"]]: "#, line.terminal.output());
        assert_eq!(11, used_space);

        line.mode = Mode::Data;

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(r#"[["one"]]: "#, line.terminal.output());
        assert_eq!(11, used_space);

        line.row = &line.flatjson[2];

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(r#"[["t", "w", "o"]]: "#, line.terminal.output());
        assert_eq!(19, used_space);

        line.row = &line.flatjson[3];

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(r#"[3]: "#, line.terminal.output());
        assert_eq!(5, used_space);

        line.row = &line.flatjson[4];

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(r#"[null]: "#, line.terminal.output());
        assert_eq!(8, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_index_label_basic() -> std::fmt::Result {
        const JSON: &str = r#"[
            8,
        ]"#;
        let mut fj = parse_top_level_json(JSON.to_owned()).unwrap();
        fj[1].index_in_parent = 12345;

        let mut term = VisibleEscapesTerminal::new(false, true);
        let mut line: LinePrinter = LinePrinter {
            ..default_line_printer(&mut term, &fj, 1)
        };

        let used_space = line.fill_in_label(100)?;
        assert_eq!("_D_[12345]_!D_: ", line.terminal.output());
        assert_eq!(9, used_space);

        line.focused = true;
        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!("_B_[12345]_!B_: ", line.terminal.output());
        assert_eq!(9, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_label_not_enough_space() -> std::fmt::Result {
        const JSON: &str = r#"{
            "hello": 1,
            "2": [
                3,
            ],
        }"#;
        let mut fj = parse_top_level_json(JSON.to_owned()).unwrap();
        fj[3].index_in_parent = 12345;

        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 1);
        line.mode = Mode::Line;

        // QUOTED STRING KEY

        // Minimum space is: '"h…": ', which has a length of 6, plus extra space for value char.

        let used_space = line.fill_in_label(7)?;
        assert_eq!("\"h…\": ", line.terminal.output());
        assert_eq!(6, used_space);

        line.terminal.clear_output();

        // Elide the whole key
        let used_space = line.fill_in_label(6)?;
        assert_eq!("\"…\": ", line.terminal.output());
        assert_eq!(5, used_space);

        line.terminal.clear_output();

        // Can't fit at all
        let used_space = line.fill_in_label(5)?;
        assert_eq!("", line.terminal.output());
        assert_eq!(0, used_space);

        // UNQUOTED STRING KEY

        // Minimum space is: "h…: ", which has a length of 4, plus extra space for value char.
        line.mode = Mode::Data;

        line.terminal.clear_output();

        let used_space = line.fill_in_label(5)?;
        assert_eq!("h…: ", line.terminal.output());
        assert_eq!(4, used_space);

        line.terminal.clear_output();

        // Elide the whole key.
        let used_space = line.fill_in_label(4)?;
        assert_eq!("…: ", line.terminal.output());
        assert_eq!(3, used_space);

        line.terminal.clear_output();

        // Not enough room, returns 0.
        let used_space = line.fill_in_label(3)?;
        assert_eq!("", line.terminal.output());
        assert_eq!(0, used_space);

        // ARRAY INDEX

        line.row = &line.flatjson[3];

        line.terminal.clear_output();

        let used_space = line.fill_in_label(7)?;
        assert_eq!("[1…]: ", line.terminal.output());
        assert_eq!(6, used_space);

        line.terminal.clear_output();

        // Not enough room, elides whole index.
        let used_space = line.fill_in_label(6)?;
        assert_eq!("[…]: ", line.terminal.output());
        assert_eq!(5, used_space);

        line.terminal.clear_output();

        // Not enough room, returns 0.
        let used_space = line.fill_in_label(5)?;
        assert_eq!("", line.terminal.output());
        assert_eq!(0, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_value_basic() -> std::fmt::Result {
        let fj = parse_top_level_json("\"hello\"\nnull".to_owned()).unwrap();
        let mut term = VisibleEscapesTerminal::new(false, true);
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 0);

        let used_space = line.fill_in_value(100)?;

        assert_eq!("_FG(Green)_\"hello\"", line.terminal.output());
        assert_eq!(7, used_space);

        line.trailing_comma = true;
        line.row = &line.flatjson[1];

        line.terminal.clear_output();
        let used_space = line.fill_in_value(100)?;

        assert_eq!("_FG(LightBlack)_null_FG(Default)_,", line.terminal.output());
        assert_eq!(5, used_space);

        Ok(())
    }

    #[test]
    fn test_fill_value_not_enough_space() -> std::fmt::Result {
        let fj = parse_top_level_json(r#"["hello", "", true]"#.to_owned()).unwrap();
        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 1);

        // QUOTED VALUE

        // Minimum space is: '"h…"', which has a length of 4.

        let used_space = line.fill_in_value(4)?;
        assert_eq!("\"h…\"", line.terminal.output());
        assert_eq!(4, used_space);

        line.terminal.clear_output();

        // Not enough room; fully elides string.
        let used_space = line.fill_in_value(3)?;
        assert_eq!("\"…\"", line.terminal.output());
        assert_eq!(3, used_space);

        line.terminal.clear_output();

        // Not enough room, returns empty string.
        let used_space = line.fill_in_value(2)?;
        assert_eq!("", line.terminal.output());
        assert_eq!(0, used_space);

        // QUOTED EMPTY STRING

        line.row = &line.flatjson[2];

        line.terminal.clear_output();
        let used_space = line.fill_in_value(2)?;
        assert_eq!("\"\"", line.terminal.output());
        assert_eq!(2, used_space);

        line.terminal.clear_output();
        let used_space = line.fill_in_value(1)?;
        assert_eq!("", line.terminal.output());
        assert_eq!(0, used_space);

        // UNQUOTED VALUE, TRAILING COMMA

        // Minimum space is: 't…,', which has a length of 3.
        line.trailing_comma = true;

        line.row = &line.flatjson[3];

        line.terminal.clear_output();

        let used_space = line.fill_in_value(3)?;
        assert_eq!("t…,", line.terminal.output());
        assert_eq!(3, used_space);

        line.terminal.clear_output();

        let used_space = line.fill_in_value(2)?;
        assert_eq!("…,", line.terminal.output());
        assert_eq!(2, used_space);

        line.terminal.clear_output();

        // Not enough room, returns 0.
        let used_space = line.fill_in_value(1)?;
        assert_eq!("", line.terminal.output());
        assert_eq!(0, used_space);

        // UNQUOTED VALUE, NO TRAILING COMMA
        line.trailing_comma = false;

        line.terminal.clear_output();

        // Don't print just an ellipsis, we'll print '>' instead.
        let used_space = line.fill_in_value(1)?;
        assert_eq!("", line.terminal.output());
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
        let fj = parse_top_level_json(json.to_owned()).unwrap();

        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 0);

        for (available_space, used_space, always_quote_string_object_keys, expected) in vec![
            (54, 35, true, r#"(3) {"a": 1, "d": {…}, "b c": null}"#),
            (54, 31, false, r#"(3) {a: 1, d: {…}, "b c": null}"#),
            (30, 30, false, r#"(3) {a: 1, d: {…}, "b c": nu…}"#),
            (29, 29, false, r#"(3) {a: 1, d: {…}, "b c": n…}"#),
            (28, 28, false, r#"(3) {a: 1, d: {…}, "b c": …}"#),
            (27, 27, false, r#"(3) {a: 1, d: {…}, "b…": …}"#),
            (26, 21, false, r#"(3) {a: 1, d: {…}, …}"#),
            (20, 19, false, r#"(3) {a: 1, d: …, …}"#),
            (18, 13, false, r#"(3) {a: 1, …}"#),
            (12, 7, false, r#"(3) {…}"#),
            (6, 0, false, r#""#),
        ]
        .into_iter()
        {
            let is_nested = false;
            let used = line.generate_container_preview(
                &line.flatjson[0],
                available_space,
                is_nested,
                always_quote_string_object_keys,
            )?;
            assert_eq!(
                expected,
                line.terminal.output(),
                "expected preview with {} available columns (used up {} columns)",
                available_space,
                UnicodeWidthStr::width(line.terminal.output()),
            );
            assert_eq!(used_space, used);

            line.terminal.clear_output();
        }

        Ok(())
    }

    #[test]
    fn test_generate_array_preview() -> fmt::Result {
        let json = r#"[1, {"x": true}, null, "hello", true]"#;
        //            [1, {…}, null, "hello", true]
        //           012345678901234567890123456789 (29 characters)
        let fj = parse_top_level_json(json.to_owned()).unwrap();

        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 0);

        for (available_space, used_space, expected) in vec![
            (54, 33, r#"(5) [1, {…}, null, "hello", true]"#),
            (32, 32, r#"(5) [1, {…}, null, "hello", tr…]"#),
            (31, 31, r#"(5) [1, {…}, null, "hello", t…]"#),
            (30, 30, r#"(5) [1, {…}, null, "hello", …]"#),
            (29, 29, r#"(5) [1, {…}, null, "hel…", …]"#),
            (28, 28, r#"(5) [1, {…}, null, "he…", …]"#),
            (27, 27, r#"(5) [1, {…}, null, "h…", …]"#),
            (26, 21, r#"(5) [1, {…}, null, …]"#),
            (20, 20, r#"(5) [1, {…}, nu…, …]"#),
            (19, 19, r#"(5) [1, {…}, n…, …]"#),
            (18, 15, r#"(5) [1, {…}, …]"#),
            (14, 10, r#"(5) [1, …]"#),
            (9, 7, r#"(5) […]"#),
            (6, 0, r#""#),
        ]
        .into_iter()
        {
            let is_nested = false;
            let always_quote_string_object_keys = false;
            let used = line.generate_container_preview(
                &line.flatjson[0],
                available_space,
                is_nested,
                always_quote_string_object_keys,
            )?;
            assert_eq!(
                expected,
                line.terminal.output(),
                "expected preview with {} available columns (used up {} columns)",
                available_space,
                UnicodeWidthStr::width(line.terminal.output()),
            );
            assert_eq!(used_space, used);

            line.terminal.clear_output();
        }

        Ok(())
    }

    #[test]
    fn test_generate_container_preview_single_container_child() -> fmt::Result {
        let json = r#"{"a": [1, {"x": true}, null, "hello", true]}"#;
        //            {a: [1, {…}, null, "hello", true]}
        //           01234567890123456789012345678901234 (34 characters)
        let fj = parse_top_level_json(json.to_owned()).unwrap();

        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 0);

        let used = line.generate_container_preview(&line.flatjson[0], 38, false, false)?;
        assert_eq!(
            r#"(1) {a: [1, {…}, null, "hello", true]}"#,
            line.terminal.output()
        );
        assert_eq!(38, used);

        line.terminal.clear_output();
        let used = line.generate_container_preview(&line.flatjson[0], 37, false, false)?;
        assert_eq!(
            r#"(1) {a: [1, {…}, null, "hello", tr…]}"#,
            line.terminal.output()
        );
        assert_eq!(37, used);

        let json = r#"[{"a": 1, "d": {"x": true}, "b c": null}]"#;
        //            [{a: 1, d: {…}, "b c": null}]
        //           012345678901234567890123456789 (29 characters)
        let fj = parse_top_level_json(json.to_owned()).unwrap();

        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 0);

        let used = line.generate_container_preview(&line.flatjson[0], 33, false, false)?;
        assert_eq!(
            r#"(1) [{a: 1, d: {…}, "b c": null}]"#,
            line.terminal.output()
        );
        assert_eq!(33, used);

        line.terminal.clear_output();
        let used = line.generate_container_preview(&line.flatjson[0], 32, false, false)?;
        assert_eq!(
            r#"(1) [{a: 1, d: {…}, "b c": nu…}]"#,
            line.terminal.output()
        );
        assert_eq!(32, used);

        Ok(())
    }

    #[test]
    fn test_generate_object_preview_with_non_scalar_keys() -> std::fmt::Result {
        const YAML: &str = r#"{
            true: 1,
            [t, w, o]: 2,
            3: 3,
            null: 4,
        }"#;
        let fj = parse_top_level_yaml(YAML.to_owned()).unwrap();

        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = default_line_printer(&mut term, &fj, 0);

        let expected = r#"{[true]: 1, [["t", "w", "o"]]: 2, [3]: 3, [null]: 4}"#;

        let _ = line.generate_container_preview(&line.flatjson[0], 100, true, true)?;
        assert_eq!(expected, line.terminal.output());

        line.terminal.clear_output();
        let _ = line.generate_container_preview(&line.flatjson[0], 100, true, false)?;
        assert_eq!(expected, line.terminal.output());

        Ok(())
    }
}
