use std::collections::HashMap;
use std::fmt::Write;
use std::iter::Peekable;
use std::ops::Range;

use rustyline::Editor;
use termion::{clear, cursor};
use termion::{color, style};
use unicode_width::UnicodeWidthStr;

use crate::app::MAX_BUFFER_SIZE;
use crate::flatjson::{Index, OptionIndex, Row, Value};
use crate::lineprinter as lp;
use crate::lineprinter::JS_IDENTIFIER;
use crate::search::{MatchRangeIter, SearchState};
use crate::terminal;
use crate::terminal::AnsiTerminal;
use crate::truncate::TruncationResult::{DoesntFit, NoTruncation, Truncated};
use crate::truncate::{truncate_left_to_fit, truncate_right_to_fit};
use crate::truncatedstrview::TruncatedStrView;
use crate::types::TTYDimensions;
use crate::viewer::{JsonViewer, Mode};

#[derive(Copy, Clone)]
#[allow(dead_code)]
pub enum Color {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    LightBlack,
    LightRed,
    LightGreen,
    LightYellow,
    LightBlue,
    LightMagenta,
    LightCyan,
    LightWhite,
}

use Color::*;

pub struct ScreenWriter {
    pub tty_writer: AnsiTTYWriter,
    pub command_editor: Editor<()>,
    pub dimensions: TTYDimensions,

    indentation_reduction: u16,
    truncated_row_value_views: HashMap<Index, TruncatedStrView>,
}

const PATH_BASE: &'static str = "input";
const SPACE_BETWEEN_PATH_AND_FILENAME: isize = 3;

impl ScreenWriter {
    pub fn init(
        tty_writer: AnsiTTYWriter,
        command_editor: Editor<()>,
        dimensions: TTYDimensions,
    ) -> Self {
        ScreenWriter {
            tty_writer,
            command_editor,
            dimensions,
            indentation_reduction: 0,
            truncated_row_value_views: HashMap::new(),
        }
    }

    pub fn print(
        &mut self,
        viewer: &JsonViewer,
        input_buffer: &[u8],
        input_filename: &str,
        search_state: &SearchState,
    ) {
        self.print_viewer(viewer, search_state);
        self.print_status_bar(viewer, input_buffer, input_filename, search_state);
    }

