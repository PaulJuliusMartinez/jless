use std::collections::HashMap;
use std::fmt::Write;
use std::iter::Peekable;
use std::ops::Range;

use rustyline::DefaultEditor;
use termion::raw::RawTerminal;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::MAX_BUFFER_SIZE;
use crate::flatjson::{Index, OptionIndex, PathType, Row, Value};
use crate::lineprinter as lp;
use crate::lineprinter::LineNumber;
use crate::options::Opt;
use crate::search::{MatchRangeIter, SearchState};
use crate::terminal;
use crate::terminal::{AnsiTerminal, Terminal};
use crate::truncatedstrview::{TruncatedStrSlice, TruncatedStrView};
use crate::types::TTYDimensions;
use crate::viewer::{JsonViewer, Mode};

pub struct ScreenWriter {
    pub stdout: RawTerminal<Box<dyn std::io::Write>>,
    pub command_editor: DefaultEditor,
    pub dimensions: TTYDimensions,
    pub terminal: AnsiTerminal,

    pub show_line_numbers: bool,
    pub show_relative_line_numbers: bool,

    indentation_reduction: u16,
    truncated_row_value_views: HashMap<Index, TruncatedStrView>,
}

pub enum MessageSeverity {
    Info,
    Warn,
    Error,
}

impl MessageSeverity {
    pub fn color(&self) -> terminal::Color {
        match self {
            MessageSeverity::Info => terminal::WHITE,
            MessageSeverity::Warn => terminal::YELLOW,
            MessageSeverity::Error => terminal::RED,
        }
    }
}

const TAB_SIZE: isize = 2;
const PATH_BASE: &str = "input";
const SPACE_BETWEEN_PATH_AND_FILENAME: isize = 3;

