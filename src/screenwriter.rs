use regex::Regex;
use rustyline::Editor;
use std::fmt::Write;
use termion::{clear, cursor};
use termion::{color, style};

use super::flatjson::{ContainerType, Index, OptionIndex, Row, Value};
use super::viewer::{JsonViewer, Mode};

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
}

const FOCUSED_LINE: &'static str = "▶ ";
const FOCUSED_COLLAPSED_CONTAINER: &'static str = "▶ ";
const FOCUSED_EXPANDED_CONTAINER: &'static str = "▼ ";
const COLLAPSED_CONTAINER: &'static str = "▷ ";
const EXPANDED_CONTAINER: &'static str = "▽ ";

lazy_static! {
    static ref JS_IDENTIFIER: Regex = Regex::new("^[_$a-zA-Z][_$a-zA-Z0-9]*$").unwrap();
}

impl ScreenWriter {
    pub fn print_screen(&mut self, viewer: &JsonViewer) {
        match self.print_screen_no_error_handling(viewer) {
            Ok(_) => {}
            Err(e) => {
                eprintln!("Error while printing to screen: {}", e);
            }
        }
    }

    pub fn print_screen_no_error_handling(&mut self, viewer: &JsonViewer) -> std::io::Result<()> {
        self.tty_writer.clear_screen()?;

        let mut line = OptionIndex::Index(viewer.top_row);
        for row_index in 0..viewer.height {
            match line {
                OptionIndex::Nil => {
                    self.tty_writer.position_cursor(1, row_index + 1)?;
                    write!(self.tty_writer, "~")?;
                }
                OptionIndex::Index(index) => {
                    let row = &viewer.flatjson[index];
                    self.print_line(viewer, row_index, row, index == viewer.focused_row)?;
                    line = match viewer.mode {
                        Mode::Line => viewer.flatjson.next_visible_row(index),
                        Mode::Data => viewer.flatjson.next_item(index),
                    };
                }
            }
        }

        self.print_status_bar(viewer)?;

        self.tty_writer.flush()
    }