    pub fn print_viewer(&mut self, viewer: &JsonViewer, search_state: &SearchState) {
        match self.print_screen_impl(viewer, search_state) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error while printing viewer: {}", e);
            }
        }
    }

    pub fn print_status_bar(
        &mut self,
        viewer: &JsonViewer,
        input_buffer: &[u8],
        input_filename: &str,
        search_state: &SearchState,
    ) {
        match self.print_status_bar_impl(viewer, input_buffer, input_filename, search_state) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error while printing status bar: {}", e);
            }
        }
    }

    fn print_screen_impl(
        &mut self,
        viewer: &JsonViewer,
        search_state: &SearchState,
    ) -> std::io::Result<()> {
        self.tty_writer.clear_screen()?;

        let mut line = OptionIndex::Index(viewer.top_row);
        let mut search_matches = search_state
            .matches_iter(viewer.flatjson[line.unwrap()].range.start)
            .peekable();
        let current_match = search_state.current_match_range();

        for row_index in 0..viewer.dimensions.height {
            match line {
                OptionIndex::Nil => {
                    self.reset_style()?;
                    self.tty_writer.position_cursor(1, row_index + 1)?;
                    self.tty_writer.set_fg_color(LightBlack)?;
                    write!(self.tty_writer, "~")?;
                }
                OptionIndex::Index(index) => {
                    self.print_line(
                        viewer,
                        row_index,
                        index,
                        &mut search_matches,
                        &current_match,
                    )?;
                    line = match viewer.mode {
                        Mode::Line => viewer.flatjson.next_visible_row(index),
                        Mode::Data => viewer.flatjson.next_item(index),
                    };
                }
            }
        }

        self.tty_writer.flush()
    }

    pub fn get_command(&mut self, prompt: &str) -> rustyline::Result<String> {
        write!(self.tty_writer, "{}", termion::cursor::Show)?;
        self.tty_writer.position_cursor(1, self.dimensions.height)?;
        let result = self.command_editor.readline(prompt);
        write!(self.tty_writer, "{}", termion::cursor::Hide)?;

        self.tty_writer.position_cursor(1, self.dimensions.height)?;
        self.tty_writer.clear_line()?;

        match &result {
            Ok(line) => {
                write!(self.tty_writer, "Executed command: {}", line)?;
            }
            Err(err) => {
                write!(self.tty_writer, "Command error: {:?}", err)?;
            }
        }

        self.tty_writer.flush()?;
        result
    }

    fn invert_colors(&mut self, fg: Color) -> std::io::Result<()> {
        self.tty_writer.set_bg_color(LightWhite)?;
        self.tty_writer.set_fg_color(fg)
    }

    fn reset_style(&mut self) -> std::io::Result<()> {
        write!(self.tty_writer, "{}", style::Reset)
    }

    fn print_line<'a>(
        &mut self,
        viewer: &JsonViewer,
        screen_index: u16,
        index: Index,
        search_matches: &mut Peekable<MatchRangeIter>,
        focused_search_match: &Range<usize>,
    ) -> std::io::Result<()> {
        let is_focused = index == viewer.focused_row;

        self.tty_writer.position_cursor(1, screen_index + 1)?;
        let row = &viewer.flatjson[index];

        let depth = row
            .depth
            .saturating_sub(self.indentation_reduction as usize);

        let focused = is_focused;

        let mut label = None;
        let index_label: String;
        let mut label_range = &None;

        // Set up key label.
        if let Some(key_range) = &row.key_range {
            let key = &viewer.flatjson.1[key_range.start + 1..key_range.end - 1];
            label = Some(lp::LineLabel::Key { key });
            label_range = &row.key_range;
        }

        // Set up index label.
        if let OptionIndex::Index(parent) = row.parent {
            if viewer.mode == Mode::Data && viewer.flatjson[parent].is_array() {
                index_label = format!("{}", row.index);
                label = Some(lp::LineLabel::Index {
                    index: &index_label,
                });
            }
        }

        let value = match &row.value {
            Value::OpenContainer { .. } | Value::CloseContainer { .. } => {
                lp::LineValue::Container {
                    flatjson: &viewer.flatjson,
                    row,
                }
            }
            _ => {
                let color = match &row.value {
                    Value::Null => terminal::LIGHT_BLACK,
                    Value::Boolean => terminal::YELLOW,
                    Value::Number => terminal::MAGENTA,
                    Value::String => terminal::GREEN,
                    Value::EmptyObject => terminal::WHITE,
                    Value::EmptyArray => terminal::WHITE,
                    _ => terminal::WHITE,
                };

                let range = row.range.clone();
                let (s, quotes) = if let Value::String = &row.value {
                    (&viewer.flatjson.1[range.start + 1..range.end - 1], true)
                } else {
                    (&viewer.flatjson.1[range], false)
                };

                lp::LineValue::Value { s, quotes, color }
            }
        };

        let mut focused_because_matching_container_pair = false;
        if row.is_container() {
            let pair_index = row.pair_index().unwrap();
            if is_focused || viewer.focused_row == pair_index {
                focused_because_matching_container_pair = true;
            }
        }

        let mut trailing_comma = false;

        if viewer.mode == Mode::Line {
            // The next_sibling field isn't set for CloseContainer rows, so
            // we need to get the OpenContainer row before we check if a row
            // is the last row in a container, and thus whether we should
            // print a trailing comma or not.
            let row_root = if row.is_closing_of_container() {
                &viewer.flatjson[row.pair_index().unwrap()]
            } else {
                row
            };

            // Don't print trailing commas after top level elements.
            if row_root.parent.is_some() && row_root.next_sibling.is_some() {
                if row.is_opening_of_container() && row.is_expanded() {
                    // Don't print trailing commas after { or [, but
                    // if it's collapsed, we do print one after the } or ].
                } else {
                    trailing_comma = true;
                }
            }
        }

        let search_matches_copy = (*search_matches).clone();
        let mut terminal = AnsiTerminal::new(String::new());

        let mut line = lp::LinePrinter {
            mode: viewer.mode,
            terminal: &mut terminal,

            depth,
            width: self.dimensions.width as usize,
            tab_size: 2,

            focused,
            focused_because_matching_container_pair,
            trailing_comma,

            label,
            label_range,
            value,
            value_range: &row.range,

            search_matches: Some(search_matches_copy),
            focused_search_match,

            cached_formatted_value: Some(self.truncated_row_value_views.entry(index)),
        };

        // TODO: Handle error here? Or is never an error because writes
        // to String should never fail?
        line.print_line().unwrap();

        *search_matches = line.search_matches.unwrap();

        write!(self.tty_writer, "{}", terminal.output)
    }

    fn line_primitive_value_ref<'a, 'b>(
        &'a self,
        row: &'a Row,
        viewer: &'b JsonViewer,
    ) -> Option<&'b str> {
        match &row.value {
            Value::OpenContainer { .. } | Value::CloseContainer { .. } => None,
            _ => {
                let range = row.range.clone();
                if let Value::String = &row.value {
                    Some(&viewer.flatjson.1[range.start + 1..range.end - 1])
                } else {
                    Some(&viewer.flatjson.1[range])
                }
            }
        }
    }

    // input.data.viewer.gameDetail.plays[3].playStats[0].gsisPlayer.id filename.>
    // input.data.viewer.gameDetail.plays[3].playStats[0].gsisPlayer.id fi>
    // // Path also shrinks if needed
    // <.data.viewer.gameDetail.plays[3].playStats[0].gsisPlayer.id

    fn print_status_bar_impl(
        &mut self,
        viewer: &JsonViewer,
        input_buffer: &[u8],
        input_filename: &str,
        search_state: &SearchState,
    ) -> std::io::Result<()> {
        self.tty_writer
            .position_cursor(1, self.dimensions.height - 1)?;
        self.invert_colors(Black)?;
        self.tty_writer.clear_line()?;
        self.tty_writer
            .position_cursor(1, self.dimensions.height - 1)?;

        let path_to_node = ScreenWriter::get_path_to_focused_node(viewer);
        self.print_path_to_node_and_file_name(
            &path_to_node,
            &input_filename,
            viewer.dimensions.width as isize,
        )?;

        self.reset_style()?;
        self.tty_writer.position_cursor(1, self.dimensions.height)?;

        if let Some((match_num, just_wrapped)) = search_state.active_search_state() {
            self.tty_writer
                .write_char(search_state.direction.prompt_char())?;
            write!(self.tty_writer, "{}", &search_state.search_term)?;

            // Print out which match we're on:
            let match_tracker = format!("[{}/{}]", match_num + 1, search_state.num_matches());
            self.tty_writer.position_cursor(
                self.dimensions.width
                    - (1 + MAX_BUFFER_SIZE as u16)
                    - (3 + match_tracker.len() as u16 + 3),
                self.dimensions.height,
            )?;

            let wrapped_char = if just_wrapped { 'W' } else { ' ' };
            write!(self.tty_writer, " {} {}", wrapped_char, match_tracker)?;
        } else {
            write!(self.tty_writer, ":")?;
        }

        self.tty_writer.position_cursor(
            // TODO: This can overflow on very skinny screens (2-3 columns).
            self.dimensions.width - (1 + MAX_BUFFER_SIZE as u16),
            self.dimensions.height,
        )?;
        write!(
            self.tty_writer,
            "{}",
            std::str::from_utf8(input_buffer).unwrap()
        )?;

        // Position the cursor better for random debugging prints. (2 so it's after ':')
        self.tty_writer.position_cursor(2, self.dimensions.height)?;

        self.tty_writer.flush()
    }

    fn print_path_to_node_and_file_name(
        &mut self,
        path_to_node: &str,
        file_name: &str,
        width: isize,
    ) -> std::io::Result<()> {
        let base_len = PATH_BASE.len() as isize;
        let path_display_width = UnicodeWidthStr::width(path_to_node) as isize;
        let file_display_width = UnicodeWidthStr::width(file_name) as isize;

        let mut base_visible = true;
        let mut base_truncated = false;
        let mut base_ref = PATH_BASE;

        let mut path_ref = path_to_node;

        let mut file_visible = true;
        let mut file_truncated = false;
        let mut file_ref = file_name;
        let mut file_offset = file_display_width;

        let space_available_for_filename =
            width - base_len - path_display_width - SPACE_BETWEEN_PATH_AND_FILENAME;

        match truncate_right_to_fit(file_name, space_available_for_filename, ">") {
            NoTruncation(_) => { /* Don't need to truncate filename */ }
            Truncated(filename_prefix, width) => {
                file_ref = filename_prefix;
                file_offset = width;
                file_truncated = true;
            }
            DoesntFit => {
                file_visible = false;
            }
        };

        // Might need to truncate path if we're not showing the file.
        if !file_visible {
            let space_available_for_base = width - path_display_width;

            match truncate_left_to_fit(PATH_BASE, space_available_for_base, "<") {
                NoTruncation(_) => { /* Don't need to truncate base */ }
                Truncated(base_suffix, _) => {
                    base_ref = base_suffix;
                    base_truncated = true;
                }
                DoesntFit => {
                    base_visible = false;
                }
            };
        }

        // Might need to truncate path if we're not showing the the base ref.
        if !base_visible {
            match truncate_left_to_fit(path_to_node, width, "<") {
                NoTruncation(_) => { /* Don't need to truncate path */ }
                Truncated(path_suffix, _) => {
                    path_ref = path_suffix;
                }
                DoesntFit => {
                    panic!("Not enough room to display any of path.");
                }
            };
        }

        // Print the remaining bits of the base_ref and the path ref.
        if base_visible {
            write!(
                self.tty_writer,
                "{}{}{}{}",
                color::Fg(color::LightBlack),
                if base_truncated { "<" } else { "" },
                base_ref,
                color::Fg(color::Black)
            )?;
        }

        if !base_visible {
            write!(self.tty_writer, "<")?;
        }
        write!(self.tty_writer, "{}", path_ref)?;

        if file_visible {
            self.tty_writer.position_cursor(
                // 1 indexed
                1 + self.dimensions.width - (file_offset as u16),
                self.dimensions.height - 1,
            )?;
            write!(self.tty_writer, "{}", file_ref)?;

            if file_truncated {
                write!(self.tty_writer, ">")?;
            }
        }

        Ok(())
    }

    fn get_path_to_focused_node(viewer: &JsonViewer) -> String {
        let mut buf = String::new();
        ScreenWriter::build_path_to_focused_node(viewer, &mut buf, viewer.focused_row);
        buf
    }

    fn build_path_to_focused_node(viewer: &JsonViewer, buf: &mut String, index: Index) {
        let row = &viewer.flatjson[index];

        if row.is_closing_of_container() {
            return ScreenWriter::build_path_to_focused_node(
                viewer,
                buf,
                row.pair_index().unwrap(),
            );
        }

        if let OptionIndex::Index(parent_index) = row.parent {
            ScreenWriter::build_path_to_focused_node(viewer, buf, parent_index);
        }

        if let Some(key_range) = &row.key_range {
            let key = &viewer.flatjson.1[key_range.start + 1..key_range.end - 1];

            if JS_IDENTIFIER.is_match(key) {
                write!(buf, ".{}", key).unwrap();
            } else {
                write!(buf, "[\"{}\"]", key).unwrap();
            }
        } else {
            if index == 0 && row.next_sibling.is_nil() {
                // Don't print out an array index if there is only one top level item.
            } else {
                write!(buf, "[{}]", row.index).unwrap();
            }
        }
    }

    pub fn decrease_indentation_level(&mut self, max_depth: u16) {
        self.indentation_reduction = self.indentation_reduction.saturating_add(1).min(max_depth);
    }

    pub fn increase_indentation_level(&mut self) {
        self.indentation_reduction = self.indentation_reduction.saturating_sub(1)
    }

    pub fn scroll_focused_line_right(&mut self, viewer: &JsonViewer, count: usize) {
        self.scroll_focused_line(viewer, count, true);
    }

    pub fn scroll_focused_line_left(&mut self, viewer: &JsonViewer, count: usize) {
        self.scroll_focused_line(viewer, count, false);
    }

    pub fn scroll_focused_line(&mut self, viewer: &JsonViewer, count: usize, to_right: bool) {
        let row = viewer.focused_row;
        let tsv = self.truncated_row_value_views.get(&row);
        if let Some(tsv) = tsv {
            if tsv.range.is_none() {
                return;
            }

            // Make tsv not a reference.
            let mut tsv = *tsv;
            let value_ref = self
                .line_primitive_value_ref(&viewer.flatjson[row], &viewer)
                .unwrap();
            if to_right {
                tsv = tsv.scroll_right(value_ref, count);
            } else {
                tsv = tsv.scroll_left(value_ref, count);
            }
            self.truncated_row_value_views
                .insert(viewer.focused_row, tsv);
        }
    }

    pub fn scroll_focused_line_to_an_end(&mut self, viewer: &JsonViewer) {
        let row = viewer.focused_row;
        let tsv = self.truncated_row_value_views.get(&row);
        if let Some(tsv) = tsv {
            if tsv.range.is_none() {
                return;
            }

            // Make tsv not a reference.
            let mut tsv = *tsv;
            let value_ref = self
                .line_primitive_value_ref(&viewer.flatjson[row], &viewer)
                .unwrap();
            tsv = tsv.jump_to_an_end(value_ref);
            self.truncated_row_value_views
                .insert(viewer.focused_row, tsv);
        }
    }

    pub fn scroll_line_to_search_match(
        &mut self,
        viewer: &JsonViewer,
        focused_search_range: Range<usize>,
    ) {
        let row = viewer.focused_row;
        let tsv = self.truncated_row_value_views.get(&row);
        if let Some(tsv) = tsv {
            // Make tsv not a reference.
            let mut tsv = *tsv;
            if tsv.range.is_none() {
                return;
            }

            let json_row = &viewer.flatjson[row];
            let value_ref = self.line_primitive_value_ref(json_row, &viewer).unwrap();

            let mut range = json_row.range.clone();
            if json_row.is_string() {
                range.start += 1;
                range.end -= 1;
            }

            let no_overlap =
                focused_search_range.end <= range.start || range.end <= focused_search_range.start;
            if no_overlap {
                return;
            }

            let mut value_range_start = range.start;
            if let Value::String = &json_row.value {
                value_range_start += 1;
            }

            let offset_focused_range = Range {
                start: focused_search_range.start.saturating_sub(value_range_start),
                end: focused_search_range.end - value_range_start,
            };

            tsv = tsv.focus(value_ref, &offset_focused_range);

            self.truncated_row_value_views
                .insert(viewer.focused_row, tsv);
        }
    }
}