impl ScreenWriter {
    pub fn init(
        options: &Opt,
        stdout: RawTerminal<Box<dyn std::io::Write>>,
        command_editor: DefaultEditor,
        dimensions: TTYDimensions,
    ) -> Self {
        ScreenWriter {
            stdout,
            command_editor,
            dimensions,
            terminal: AnsiTerminal::new(String::new()),
            show_line_numbers: options.show_line_numbers,
            show_relative_line_numbers: options.show_relative_line_numbers,
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
        message: &Option<(String, MessageSeverity)>,
    ) {
        self.print_viewer(viewer, search_state);
        self.print_status_bar(viewer, input_buffer, input_filename, search_state, message);
    }

    pub fn print_viewer(&mut self, viewer: &JsonViewer, search_state: &SearchState) {
        match self.print_screen_impl(viewer, search_state) {
            Ok(_) => match self.terminal.flush_contents(&mut self.stdout) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("Error while printing viewer: {e}");
                }
            },
            Err(e) => {
                eprintln!("Error while printing viewer: {e}");
            }
        }
    }

    pub fn print_status_bar(
        &mut self,
        viewer: &JsonViewer,
        input_buffer: &[u8],
        input_filename: &str,
        search_state: &SearchState,
        message: &Option<(String, MessageSeverity)>,
    ) {
        match self.print_status_bar_impl(
            viewer,
            input_buffer,
            input_filename,
            search_state,
            message,
        ) {
            Ok(_) => match self.terminal.flush_contents(&mut self.stdout) {
                Ok(_) => {}
                Err(e) => {
                    eprintln!("Error while printing status bar: {e}");
                }
            },
            Err(e) => {
                eprintln!("Error while printing status bar: {e}");
            }
        }
    }

    fn print_screen_impl(
        &mut self,
        viewer: &JsonViewer,
        search_state: &SearchState,
    ) -> std::fmt::Result {
        let mut line = OptionIndex::Index(viewer.top_row);
        let mut search_matches = search_state
            .matches_iter(viewer.flatjson[line.unwrap()].range.start)
            .peekable();
        let current_match = search_state.current_match_range();

        let mut delta_to_focused_row = viewer.index_of_focused_row_on_screen() as isize;

        for row_index in 0..viewer.dimensions.height {
            match line {
                OptionIndex::Nil => {
                    self.terminal.position_cursor(1, row_index + 1)?;
                    self.terminal.clear_line()?;
                    self.terminal.set_fg(terminal::LIGHT_BLACK)?;
                    self.terminal.write_char('~')?;
                }
                OptionIndex::Index(index) => {
                    self.print_line(
                        viewer,
                        row_index,
                        index,
                        delta_to_focused_row,
                        &mut search_matches,
                        &current_match,
                    )?;
                    line = match viewer.mode {
                        Mode::Line => viewer.flatjson.next_visible_row(index),
                        Mode::Data => viewer.flatjson.next_item(index),
                    };
                }
            }

            delta_to_focused_row -= 1;
        }

        Ok(())
    }

    pub fn get_command(&mut self, prompt: &str) -> rustyline::Result<String> {
        write!(self.stdout, "{}", termion::cursor::Show)?;
        let _ = self.terminal.position_cursor(1, self.dimensions.height);
        self.terminal.flush_contents(&mut self.stdout)?;

        let result = self.command_editor.readline(prompt);
        write!(self.stdout, "{}", termion::cursor::Hide)?;

        let _ = self.terminal.position_cursor(1, self.dimensions.height);
        let _ = self.terminal.clear_line();
        self.terminal.flush_contents(&mut self.stdout)?;

        result
    }

    fn print_line(
        &mut self,
        viewer: &JsonViewer,
        screen_index: u16,
        index: Index,
        delta_to_focused_row: isize,
        search_matches: &mut Peekable<MatchRangeIter>,
        focused_search_match: &Range<usize>,
    ) -> std::fmt::Result {
        let is_focused = index == viewer.focused_row;

        self.terminal.position_cursor(1, screen_index + 1)?;
        self.terminal.clear_line()?;
        let row = &viewer.flatjson[index];

        let indentation_level =
            row.depth
                .saturating_sub(self.indentation_reduction as usize) as isize;
        let indentation = indentation_level * TAB_SIZE;

        let focused = is_focused;

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

        let mut absolute_line_number = None;
        let mut relative_line_number = None;
        let max_line_number_width = isize::max(
            2,
            isize::ilog10(viewer.flatjson.0.len() as isize + 1) as isize + 1,
        );

        if self.show_line_numbers {
            absolute_line_number = Some(index + 1);
        }
        if self.show_relative_line_numbers {
            relative_line_number = Some(delta_to_focused_row.unsigned_abs());
        }

        let mut line = lp::LinePrinter {
            mode: viewer.mode,
            terminal: &mut self.terminal,

            flatjson: &viewer.flatjson,
            row,
            line_number: LineNumber {
                absolute: absolute_line_number,
                relative: relative_line_number,
                max_width: max_line_number_width,
            },

            width: self.dimensions.width as isize,
            indentation,

            focused,
            focused_because_matching_container_pair,
            trailing_comma,

            search_matches: Some(search_matches_copy),
            focused_search_match,
            // This is only used internally and really shouldn't be exposed.
            emphasize_focused_search_match: true,

            cached_truncated_value: Some(self.truncated_row_value_views.entry(index)),
        };

        // TODO: Handle error here? Or is never an error because writes
        // to String should never fail?
        line.print_line().unwrap();

        *search_matches = line.search_matches.unwrap();

        Ok(())
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

    fn print_status_bar_impl(
        &mut self,
        viewer: &JsonViewer,
        input_buffer: &[u8],
        input_filename: &str,
        search_state: &SearchState,
        message: &Option<(String, MessageSeverity)>,
    ) -> std::fmt::Result {
        self.terminal
            .position_cursor(1, self.dimensions.height - 1)?;
        self.terminal.clear_line()?;
        self.terminal.set_style(&terminal::Style {
            inverted: true,
            ..terminal::Style::default()
        })?;
        // Need to print a line to ensure the entire bar with the path to
        // the node and the filename is highlighted.
        for _ in 0..self.dimensions.width {
            self.terminal.write_char(' ')?;
        }
        self.terminal.write_char('\r')?;

        let path_to_node = viewer
            .flatjson
            .build_path_to_node(PathType::DotWithTopLevelIndex, viewer.focused_row)
            .unwrap();
        self.print_path_to_node_and_file_name(
            &path_to_node,
            input_filename,
            viewer.dimensions.width as isize,
        )?;

        self.terminal.position_cursor(1, self.dimensions.height)?;
        self.terminal.clear_line()?;

        if let Some((contents, severity)) = message {
            self.terminal.set_style(&terminal::Style {
                fg: severity.color(),
                ..terminal::Style::default()
            })?;
            self.terminal.write_str(contents)?;
        } else if search_state.showing_matches() {
            self.terminal
                .write_char(search_state.direction.prompt_char())?;
            self.terminal.write_str(&search_state.search_term)?;

            if let Some((match_num, just_wrapped)) = search_state.active_search_state() {
                // Print out which match we're on:
                let match_tracker = format!("[{}/{}]", match_num + 1, search_state.num_matches());
                self.terminal.position_cursor(
                    self.dimensions.width
                        - (1 + MAX_BUFFER_SIZE as u16)
                        - (3 + match_tracker.len() as u16 + 3),
                    self.dimensions.height,
                )?;

                let wrapped_char = if just_wrapped { 'W' } else { ' ' };
                write!(self.terminal, " {wrapped_char} {match_tracker}")?;
            }
        } else {
            write!(self.terminal, ":")?;
        }

        self.terminal.position_cursor(
            // TODO: This can overflow on very skinny screens (2-3 columns).
            self.dimensions.width - (1 + MAX_BUFFER_SIZE as u16),
            self.dimensions.height,
        )?;
        self.terminal
            .write_str(std::str::from_utf8(input_buffer).unwrap())?;

        // Position the cursor better for random debugging prints. (2 so it's after ':')
        self.terminal.position_cursor(2, self.dimensions.height)?;

        Ok(())
    }

    // input.data.viewer.gameDetail.plays[3].playStats[0].gsisPlayer.id filename.>
    // input.data.viewer.gameDetail.plays[3].playStats[0].gsisPlayer.id fi>
    // // Path also shrinks if needed
    // <.data.viewer.gameDetail.plays[3].playStats[0].gsisPlayer.id
    fn print_path_to_node_and_file_name(
        &mut self,
        path_to_node: &str,
        filename: &str,
        width: isize,
    ) -> std::fmt::Result {
        let base_len = PATH_BASE.len() as isize;
        let path_display_width = UnicodeWidthStr::width(path_to_node) as isize;
        let row = self.dimensions.height - 1;

        let space_available_for_filename =
            width - base_len - path_display_width - SPACE_BETWEEN_PATH_AND_FILENAME;
        let mut space_available_for_base = width - path_display_width;

        let inverted_style = terminal::Style {
            inverted: true,
            ..terminal::Style::default()
        };

        let truncated_filename =
            TruncatedStrView::init_start(filename, space_available_for_filename);

        if truncated_filename.any_contents_visible() {
            let filename_width = truncated_filename.used_space().unwrap();
            space_available_for_base -= filename_width - SPACE_BETWEEN_PATH_AND_FILENAME;
        }

        let truncated_base = TruncatedStrView::init_back(PATH_BASE, space_available_for_base);

        self.terminal.position_cursor(1, row)?;
        self.terminal.set_style(&inverted_style)?;
        self.terminal.set_bg(terminal::LIGHT_BLACK)?;

        let base_slice = TruncatedStrSlice {
            s: PATH_BASE,
            truncated_view: &truncated_base,
        };

        write!(self.terminal, "{base_slice}")?;

        self.terminal.set_bg(terminal::DEFAULT)?;

        // If the path is the exact same width as the screen, we won't print out anything
        // for the PATH_BASE, and the path won't be truncated. But there is truncated
        // content (the PATH_BASE), so we'll just manually handle this case.
        if truncated_base.used_space().is_none() && path_display_width == width {
            self.terminal.write_char('…')?;
            let mut graphemes = path_to_node.graphemes(true);
            // Skip one character.
            graphemes.next();
            self.terminal.write_str(graphemes.as_str())?;
        } else {
            let path_slice = TruncatedStrSlice {
                s: path_to_node,
                truncated_view: &TruncatedStrView::init_back(path_to_node, width),
            };

            write!(self.terminal, "{path_slice}")?;
        }

        if truncated_filename.any_contents_visible() {
            let filename_width = truncated_filename.used_space().unwrap();

            self.terminal
                .position_cursor(self.dimensions.width - (filename_width as u16) + 1, row)?;
            self.terminal.set_style(&inverted_style)?;

            let truncated_slice = TruncatedStrSlice {
                s: filename,
                truncated_view: &truncated_filename,
            };

            write!(self.terminal, "{truncated_slice}")?;
        }

        Ok(())
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
                .line_primitive_value_ref(&viewer.flatjson[row], viewer)
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
                .line_primitive_value_ref(&viewer.flatjson[row], viewer)
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
            let value_ref = self.line_primitive_value_ref(json_row, viewer).unwrap();

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
