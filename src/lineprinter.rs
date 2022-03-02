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

const FOCUSED_LINE: &str = "▶ ";
const FOCUSED_COLLAPSED_CONTAINER: &str = "▶ ";
const FOCUSED_EXPANDED_CONTAINER: &str = "▼ ";
const COLLAPSED_CONTAINER: &str = "▷ ";
const EXPANDED_CONTAINER: &str = "▽ ";
const INDICATOR_WIDTH: usize = 2;

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

pub struct LinePrinter<'a, 'b> {
    pub mode: Mode,
    pub terminal: &'a mut dyn Terminal,

    pub flatjson: &'a FlatJson,
    pub row: &'a Row,

    pub indentation: usize,
    pub width: usize,

    // Line-by-line formatting options
    pub focused: bool,
    pub focused_because_matching_container_pair: bool,
    pub trailing_comma: bool,

    // Stuff to actually print out
    pub value_range: &'a Range<usize>,

    pub search_matches: Option<Peekable<MatchRangeIter<'b>>>,
    pub focused_search_match: &'a Range<usize>,

    pub cached_truncated_value: Option<Entry<'a, usize, TruncatedStrView>>,
}

impl<'a, 'b> LinePrinter<'a, 'b> {
    pub fn print_line(&mut self) -> fmt::Result {
        self.terminal.reset_style()?;

        self.print_focus_and_container_indicators()?;

        let label_depth = INDICATOR_WIDTH + self.indentation;

        // I don't know if there's standard behavior for setting the column
        // past the width of the screen, so let's avoid doing that. There
        // will still be cases where this condition is true, but we still end
        // up printing the truncated indicator, but that's fine.
        if label_depth < self.width {
            self.terminal
                .position_cursor_col((1 + label_depth) as u16)?;
        }

        let mut available_space = self.width as isize - label_depth as isize;

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
            self.terminal.position_cursor_col(1)?;
            write!(self.terminal, "{}", FOCUSED_LINE)?;
        }