pub trait TTYWriter {
    fn clear_screen(&mut self) -> std::io::Result<()>;
    fn clear_line(&mut self) -> std::io::Result<()>;
    fn position_cursor(&mut self, row: u16, col: u16) -> std::io::Result<()>;
    fn set_fg_color(&mut self, color: Color) -> std::io::Result<()>;
    fn set_bg_color(&mut self, color: Color) -> std::io::Result<()>;
    fn write(&mut self, s: &str) -> std::io::Result<()>;
    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> std::io::Result<()>;
    fn write_char(&mut self, c: char) -> std::io::Result<()>;
    fn flush(&mut self) -> std::io::Result<()>;
}

pub struct AnsiTTYWriter {
    pub stdout: Box<dyn std::io::Write>,
    pub color: bool,
}

impl TTYWriter for AnsiTTYWriter {
    fn clear_screen(&mut self) -> std::io::Result<()> {
        write!(self.stdout, "{}", clear::All)
    }

    fn clear_line(&mut self) -> std::io::Result<()> {
        write!(self.stdout, "{}", clear::CurrentLine)
    }

    fn position_cursor(&mut self, row: u16, col: u16) -> std::io::Result<()> {
        write!(self.stdout, "{}", cursor::Goto(row, col))
    }

    fn set_fg_color(&mut self, color: Color) -> std::io::Result<()> {
        match color {
            Black => write!(self.stdout, "{}", color::Fg(color::Black)),
            Red => write!(self.stdout, "{}", color::Fg(color::Red)),
            Green => write!(self.stdout, "{}", color::Fg(color::Green)),
            Yellow => write!(self.stdout, "{}", color::Fg(color::Yellow)),
            Blue => write!(self.stdout, "{}", color::Fg(color::Blue)),
            Magenta => write!(self.stdout, "{}", color::Fg(color::Magenta)),
            Cyan => write!(self.stdout, "{}", color::Fg(color::Cyan)),
            White => write!(self.stdout, "{}", color::Fg(color::White)),
            LightBlack => write!(self.stdout, "{}", color::Fg(color::LightBlack)),
            LightRed => write!(self.stdout, "{}", color::Fg(color::LightRed)),
            LightGreen => write!(self.stdout, "{}", color::Fg(color::LightGreen)),
            LightYellow => write!(self.stdout, "{}", color::Fg(color::LightYellow)),
            LightBlue => write!(self.stdout, "{}", color::Fg(color::LightBlue)),
            LightMagenta => write!(self.stdout, "{}", color::Fg(color::LightMagenta)),
            LightCyan => write!(self.stdout, "{}", color::Fg(color::LightCyan)),
            LightWhite => write!(self.stdout, "{}", color::Fg(color::LightWhite)),
        }
    }

