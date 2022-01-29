use std::fmt;
use std::fmt::Write;
use std::iter::Peekable;
use std::ops::Range;

use crate::search::MatchRangeIter;
use crate::truncatedstrview::TruncatedStrView;
use crate::tuicontrol::{Color, TUIControl};

// This module is responsible for highlighting text in the
// appropriate colors when we print it out.
//
// We use different colors for different JSON value types,
// as well as different shades of gray.
//
// We searching for text, we highlight matches in yellow.
// In certain cases, when highlighted matches are also focused,
// we invert the normal colors of the terminal (to handle
// both light and dark color schemes).
//
//
// These are all the different things that we print out that
// may require special formatting:
//
// - Literal Values:
//   - null
//   - boolean
//   - number
//   - string
//   - empty objects and arrays
// - Object Keys
// - The ": " between an object key and its value
// - Array indexes in data mode (e.g., "[123]")
// - The ": " between an array index and the value
//   - Note that unlike the ": " between object keys and
//     values, these do not actually appear in the source
//     JSON and cannot be part of a search match
// - Commas after object and array elements
// - Open and close braces and brackets ("{}[]")
// - Container previews

#[derive(Copy, Clone)]
pub struct PrintStyle {
    pub fg: Color,
    pub bg: Color,
    pub inverted: bool,
    pub bold: bool,
}

impl PrintStyle {
    pub const fn default() -> Self {
        PrintStyle {
            fg: Color::Default,
            bg: Color::Default,
            inverted: false,
            bold: false,
        }
    }
}

impl Default for PrintStyle {
    fn default() -> Self {
        PrintStyle::default()
    }
}

//      Thing      |  Default Style  |  Focused Style  |      Match     |  Focused/Current Match
// ----------------+-----------------+-----------------+----------------+------------------------
//      null       |      Gray       |        X        | Yellow/Default |        Inverted
//     boolean     |     Yellow      |        X        | Yellow/Default |        Inverted
//     number      |     Magenta     |        X        | Yellow/Default |        Inverted
//     string      |      Green      |        X        | Yellow/Default |        Inverted
//  empty obj/arr  |     Default     |        X        | Yellow/Default |        Inverted
//
//  ^ Object values can't be focused
//
//   ": " and ","  |     Default     |     Default     | Yellow/Default |        Inverted
//
//  Object Labels  |      Blue       |  Inverted/Blue  | Yellow/Default |        Inverted
//                                        + Bold
//
//   Array Labels  |      Gray       | Default + Bold  |       X        |            X
//
//    Container    |     Default     |      Bold       | Yellow/Default |     Inverted + Bold
//    Delimiters
//
//    Container    |      Gray       |     Default     | Inverted Gray  |        Inverted
//     Previews

pub const DEFAULT_STYLE: PrintStyle = PrintStyle::default();

pub const BOLD_STYLE: PrintStyle = PrintStyle {
    bold: true,
    ..PrintStyle::default()
};

pub const INVERTED_STYLE: PrintStyle = PrintStyle {
    inverted: true,
    ..PrintStyle::default()
};

pub const BOLD_INVERTED_STYLE: PrintStyle = PrintStyle {
    inverted: true,
    bold: true,
    ..PrintStyle::default()
};

pub const GRAY_INVERTED_STYLE: PrintStyle = PrintStyle {
    fg: Color::LightBlack,
    inverted: true,
    ..PrintStyle::default()
};

pub const SEARCH_MATCH_HIGHLIGHTED: PrintStyle = PrintStyle {
    fg: Color::Yellow,
    inverted: true,
    ..PrintStyle::default()
};

pub const GRAY_STYLE: PrintStyle = PrintStyle {
    fg: Color::LightBlack,
    ..PrintStyle::default()
};

pub const YELLOW_STYLE: PrintStyle = PrintStyle {
    fg: Color::LightYellow,
    ..PrintStyle::default()
};

pub const MAGENTA_STYLE: PrintStyle = PrintStyle {
    fg: Color::LightMagenta,
    ..PrintStyle::default()
};

pub const GREEN_STYLE: PrintStyle = PrintStyle {
    fg: Color::LightGreen,
    ..PrintStyle::default()
};

pub const BLUE_STYLE: PrintStyle = PrintStyle {
    fg: Color::LightBlue,
    ..PrintStyle::default()
};

pub const INVERTED_BOLD_BLUE_STYLE: PrintStyle = PrintStyle {
    bg: Color::Blue,
    inverted: true,
    bold: true,
    ..PrintStyle::default()
};

