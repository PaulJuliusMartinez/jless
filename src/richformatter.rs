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

pub trait RichFormatter {
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

#[cfg(test)]
pub mod test {
    use super::*;

    pub struct NoFormatting {}

    impl Default for NoFormatting {
        fn default() -> Self {
            NoFormatting {}
        }
    }

    impl RichFormatter for NoFormatting {
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

    pub struct VisibleEscapes {}

    impl Default for VisibleEscapes {
        fn default() -> Self {
            VisibleEscapes {}
        }
    }

    impl RichFormatter for VisibleEscapes {
        fn position_cursor<W: Write>(&self, buf: &mut W, col: u16) -> Result {
            write!(buf, "_COL({})_", col)
        }

        fn fg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result {
            write!(buf, "_FG({:?})_", color)
        }

        fn bg_color<W: Write>(&self, buf: &mut W, color: Color) -> Result {
            write!(buf, "_BG({:?})_", color)
        }

        fn bold<W: Write>(&self, buf: &mut W) -> Result {
            write!(buf, "_BOLD_")
        }

        fn reset_style<W: Write>(&self, buf: &mut W) -> Result {
            write!(buf, "_RESET_")
        }
    }
}
