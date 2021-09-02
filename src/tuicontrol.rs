use std::fmt::{Result, Write};

#[derive(Copy, Clone, Debug)]
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

impl Color {
    fn id(&self) -> u8 {
        match self {
            Color::Black => 0,
            Color::Red => 1,
            Color::Green => 2,
            Color::Yellow => 3,
            Color::Blue => 4,
            Color::Magenta => 5,
            Color::Cyan => 6,
            Color::White => 7,
            Color::LightBlack => 8,
            Color::LightRed => 9,
            Color::LightGreen => 10,
            Color::LightYellow => 11,
            Color::LightBlue => 12,
            Color::LightMagenta => 13,
            Color::LightCyan => 14,
            Color::LightWhite => 15,
        }
    }
}

pub trait TUIControl {
    fn position_cursor<W: Write>(&self, buf: &mut W, col: u16) -> Result;
    fn fg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result;
    fn bg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result;
    fn bold<W: Write>(&self, buf: &mut W) -> Result;
    fn reset_style<W: Write>(&self, buf: &mut W) -> Result;

    fn maybe_fg_color<W: Write>(&self, buf: &mut W, color: Option<Color>) -> Result {
        if let Some(c) = color {
            self.fg_color(buf, c)?;
        }
        Ok(())
    }

    fn maybe_bg_color<W: Write>(&self, buf: &mut W, color: Option<Color>) -> Result {
        if let Some(c) = color {
            self.bg_color(buf, c)?;
        }
        Ok(())
    }
}

#[derive(Default)]
pub struct ColorControl {}

impl TUIControl for ColorControl {
    fn position_cursor<W: Write>(&self, buf: &mut W, col: u16) -> Result {
        write!(buf, "\x1b[{}G", col)
    }

    fn fg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result {
        write!(buf, "\x1b[38;5;{}m", color.id())
    }

    fn bg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result {
        write!(buf, "\x1b[48;5;{}m", color.id())
    }

    fn bold<W: Write>(&self, buf: &mut W) -> Result {
        write!(buf, "\x1b[1m")
    }

    fn reset_style<W: Write>(&self, buf: &mut W) -> Result {
        write!(buf, "\x1b[0m")
    }
}

#[cfg(test)]
pub mod test {
    use super::*;

    #[derive(Default)]
    pub struct EmptyControl {}

    impl TUIControl for EmptyControl {
        fn position_cursor<W: Write>(&self, _buf: &mut W, _col: u16) -> Result {
            Ok(())
        }

        fn fg_color<W: Write>(&self, _buf: &mut W, _color: Color) -> Result {
            Ok(())
        }

        fn bg_color<W: Write>(&self, _buf: &mut W, _color: Color) -> Result {
            Ok(())
        }

        fn bold<W: Write>(&self, _buf: &mut W) -> Result {
            Ok(())
        }

        fn reset_style<W: Write>(&self, _buf: &mut W) -> Result {
            Ok(())
        }
    }

    pub struct VisibleEscapes {
        pub position: bool,
        pub style: bool,
    }

    impl VisibleEscapes {
        pub fn position_only() -> Self {
            VisibleEscapes {
                position: true,
                style: false,
            }
        }

        pub fn style_only() -> Self {
            VisibleEscapes {
                position: false,
                style: true,
            }
        }
    }

    impl Default for VisibleEscapes {
        fn default() -> Self {
            VisibleEscapes {
                position: true,
                style: true,
            }
        }
    }

    impl TUIControl for VisibleEscapes {
        fn position_cursor<W: Write>(&self, buf: &mut W, col: u16) -> Result {
            if self.position {
                write!(buf, "_C({})_", col)
            } else {
                Ok(())
            }
        }

        fn fg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result {
            if self.style {
                write!(buf, "_FG({:?})_", color)
            } else {
                Ok(())
            }
        }

        fn bg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result {
            if self.style {
                write!(buf, "_BG({:?})_", color)
            } else {
                Ok(())
            }
        }

        fn bold<W: Write>(&self, buf: &mut W) -> Result {
            if self.style {
                write!(buf, "_B_")
            } else {
                Ok(())
            }
        }

        fn reset_style<W: Write>(&self, buf: &mut W) -> Result {
            if self.style {
                write!(buf, "_R_")
            } else {
                Ok(())
            }
        }
    }
}
