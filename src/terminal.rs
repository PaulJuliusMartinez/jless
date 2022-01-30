use std::fmt::{Result, Write};

#[derive(Copy, Clone, Debug, Eq, PartialEq)]
pub enum Color {
    C16(u8),
    Default,
}

pub const BLACK: Color = Color::C16(0);
pub const RED: Color = Color::C16(1);
pub const GREEN: Color = Color::C16(2);
pub const YELLOW: Color = Color::C16(3);
pub const BLUE: Color = Color::C16(4);
pub const MAGENTA: Color = Color::C16(5);
pub const CYAN: Color = Color::C16(6);
pub const WHITE: Color = Color::C16(7);
pub const LIGHT_BLACK: Color = Color::C16(8);
pub const LIGHT_RED: Color = Color::C16(9);
pub const LIGHT_GREEN: Color = Color::C16(10);
pub const LIGHT_YELLOW: Color = Color::C16(11);
pub const LIGHT_BLUE: Color = Color::C16(12);
pub const LIGHT_MAGENTA: Color = Color::C16(13);
pub const LIGHT_CYAN: Color = Color::C16(14);
pub const LIGHT_WHITE: Color = Color::C16(15);
pub const DEFAULT: Color = Color::Default;

pub struct Style {
    pub fg: Color,
    pub bg: Color,
    pub inverted: bool,
    pub bold: bool,
}

impl Style {
    pub const fn default() -> Self {
        Style {
            fg: Color::Default,
            bg: Color::Default,
            inverted: false,
            bold: false,
        }
    }
}

impl Default for Style {
    fn default() -> Self {
        Style::default()
    }
}

pub trait Terminal: Write {
    fn clear_screen(&mut self) -> Result;

    fn position_cursor(&mut self, row: u16, col: u16) -> Result;
    fn position_cursor_col(&mut self, col: u16) -> Result;

    fn set_style(&mut self, style: &Style) -> Result;
    fn reset_style(&mut self) -> Result;

    fn set_fg(&mut self, color: Color) -> Result;
    fn set_bg(&mut self, color: Color) -> Result;
    fn set_inverted(&mut self, inverted: bool) -> Result;
    fn set_bold(&mut self, bold: bool) -> Result;
}

pub struct AnsiTerminal {
    pub output: String,
    pub style: Style,
}

impl AnsiTerminal {
    pub fn new(output: String) -> Self {
        AnsiTerminal {
            output,
            style: Style::default(),
        }
    }
}

impl Write for AnsiTerminal {
    fn write_str(&mut self, s: &str) -> Result {
        self.output.write_str(s)
    }
}

impl Terminal for AnsiTerminal {
    fn clear_screen(&mut self) -> Result {
        write!(self, "\x1b[2J")
    }

    fn position_cursor(&mut self, row: u16, col: u16) -> Result {
        write!(self, "\x1b[{};{}H", row, col)?;
        self.reset_style()
    }

    fn position_cursor_col(&mut self, col: u16) -> Result {
        write!(self, "\x1b[{}G", col)?;
        self.reset_style()
    }

    fn set_style(&mut self, style: &Style) -> Result {
        self.set_fg(style.fg)?;
        self.set_bg(style.bg)?;
        self.set_inverted(style.inverted)?;
        self.set_bold(style.bold)?;
        Ok(())
    }

    fn reset_style(&mut self) -> Result {
        self.style = Style::default();
        write!(self, "\x1b[0m")
    }

    fn set_fg(&mut self, color: Color) -> Result {
        if self.style.fg != color {
            match color {
                Color::C16(c) => write!(self, "\x1b[38;5;{}m", c)?,
                Color::Default => write!(self, "\x1b[39m")?,
            }
            self.style.fg = color;
        }
        Ok(())
    }

    fn set_bg(&mut self, color: Color) -> Result {
        if self.style.bg != color {
            match color {
                Color::C16(c) => write!(self, "\x1b[48;5;{}m", c)?,
                Color::Default => write!(self, "\x1b[49m")?,
            }
            self.style.bg = color;
        }
        Ok(())
    }

    fn set_inverted(&mut self, inverted: bool) -> Result {
        if self.style.inverted != inverted {
            if inverted {
                write!(self, "\x1b[7m")?;
            } else {
                write!(self, "\x1b[27m")?;
            }
            self.style.inverted = inverted;
        }
        Ok(())
    }

    fn set_bold(&mut self, bold: bool) -> Result {
        if self.style.bold != bold {
            if bold {
                write!(self, "\x1b[1m")?;
            } else {
                write!(self, "\x1b[22m")?;
            }
            self.style.bold = bold;
        }
        Ok(())
    }
}
