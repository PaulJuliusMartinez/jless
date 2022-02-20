use std::fmt;
use std::iter::Peekable;
use std::ops::Range;

use crate::search::MatchRangeIter;
use crate::terminal;
use crate::terminal::{Style, Terminal};
use crate::truncatedstrview::TruncatedStrView;

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

pub const DEFAULT_STYLE: Style = Style::default();

pub const BOLD_STYLE: Style = Style {
    bold: true,
    ..Style::default()
};

pub const BOLD_INVERTED_STYLE: Style = Style {
    inverted: true,
    bold: true,
    ..Style::default()
};

pub const GRAY_INVERTED_STYLE: Style = Style {
    fg: terminal::LIGHT_BLACK,
    inverted: true,
    ..Style::default()
};

pub const SEARCH_MATCH_HIGHLIGHTED: Style = Style {
    fg: terminal::YELLOW,
    inverted: true,
    ..Style::default()
};

pub const DIMMED_STYLE: Style = Style {
    dimmed: true,
    ..Style::default()
};

pub const PREVIEW_STYLES: (&Style, &Style) = (&DIMMED_STYLE, &GRAY_INVERTED_STYLE);

pub const BLUE_STYLE: Style = Style {
    fg: terminal::LIGHT_BLUE,
    ..Style::default()
};

pub const INVERTED_BOLD_BLUE_STYLE: Style = Style {
    bg: terminal::BLUE,
    inverted: true,
    bold: true,
    ..Style::default()
};

#[allow(clippy::too_many_arguments)]
pub fn highlight_truncated_str_view<'a>(
    out: &mut dyn Terminal,
    mut s: &str,
    str_view: &TruncatedStrView,
    mut str_range_start: Option<usize>,
    style: &Style,
    highlight_style: &Style,
    matches_iter: &mut Option<&mut Peekable<MatchRangeIter<'a>>>,
    focused_search_match: &Range<usize>,
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
        out.set_style(&DIMMED_STYLE)?;
        out.write_char('…')?;
    }

    // Print replacement character
    if replacement_character {
        out.set_style(style)?;
        // TODO: Technically we should figure out whether this
        // character's range should be highlighted, but also
        // maybe not bad to not highlight the replacement character;
        out.write_char('�')?;
    }

    // Print actual string itself
    highlight_matches(
        out,
        s,
        str_range_start,
        style,
        highlight_style,
        matches_iter,
        focused_search_match,
    )?;

    // Print trailing ellipsis
    if trailing_ellipsis {
        out.set_style(&DIMMED_STYLE)?;
        out.write_char('…')?;
    }

    Ok(())
}

pub fn highlight_matches<'a>(
    out: &mut dyn Terminal,
    mut s: &str,
    str_range_start: Option<usize>,
    style: &Style,
    highlight_style: &Style,
    matches_iter: &mut Option<&mut Peekable<MatchRangeIter<'a>>>,
    focused_search_match: &Range<usize>,
) -> fmt::Result {
    if str_range_start.is_none() {
        out.set_style(style)?;
        write!(out, "{}", s)?;
        return Ok(());
    }

    let mut start_index = str_range_start.unwrap();

    while !s.is_empty() {
        // Initialize the next match to be a fake match past the end of the string.
        let string_end = start_index + s.len();
        let mut match_start = string_end;
        let mut match_end = string_end;
        let mut match_is_focused_match = false;

        // Get rid of matches before the string.
        while let Some(range) = matches_iter.as_mut().map(|i| i.peek()).flatten() {
            if start_index < range.end {
                if *range == focused_search_match {
                    match_is_focused_match = true;
                }

                match_start = range.start.clamp(start_index, string_end);
                match_end = range.end.clamp(start_index, string_end);
                break;
            }
            matches_iter.as_mut().unwrap().next();
        }

        // Print out stuff before the start of the match, if there's any.
        if start_index < match_start {
            let print_end = match_start - start_index;
            out.set_style(style)?;
            write!(out, "{}", &s[..print_end])?;
        }

        // Highlight the matching substring.
        if match_start < string_end {
            if match_is_focused_match {
                out.set_style(&BOLD_INVERTED_STYLE)?;
            } else {
                out.set_style(highlight_style)?;
            }
            let print_start = match_start - start_index;
            let print_end = match_end - start_index;
            write!(out, "{}", &s[print_start..print_end])?;
        }

        // Update start_index and s
        s = &s[(match_end - start_index)..];
        start_index = match_end;
    }

    Ok(())
}