    fn set_bg_color(&mut self, color: Color) -> std::io::Result<()> {
        match color {
            Black => write!(self.stdout, "{}", color::Bg(color::Black)),
            Red => write!(self.stdout, "{}", color::Bg(color::Red)),
            Green => write!(self.stdout, "{}", color::Bg(color::Green)),
            Yellow => write!(self.stdout, "{}", color::Bg(color::Yellow)),
            Blue => write!(self.stdout, "{}", color::Bg(color::Blue)),
            Magenta => write!(self.stdout, "{}", color::Bg(color::Magenta)),
            Cyan => write!(self.stdout, "{}", color::Bg(color::Cyan)),
            White => write!(self.stdout, "{}", color::Bg(color::White)),
            LightBlack => write!(self.stdout, "{}", color::Bg(color::LightBlack)),
            LightRed => write!(self.stdout, "{}", color::Bg(color::LightRed)),
            LightGreen => write!(self.stdout, "{}", color::Bg(color::LightGreen)),
            LightYellow => write!(self.stdout, "{}", color::Bg(color::LightYellow)),
            LightBlue => write!(self.stdout, "{}", color::Bg(color::LightBlue)),
            LightMagenta => write!(self.stdout, "{}", color::Bg(color::LightMagenta)),
            LightCyan => write!(self.stdout, "{}", color::Bg(color::LightCyan)),
            LightWhite => write!(self.stdout, "{}", color::Bg(color::LightWhite)),
        }
    }

    fn write(&mut self, s: &str) -> std::io::Result<()> {
        self.stdout.write(s.as_bytes()).map(|_| ())
    }

    fn write_fmt(&mut self, args: std::fmt::Arguments<'_>) -> std::io::Result<()> {
        self.stdout.write_fmt(args)
    }

    fn write_char(&mut self, c: char) -> std::io::Result<()> {
        write!(self.stdout, "{}", c)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.stdout.flush()
    }
}