// Helper struct for managing color state and not printing
// out so many escapes to make testing easier.
pub struct ColorPrinter<'a, TUI: TUIControl, W: Write> {
    tui: TUI,
    pub buf: &'a mut W,
    style: PrintStyle,
}

impl<'a, TUI: TUIControl, W: Write> ColorPrinter<'a, TUI, W> {
    pub fn new(tui: TUI, buf: &'a mut W) -> Self {
        ColorPrinter {
            tui,
            buf,
            style: PrintStyle::default(),
        }
    }

    pub fn set_style(&mut self, style: &PrintStyle) -> fmt::Result {
        if self.style.fg != style.fg {
            self.tui.fg_color(self.buf, style.fg)?;
        }

        if self.style.bg != style.bg {
            self.tui.bg_color(self.buf, style.bg)?;
        }

        if self.style.inverted != style.inverted {
            self.tui.set_inverted(self.buf, style.inverted)?;
        }

        if self.style.bold != style.bold {
            self.tui.set_bold(self.buf, style.inverted)?;
        }

        self.style = style.clone();

        Ok(())
    }
}

pub fn highlight_truncated_str_view<'a, W: Write, TUI: TUIControl>(
    out: &mut ColorPrinter<TUI, W>,
    mut s: &str,
    str_view: &TruncatedStrView,
    mut str_range_start: Option<usize>,
    style: &PrintStyle,
    highlight_style: &PrintStyle,
    matches_iter: &mut Option<&mut Peekable<MatchRangeIter<'a>>>,
) -> fmt::Result {
    let mut leading_ellipsis = false;
    let mut replacement_character = false;
    let mut trailing_ellipsis = false;

    if let Some(tr) = str_view.range {
        leading_ellipsis = tr.print_leading_ellipsis();
        replacement_character = tr.showing_replacement_character;
        trailing_ellipsis = tr.print_trailing_ellipsis(s);
        s = &s[tr.start..tr.end];
        str_range_start = str_range_start.map(|start| start + tr.start);
    }

    if leading_ellipsis {
        out.set_style(&GRAY_STYLE)?;
        out.buf.write_char('…')?;
    }

    // Print replacement character
    if replacement_character {
        out.set_style(style)?;
        // TODO: Technically we should figure out whether this
        // character's range should be highlighted, but also
        // maybe not bad to not highlight the replacement character;
        out.buf.write_char('�')?;
    }

    // Print actual string itself
    highlight_matches(
        out,
        s,
        str_range_start,
        style,
        highlight_style,
        matches_iter,
    )?;

    // Print trailing ellipsis
    if trailing_ellipsis {
        out.set_style(&GRAY_STYLE)?;
        out.buf.write_char('…')?;
    }

    Ok(())
}

pub fn highlight_matches<'a, W: Write, TUI: TUIControl>(
    out: &mut ColorPrinter<TUI, W>,
    mut s: &str,
    str_range_start: Option<usize>,
    style: &PrintStyle,
    highlight_style: &PrintStyle,
    matches_iter: &mut Option<&mut Peekable<MatchRangeIter<'a>>>,
) -> fmt::Result {
    if str_range_start.is_none() {
        out.set_style(style)?;
        write!(out.buf, "{}", s)?;
        return Ok(());
    }

    let mut start_index = str_range_start.unwrap();

    while !s.is_empty() {
        // Initialize the next match to be a fake match past the end of the string.
        let string_end = start_index + s.len();
        let mut match_start = string_end;
        let mut match_end = string_end;

        // Get rid of matches before the string.
        while let Some(Range { start, end }) = matches_iter.as_mut().map(|i| i.peek()).flatten() {
            if start_index < *end {
                match_start = (*start).clamp(start_index, string_end);
                match_end = (*end).clamp(start_index, string_end);
                break;
            }
            matches_iter.as_mut().unwrap().next();
        }

        // Print out stuff before the start of the match, if there's any.
        if start_index < match_start {
            let print_end = match_start - start_index;
            out.set_style(style)?;
            write!(out.buf, "{}", &s[..print_end])?;
        }

        // Highlight the matching substring.
        if match_start < string_end {
            out.set_style(highlight_style)?;
            let print_start = match_start - start_index;
            let print_end = match_end - start_index;
            write!(out.buf, "{}", &s[print_start..print_end])?;
        }

        // Update start_index and s
        s = &s[(match_end - start_index)..];
        start_index = match_end;
    }

    Ok(())
}