        Ok(())
    }

    fn print_container_indicator(&mut self) -> fmt::Result {
        if self.row.is_primitive() {
            // Print a focused indicator for top-level primitives.
            if self.focused && self.row.depth == 0 {
                self.terminal.position_cursor_col(0)?;
                write!(self.terminal, "{}", FOCUSED_COLLAPSED_CONTAINER)?;
            }
            return Ok(());
        }

        debug_assert!(self.row.is_opening_of_container());

        let collapsed = self.row.is_collapsed();

        // Make sure there's enough room for the indicator
        if self.width <= INDICATOR_WIDTH + self.indentation {
            return Ok(());
        }

        let container_indicator_col = (1 + self.indentation) as u16;
        self.terminal.position_cursor_col(container_indicator_col)?;

        let indicator = match (self.focused, collapsed) {
            (true, true) => FOCUSED_COLLAPSED_CONTAINER,
            (true, false) => FOCUSED_EXPANDED_CONTAINER,
            (false, true) => COLLAPSED_CONTAINER,
            (false, false) => EXPANDED_CONTAINER,
        };

        write!(self.terminal, "{}", indicator)?;

        Ok(())
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

            write!(label, "{}", self.row.index).unwrap();

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
            Some(self.value_range.clone()),
            (&style, &highlighting::SEARCH_MATCH_HIGHLIGHTED),
        )?;

        if self.trailing_comma {
            used_space += 1;
            self.highlight_str(
                ",",
                Some(self.value_range.end),
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
                self.fill_in_container_preview(available_space, row)
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
                Some(self.value_range.start),
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
                Some(self.value_range.start),
                (style, &highlighting::SEARCH_MATCH_HIGHLIGHTED),
            )?;

            if self.trailing_comma {
                self.highlight_str(
                    ",",
                    Some(self.value_range.end),
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
        let mut used_space =
            self.generate_container_preview(row, available_space, always_quote_string_object_keys)?;

        if self.trailing_comma {
            used_space += 1;
            if self.trailing_comma {
                self.highlight_str(
                    ",",
                    Some(self.value_range.end),
                    (
                        &highlighting::DEFAULT_STYLE,
                        &highlighting::SEARCH_MATCH_HIGHLIGHTED,
                    ),
                )?;
            }
        }

        Ok(used_space)
    }

    fn generate_container_preview(
        &mut self,
        row: &Row,
        mut available_space: isize,
        always_quote_string_object_keys: bool,
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

        self.highlight_str(
            container_type.open_str(),
            Some(self.value_range.start),
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
            Some(self.value_range.end - 1),
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
            self.generate_container_preview(row, available_space, always_quote_string_object_keys)?
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
            self.focused_search_match,
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

        highlighting::highlight_truncated_str_view(
            self.terminal,
            s,
            truncated_view,
            str_range_start,
            styles.0,
            styles.1,
            &mut self.search_matches.as_mut(),
            self.focused_search_match,
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
        highlighting::highlight_matches(
            self.terminal,
            s,
            str_range_start,
            styles.0,
            styles.1,
            &mut self.search_matches.as_mut(),
            self.focused_search_match,
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
            indentation: 0,
            width: 100,
            focused: false,
            focused_because_matching_container_pair: false,
            trailing_comma: false,
            value_range: &DUMMY_RANGE,
            search_matches: None,
            focused_search_match: &DUMMY_RANGE,
            cached_truncated_value: None,
        }
    }

    #[test]
    fn test_line_mode_focus_indicators() -> std::fmt::Result {
        const JSON: &str = r#"{ "1": 1 }"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();

        // Line mode either focused or not.
        let mut term = VisibleEscapesTerminal::new(true, false);
        let mut line: LinePrinter = LinePrinter {
            mode: Mode::Line,
            indentation: 10,
            ..default_line_printer(&mut term, &fj, 1)
        };

        // Not focused; no indicator.
        line.print_focus_and_container_indicators()?;
        assert_eq!("", line.terminal.output());
        line.terminal.clear_output();

        line.focused = true;

        line.print_focus_and_container_indicators()?;
        assert_eq!(format!("_C(1)_{}", FOCUSED_LINE), line.terminal.output());

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

        line.print_focus_and_container_indicators()?;
        assert_eq!(
            format!("_C(1)_{}", EXPANDED_CONTAINER),
            line.terminal.output()
        );
        line.terminal.clear_output();

        line.focused = true;

        line.print_focus_and_container_indicators()?;
        assert_eq!(
            format!("_C(1)_{}", FOCUSED_EXPANDED_CONTAINER),
            line.terminal.output()
        );
        line.terminal.clear_output();

        line.row = &line.flatjson[5];
        line.indentation = 2;

        line.print_focus_and_container_indicators()?;
        assert_eq!(
            format!("_C(3)_{}", FOCUSED_COLLAPSED_CONTAINER),
            line.terminal.output()
        );
        line.terminal.clear_output();

        line.focused = false;

        line.print_focus_and_container_indicators()?;
        assert_eq!(
            format!("_C(3)_{}", COLLAPSED_CONTAINER),
            line.terminal.output()
        );

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
            format!("_FG({})_\"hello\"_FG(Default)_: ", LIGHT_BLUE),
            line.terminal.output()
        );
        assert_eq!(9, used_space);

        line.mode = Mode::Data;

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_FG({})_hello_FG(Default)_: ", LIGHT_BLUE),
            line.terminal.output()
        );
        assert_eq!(7, used_space);

        line.focused = true;

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_BG({})__INV__B_hello_BG(Default)__!INV__!B_: ", BLUE),
            line.terminal.output(),
        );
        assert_eq!(7, used_space);

        line.focused = false;

        // Non JS identifiers get quoted.
        line.row = &line.flatjson[2];

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_FG({})_\"french fry\"_FG(Default)_: ", LIGHT_BLUE),
            line.terminal.output(),
        );
        assert_eq!(14, used_space);

        // Empty strings aren't valid JS identifiers either
        line.row = &line.flatjson[3];

        line.terminal.clear_output();
        let used_space = line.fill_in_label(100)?;

        assert_eq!(
            format!("_FG({})_\"\"_FG(Default)_: ", LIGHT_BLUE),
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
        fj[1].index = 12345;

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
        fj[3].index = 12345;

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
        let mut line: LinePrinter = LinePrinter {
            value_range: &(1..6),
            ..default_line_printer(&mut term, &fj, 0)
        };

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
        line.value_range = &line.flatjson[1].range;

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
        line.value_range = &line.flatjson[2].range;

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
        line.value_range = &line.flatjson[3].range;

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
        let mut line: LinePrinter = LinePrinter {
            value_range: &(0..json.len()),
            ..default_line_printer(&mut term, &fj, 0)
        };

        for (available_space, used_space, always_quote_string_object_keys, expected) in vec![
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
            let used = line.generate_container_preview(
                &line.flatjson[0],
                available_space,
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
        let mut line: LinePrinter = LinePrinter {
            value_range: &(0..json.len()),
            ..default_line_printer(&mut term, &fj, 0)
        };

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
            let always_quote_string_object_keys = false;
            let used = line.generate_container_preview(
                &line.flatjson[0],
                available_space,
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
        let mut line: LinePrinter = LinePrinter {
            value_range: &(0..json.len()),
            ..default_line_printer(&mut term, &fj, 0)
        };

        let used = line.generate_container_preview(&line.flatjson[0], 34, false)?;
        assert_eq!(
            r#"{a: [1, {…}, null, "hello", true]}"#,
            line.terminal.output()
        );
        assert_eq!(34, used);

        line.terminal.clear_output();
        let used = line.generate_container_preview(&line.flatjson[0], 33, false)?;
        assert_eq!(
            r#"{a: [1, {…}, null, "hello", tr…]}"#,
            line.terminal.output()
        );
        assert_eq!(33, used);

        let json = r#"[{"a": 1, "d": {"x": true}, "b c": null}]"#;
        //            [{a: 1, d: {…}, "b c": null}]
        //           012345678901234567890123456789 (29 characters)
        let fj = parse_top_level_json(json.to_owned()).unwrap();

        let mut term = TextOnlyTerminal::new();
        let mut line: LinePrinter = LinePrinter {
            value_range: &(0..json.len()),
            ..default_line_printer(&mut term, &fj, 0)
        };

        let used = line.generate_container_preview(&line.flatjson[0], 29, false)?;
        assert_eq!(r#"[{a: 1, d: {…}, "b c": null}]"#, line.terminal.output());
        assert_eq!(29, used);

        line.terminal.clear_output();
        let used = line.generate_container_preview(&line.flatjson[0], 28, false)?;
        assert_eq!(r#"[{a: 1, d: {…}, "b c": nu…}]"#, line.terminal.output());
        assert_eq!(28, used);

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
        let mut line: LinePrinter = LinePrinter {
            value_range: &(0..fj.1.len()),
            ..default_line_printer(&mut term, &fj, 0)
        };

        let expected = r#"{[true]: 1, [["t", "w", "o"]]: 2, [3]: 3, [null]: 4}"#;

        let _ = line.generate_container_preview(&line.flatjson[0], 100, true)?;
        assert_eq!(expected, line.terminal.output());

        line.terminal.clear_output();
        let _ = line.generate_container_preview(&line.flatjson[0], 100, false)?;
        assert_eq!(expected, line.terminal.output());

        Ok(())
    }
}