    pub fn get_command(&mut self, viewer: &JsonViewer) -> rustyline::Result<String> {
        write!(self.tty_writer, "{}", termion::cursor::Show)?;
        self.tty_writer.position_cursor(1, viewer.height + 2)?;
        let result = self.command_editor.readline(":");
        write!(self.tty_writer, "{}", termion::cursor::Hide)?;

        self.tty_writer.position_cursor(1, viewer.height + 2)?;
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

    fn set_fg_color(&mut self, color: Color) -> std::io::Result<()> {
        self.tty_writer.set_fg_color(color)
    }

    fn reset_style(&mut self) -> std::io::Result<()> {
        write!(self.tty_writer, "{}", style::Reset)
    }

    fn bold(&mut self) -> std::io::Result<()> {
        write!(self.tty_writer, "{}", style::Bold)
    }

    fn print_line(
        &mut self,
        viewer: &JsonViewer,
        row_index: u16,
        row: &Row,
        is_focused: bool,
    ) -> std::io::Result<()> {
        let col = 2 * (row.depth + 1) as u16;
        if viewer.mode == Mode::Line && is_focused {
            self.tty_writer.position_cursor(1, row_index + 1)?;
            write!(self.tty_writer, "{}", FOCUSED_LINE)?;
        }

        if viewer.mode == Mode::Line {
            self.tty_writer.position_cursor(col + 1, row_index + 1)?;
        } else {
            self.tty_writer.position_cursor(col - 1, row_index + 1)?;
            if row.is_opening_of_container() {
                if row.is_expanded() {
                    if is_focused {
                        write!(self.tty_writer, "{}", FOCUSED_EXPANDED_CONTAINER)?;
                    } else {
                        write!(self.tty_writer, "{}", EXPANDED_CONTAINER)?;
                    }
                } else {
                    if is_focused {
                        write!(self.tty_writer, "{}", FOCUSED_COLLAPSED_CONTAINER)?;
                    } else {
                        write!(self.tty_writer, "{}", COLLAPSED_CONTAINER)?;
                    }
                }
            } else {
                write!(self.tty_writer, "  ")?;
            }
        }

        if let Some(key) = &row.key {
            if is_focused {
                self.invert_colors(Blue)?;
            } else {
                self.set_fg_color(LightBlue)?;
            }
            if viewer.mode == Mode::Line || !JS_IDENTIFIER.is_match(key) {
                write!(self.tty_writer, "\"{}\"", key)?;
            } else {
                write!(self.tty_writer, "{}", key)?;
            }
            self.reset_style()?;
            write!(self.tty_writer, ": ")?;
        }

        if let OptionIndex::Index(parent) = row.parent {
            if viewer.mode == Mode::Data
                && viewer.flatjson[parent].is_array()
                && !row.is_closing_of_container()
            {
                if !is_focused {
                    self.set_fg_color(LightBlack)?;
                } else {
                    self.bold()?;
                }
                write!(self.tty_writer, "[{}]", row.index)?;
                self.reset_style()?;
                write!(self.tty_writer, ": ")?;
            }
        }

        let mut bold_brace = false;
        if row.is_container() {
            let pair_index = row.pair_index().unwrap();
            if is_focused || viewer.focused_row == pair_index {
                bold_brace = true;
            }
        }

        match &row.value {
            Value::OpenContainer {
                container_type,
                collapsed,
                ..
            } => match container_type {
                ContainerType::Object => {
                    if *collapsed {
                        write!(self.tty_writer, "{{ ... }}")?
                    } else {
                        if viewer.mode == Mode::Line {
                            if bold_brace {
                                self.bold()?;
                            }
                            write!(self.tty_writer, "{{")?
                        } else {
                            if !is_focused {
                                self.set_fg_color(LightBlack)?;
                            }
                            write!(self.tty_writer, "Object")?
                        }
                    }
                }
                ContainerType::Array => {
                    if *collapsed {
                        write!(self.tty_writer, "[ ... ]")?
                    } else {
                        if viewer.mode == Mode::Line {
                            if bold_brace {
                                self.bold()?;
                            }
                            write!(self.tty_writer, "[")?
                        } else {
                            if !is_focused {
                                self.set_fg_color(LightBlack)?;
                            }
                            write!(self.tty_writer, "Array")?
                        }
                    }
                }
            },
            Value::CloseContainer { container_type, .. } => {
                if bold_brace {
                    self.bold()?;
                }
                match container_type {
                    ContainerType::Object => self.tty_writer.write_char('}')?,
                    ContainerType::Array => self.tty_writer.write_char(']')?,
                }
            }
            Value::Null => {
                self.set_fg_color(LightBlack)?;
                write!(self.tty_writer, "null")?;
            }
            Value::Boolean(b) => {
                self.set_fg_color(Yellow)?;
                write!(self.tty_writer, "{}", b)?;
            }
            Value::Number(n) => {
                self.set_fg_color(Magenta)?;
                write!(self.tty_writer, "{}", n)?;
            }
            Value::String(s) => {
                self.set_fg_color(Green)?;
                write!(self.tty_writer, "\"{}\"", s)?;
            }
            Value::EmptyObject => write!(self.tty_writer, "{{}}")?,
            Value::EmptyArray => write!(self.tty_writer, "[]")?,
        };

        self.reset_style()?;

        // Only print trailing comma in line mode.
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
                    self.tty_writer.write_char(',')?;
                }
            }
        }

        Ok(())
    }

    fn print_status_bar(&mut self, viewer: &JsonViewer) -> std::io::Result<()> {
        self.tty_writer.position_cursor(1, viewer.height + 1)?;
        self.invert_colors(Black)?;
        self.tty_writer.clear_line()?;
        self.tty_writer.position_cursor(1, viewer.height + 1)?;
        write!(
            self.tty_writer,
            "{}",
            ScreenWriter::get_path_to_focused_node(viewer)
        )?;
        self.tty_writer
            .position_cursor(viewer.width - 8, viewer.height + 1)?;
        write!(self.tty_writer, "FILE NAME")?;

        self.reset_style()?;
        self.tty_writer.position_cursor(1, viewer.height + 2)?;
        write!(self.tty_writer, ":")?;

        Ok(())
    }

    fn get_path_to_focused_node(viewer: &JsonViewer) -> String {
        let mut buf = String::new();
        write!(
            buf,
            "{}input{}",
            color::Fg(color::LightBlack),
            color::Fg(color::Black)
        )
        .unwrap();
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
