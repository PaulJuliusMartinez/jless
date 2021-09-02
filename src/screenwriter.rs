use regex::Regex;
use rustyline::Editor;
use std::fmt::Write;
use termion::{clear, cursor};
use termion::{color, style};
use unicode_width::UnicodeWidthStr;

use crate::flatjson::{Index, OptionIndex, Row, Value};
use crate::jless::MAX_BUFFER_SIZE;
use crate::lineprinter as lp;
use crate::truncate::TruncationResult::{DoesntFit, NoTruncation, Truncated};
use crate::truncate::{truncate_left_to_fit, truncate_right_to_fit};
use crate::tuicontrol::{Color as TUIColor, ColorControl};
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
    pub indentation_reduction: u16,
}

const PATH_BASE: &'static str = "input";
const SPACE_BETWEEN_PATH_AND_FILENAME: usize = 3;

lazy_static! {
    static ref JS_IDENTIFIER: Regex = Regex::new("^[_$a-zA-Z][_$a-zA-Z0-9]*$").unwrap();
}

impl ScreenWriter {
    pub fn print(&mut self, viewer: &JsonViewer, input_buffer: &[u8], input_filename: &str) {
        self.print_viewer(viewer);
        self.print_status_bar(viewer, input_buffer, input_filename);
    }

    pub fn print_viewer(&mut self, viewer: &JsonViewer) {
        match self.print_screen_impl(viewer) {
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
    ) {
        match self.print_status_bar_impl(viewer, input_buffer, input_filename) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error while printing status bar: {}", e);
            }
        }
    }

    fn print_screen_impl(&mut self, viewer: &JsonViewer) -> std::io::Result<()> {
        self.tty_writer.clear_screen()?;

        let mut line = OptionIndex::Index(viewer.top_row);
        for row_index in 0..viewer.dimensions.height {
            match line {
                OptionIndex::Nil => {
                    self.tty_writer.position_cursor(1, row_index + 1)?;
                    write!(self.tty_writer, "~")?;
                }
                OptionIndex::Index(index) => {
                    let row = &viewer.flatjson[index];
                    self.print_line(viewer, index, row_index, row, index == viewer.focused_row)?;
                    line = match viewer.mode {
                        Mode::Line => viewer.flatjson.next_visible_row(index),
                        Mode::Data => viewer.flatjson.next_item(index),
                    };
                }
            }
        }

        self.tty_writer.flush()
    }

    pub fn get_command(&mut self) -> rustyline::Result<String> {
        write!(self.tty_writer, "{}", termion::cursor::Show)?;
        self.tty_writer.position_cursor(1, self.dimensions.height)?;
        let result = self.command_editor.readline(":");
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

    fn print_line(
        &mut self,
        viewer: &JsonViewer,
        flatjson_index: Index,
        screen_index: u16,
        row: &Row,
        is_focused: bool,
    ) -> std::io::Result<()> {
        self.tty_writer.position_cursor(1, screen_index + 1)?;

        let depth = row
            .depth
            .saturating_sub(self.indentation_reduction as usize);

        let focused = is_focused;

        let mut label = None;
        let index_label: String;
        let number_value: String;

        // Set up key label.
        if let Some(key) = &row.key {
            label = Some(lp::LineLabel::Key {
                key,
                quoted: viewer.mode == Mode::Line || !JS_IDENTIFIER.is_match(key),
            });
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

        // TODO: It would be great if I could move this match out of here,
        // but I need a reference to the string representation of the Value::Number
        // value that lives long enough. The Container LineValue also uses some
        // local variables.
        let value = match &row.value {
            Value::OpenContainer { .. } | Value::CloseContainer { .. } => {
                lp::LineValue::Container {
                    flatjson: &viewer.flatjson,
                    row,
                    index: flatjson_index,
                }
            }
            Value::Null => lp::LineValue::Value {
                s: "null",
                quotes: false,
                color: TUIColor::LightBlack,
            },
            Value::Boolean(b) => lp::LineValue::Value {
                s: if *b { "true" } else { "false" },
                quotes: false,
                color: TUIColor::Yellow,
            },
            Value::Number(n) => {
                number_value = n.to_string();
                lp::LineValue::Value {
                    s: &number_value,
                    quotes: false,
                    color: TUIColor::Magenta,
                }
            }
            Value::String(s) => lp::LineValue::Value {
                s,
                quotes: true,
                color: TUIColor::Green,
            },
            Value::EmptyObject => lp::LineValue::Value {
                s: "{}",
                quotes: false,
                color: TUIColor::White,
            },
            Value::EmptyArray => lp::LineValue::Value {
                s: "[]",
                quotes: false,
                color: TUIColor::White,
            },
        };

        let mut secondarily_focused = false;
        if row.is_container() {
            let pair_index = row.pair_index().unwrap();
            if is_focused || viewer.focused_row == pair_index {
                secondarily_focused = true;
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

            if row_root.next_sibling.is_some() {
                if row.is_opening_of_container() && row.is_expanded() {
                    // Don't print trailing commas after { or [, but
                    // if it's collapsed, we do print one after the } or ].
                } else {
                    trailing_comma = true;
                }
            }
        }

        let line = lp::LinePrinter {
            mode: viewer.mode,
            tui: ColorControl {},

            depth,
            width: self.dimensions.width as usize,
            tab_size: 2,

            focused,
            secondarily_focused,
            trailing_comma,

            label,
            value,
        };

        let mut buf = String::new();
        // TODO: Handle error here? Or is never an error because writes
        // to String should never fail?
        line.print_line(&mut buf).unwrap();
        write!(self.tty_writer, "{}", buf)
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
            viewer.dimensions.width as usize,
        )?;

        self.reset_style()?;
        self.tty_writer.position_cursor(1, self.dimensions.height)?;
        write!(self.tty_writer, ":")?;

        self.tty_writer.position_cursor(
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
        width: usize,
    ) -> std::io::Result<()> {
        let base_len = PATH_BASE.len();
        let path_display_width = UnicodeWidthStr::width(path_to_node);
        let file_display_width = UnicodeWidthStr::width(file_name);

        let mut base_visible = true;
        let mut base_truncated = false;
        let mut base_ref = PATH_BASE;

        let mut path_ref = path_to_node;

        let mut file_visible = true;
        let mut file_truncated = false;
        let mut file_ref = file_name;
        let mut file_offset = file_display_width;

        let space_available_for_filename = width
            .saturating_sub(base_len)
            .saturating_sub(path_display_width)
            .saturating_sub(SPACE_BETWEEN_PATH_AND_FILENAME);

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
            let space_available_for_base = width.saturating_sub(path_display_width);

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

        if let Some(key) = &row.key {
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

    pub fn decrease_indentation_level(&mut self) {
        self.indentation_reduction = self.indentation_reduction.saturating_add(1)
    }

    pub fn increase_indentation_level(&mut self) {
        self.indentation_reduction = self.indentation_reduction.saturating_sub(1)
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
