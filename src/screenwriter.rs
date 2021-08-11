use termion::color::{Bg, Color, Fg};
use termion::{clear, cursor};

use super::flatjson::{ContainerType, OptionIndex, Row, Value};
use super::viewer::{JsonViewer, Mode};

pub struct ScreenWriter {
    pub tty_writer: Box<AnsiTTYWriter>,
}

impl ScreenWriter {
    pub fn print_screen(&mut self, viewer: &JsonViewer) {
        self.tty_writer.clear_screen();

        let mut line = OptionIndex::Index(viewer.top_row);
        for row_index in 0..viewer.height {
            match line {
                OptionIndex::Nil => {
                    self.tty_writer.position_cursor(1, row_index + 1);
                    write!(self.tty_writer, "~");
                }
                OptionIndex::Index(index) => {
                    let row = &viewer.flatjson[index];
                    self.print_line(row_index, row, index == viewer.focused_row);
                    line = match viewer.mode {
                        Mode::Line => viewer.flatjson.next_visible_row(index),
                        Mode::Data => viewer.flatjson.next_item(index),
                    };
                }
            }
        }
        self.tty_writer.flush();
    }

    fn print_line(&mut self, row_index: u16, row: &Row, is_focused: bool) {
        let col = 2 * row.depth as u16;
        self.tty_writer.position_cursor(col + 1, row_index + 1);

        if is_focused {
            write!(self.tty_writer, "* ");
        }

        if let Some(key) = &row.key {
            write!(self.tty_writer, "\"{}\": ", key);
        }

        match &row.value {
            Value::OpenContainer { container_type, .. } => match container_type {
                ContainerType::Object => self.tty_writer.write_char('{'),
                ContainerType::Array => self.tty_writer.write_char('['),
            },
            Value::CloseContainer { container_type, .. } => match container_type {
                ContainerType::Object => self.tty_writer.write_char('}'),
                ContainerType::Array => self.tty_writer.write_char(']'),
            },
            Value::Null => write!(self.tty_writer, "null,"),
            Value::Boolean(b) => match b {
                true => write!(self.tty_writer, "true,"),
                false => write!(self.tty_writer, "false,"),
            },
            Value::Number(n) => write!(self.tty_writer, "{},", n),
            Value::String(s) => write!(self.tty_writer, "{},", s),
            Value::EmptyObject => write!(self.tty_writer, "{{}},"),
            Value::EmptyArray => write!(self.tty_writer, "[],"),
            _ => std::io::Result::Ok(()),
        };
    }
}

pub trait TTYWriter {
    fn clear_screen(&mut self) -> std::io::Result<()>;
    fn clear_line(&mut self) -> std::io::Result<()>;
    fn position_cursor(&mut self, row: u16, col: u16) -> std::io::Result<()>;
    fn set_fg_color(&mut self, color: &dyn Color) -> std::io::Result<()>;
    fn set_bg_color(&mut self, color: &dyn Color) -> std::io::Result<()>;
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

    fn set_fg_color(&mut self, color: &dyn Color) -> std::io::Result<()> {
        write!(self.stdout, "{}", Fg(color))
    }

    fn set_bg_color(&mut self, color: &dyn Color) -> std::io::Result<()> {
        write!(self.stdout, "{}", Bg(color))
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
