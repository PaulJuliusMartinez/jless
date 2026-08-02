use std::default::Default;
use std::num::NonZeroUsize;
use std::ops::{Range, RangeFrom};
use std::rc::Rc;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::sorted_ranges::SortedRanges;
use crate::terminal::Color as TerminalColor;

/// Basic attributes for styling text: foreground and background colors, and bold.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub struct Attrs {
    pub fg: Color,
    pub bg: Color,
    pub inverted: bool,
    pub bold: bool,
    pub dimmed: bool,
}

impl Default for Attrs {
    fn default() -> Self {
        Attrs::const_default()
    }
}

impl Attrs {
    pub const fn const_default() -> Self {
        Attrs {
            fg: Color::Default,
            bg: Color::Default,
            inverted: false,
            bold: false,
            dimmed: false,
        }
    }

    pub const fn new(fg: Color, bg: Color) -> Attrs {
        Attrs {
            fg,
            bg,
            inverted: false,
            bold: false,
            dimmed: false,
        }
    }

    pub const fn new_bold(fg: Color, bg: Color) -> Attrs {
        Attrs {
            fg,
            bg,
            inverted: false,
            bold: true,
            dimmed: false,
        }
    }

    /// Creates a style from a foreground color, leaving the background set to `Default`.
    pub const fn from_fg(fg: Color) -> Attrs {
        Self::new(fg, Color::Default)
    }

    /// Creates a style from an ANSI foreground color, leaving the background set to `Default`.
    pub const fn from_ansi_fg(fg: AnsiColor) -> Attrs {
        Self::new(Color::Ansi(fg), Color::Default)
    }

    /// Inverts the foreground and background colors.
    pub const fn invert(&self) -> Attrs {
        Attrs {
            inverted: !self.inverted,
            ..*self
        }
    }
}

/// The actual text to be displayed on the screen. The ranges indicate the position of
/// the text in the actual document, and is used to highlight search matches.
#[derive(Clone, Debug)]
pub enum Text {
    /// A specific range of bytes from the internal representation of the document
    /// (as returned by `Document::raw_bytes_for_searching`).
    SourceRange(Range<usize>),
    /// A portion of a generic string.
    String((Rc<String>, Range<usize>)),
    /// A static string.
    Static(&'static str),
}

static LOTS_OF_SPACES: &str = unsafe { std::str::from_utf8_unchecked(&[b' '; 1024]) };

impl Text {
    pub fn spaces(n: usize) -> Self {
        if n < LOTS_OF_SPACES.len() {
            Text::Static(&LOTS_OF_SPACES[..n])
        } else {
            let s = String::from_utf8(vec![b' '; n]).unwrap();
            Text::String((Rc::new(s), 0..n))
        }
    }

    pub const fn ellipsis() -> Self {
        Text::Static("…")
    }

    pub fn full_string(s: Rc<String>) -> Self {
        let len = s.len();
        Text::String((s, 0..len))
    }

    pub fn len(&self) -> usize {
        match self {
            Text::SourceRange(range) => range.len(),
            Text::String((_, range)) => range.len(),
            Text::Static(s) => s.len(),
        }
    }

    pub fn as_bytes<'a>(&'a self, source: &'a [u8]) -> &'a [u8] {
        match self {
            Text::SourceRange(range) => &source[range.clone()],
            Text::String((s, range)) => &s.as_bytes()[range.clone()],
            Text::Static(s) => s.as_bytes(),
        }
    }

    pub fn as_str<'a>(&'a self, source: &'a [u8]) -> &'a str {
        match self {
            Text::SourceRange(range) => {
                str::from_utf8(&source[range.clone()]).expect("doc_content should be valid utf8")
            }
            Text::String((s, range)) => &s[range.clone()],
            Text::Static(s) => s,
        }
    }

    pub fn into_sub_range(self, range: Range<usize>) -> Self {
        fn index_range(range1: Range<usize>, range2: Range<usize>) -> Range<usize> {
            let start = range1.start + range2.start;
            let end = usize::min(range1.start + range2.end, range1.end);
            start..end
        }

        match self {
            Text::SourceRange(source_range) => Text::SourceRange(index_range(source_range, range)),
            Text::String((s, string_range)) => Text::String((s, index_range(string_range, range))),
            Text::Static(s) => Text::Static(&s[range]),
        }
    }

    fn sub_range(&self, range: Range<usize>) -> Self {
        self.clone().into_sub_range(range)
    }

    fn sub_range_from(&self, range: RangeFrom<usize>) -> Self {
        self.sub_range(range.start..(self.len()))
    }
}

/// A fragment of text, with a specified length, and a optional reference back to the source that
/// produced it.
#[derive(Clone, Debug)]
pub struct Fragment<Source> {
    pub text: Text,
    pub width: usize,
    pub source: Source,
}

pub struct Compositor<'a, Source> {
    doc_content: &'a [u8],
    doc_width: NonZeroUsize,
    lines: Vec<Vec<Fragment<Source>>>,
    remaining_space_on_current_line: usize,
    reserved_space: Option<usize>,
}

impl<'a, Source: Copy> Compositor<'a, Source> {
    pub fn new(doc_content: &'a [u8], doc_width: NonZeroUsize) -> Self {
        Compositor {
            doc_content,
            doc_width,
            lines: vec![vec![]],
            remaining_space_on_current_line: doc_width.get(),
            reserved_space: None,
        }
    }

    pub fn finish(self) -> Vec<Vec<Fragment<Source>>> {
        self.lines
    }

    fn add_entire_fragment_to_current_line(&mut self, fragment: Fragment<Source>) {
        debug_assert!(self.remaining_space_on_current_line >= fragment.width);

        self.remaining_space_on_current_line -= fragment.width;
        self.lines.last_mut().unwrap().push(fragment);
    }

    fn start_new_line(&mut self) {
        self.lines.push(vec![]);
        self.remaining_space_on_current_line = self.doc_width.get();
    }

    fn maybe_start_new_line(&mut self) {
        if self.remaining_space_on_current_line == 0 {
            self.start_new_line();
        }
    }

    fn str_prefix_that_fits_in_available_space(s: &str, available_space: usize) -> (&str, usize) {
        let mut used_bytes = 0;
        let mut used_space = 0;

        for grapheme in s.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if used_space + grapheme_width > available_space {
                break;
            }
            used_bytes += grapheme.len();
            used_space += grapheme_width;
        }

        (&s[0..used_bytes], used_space)
    }

    pub fn append_spaces(&mut self, n: usize, source: Source) {
        let spaces = Text::spaces(n);
        self.append_text(spaces, source);
    }

    pub fn append_text(&mut self, text: Text, source: Source) {
        assert!(
            self.reserved_space.is_none(),
            "should not call after reserving space"
        );

        let mut processed_bytes = 0;
        let text_str = text.as_str(self.doc_content);

        while processed_bytes < text.len() {
            let remaining_s = &text_str[processed_bytes..];

            self.maybe_start_new_line();

            let (portion, used_width) = Self::str_prefix_that_fits_in_available_space(
                remaining_s,
                self.remaining_space_on_current_line,
            );

            if used_width == 0 {
                if self.remaining_space_on_current_line < self.doc_width.get() {
                    // Maybe if we start a new line we'll be able to fit in the next character.
                    self.start_new_line();
                    continue;
                } else {
                    // There's no way we'll be able to fit the next character in, so we'll
                    // put an ellipsis in instead.
                    self.add_entire_fragment_to_current_line(Fragment {
                        text: Text::ellipsis(),
                        width: 1,
                        source,
                    });

                    let next_grapheme_len = remaining_s.graphemes(true).next().unwrap().len();
                    processed_bytes += next_grapheme_len;

                    continue;
                }
            }

            let portion_range = processed_bytes..(processed_bytes + portion.len());

            self.add_entire_fragment_to_current_line(Fragment {
                text: text.sub_range(portion_range),
                width: used_width,
                source,
            });

            processed_bytes += portion.len();
        }
    }

    pub fn start_reserving_space(&mut self, space_to_reserve: usize) -> bool {
        assert!(self.reserved_space.is_none(), "already reserved space");

        if self.remaining_space_on_current_line >= space_to_reserve {
            self.reserved_space = Some(space_to_reserve);
            true
        } else {
            false
        }
    }

    pub fn reserve_more_space(&mut self, extra_space: usize) -> bool {
        let reserved_space = self.reserved_space.expect("should have reserved space");
        let unreserved_space = self.remaining_space_on_current_line - reserved_space;

        if unreserved_space < extra_space {
            false
        } else {
            self.reserved_space = Some(reserved_space + extra_space);
            true
        }
    }

    pub fn give_back_reserved_space(&mut self, space_to_reclaim: usize) {
        let reserved_space = self.reserved_space.expect("should have reserved space");

        assert!(
            space_to_reclaim <= reserved_space,
            "gave back too much space"
        );

        self.reserved_space = Some(reserved_space - space_to_reclaim);
    }

    fn min_space_needed_to_show_str(s: &str) -> usize {
        let mut displayed_width = 0;
        let mut displayed_bytes = 0;

        for grapheme in s.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);

            // Keep adding 0-width graphemes even once we've displayed something, to see
            // if we get to the end.
            if grapheme_width > 0 && displayed_width > 0 {
                break;
            }

            displayed_width += grapheme_width;
            displayed_bytes += grapheme.len();
        }

        // If we haven't shown everything, then at a minimum we need an ellipsis at the end.
        if displayed_bytes < s.len() {
            displayed_width += 1;
        }

        displayed_width
    }

    pub fn min_space_needed_to_show_actual_content(&self, text: &Text, delimited: bool) -> usize {
        if delimited {
            let text_str = text.as_str(self.doc_content);
            let inner_range = 1..(text_str.len() - 1);

            2 + Self::min_space_needed_to_show_str(&text_str[inner_range])
        } else {
            Self::min_space_needed_to_show_str(text.as_str(self.doc_content))
        }
    }

    fn take_prefix_that_fits_in_available_space_with_ellipsis(
        s: &str,
        available_space: usize,
    ) -> (&str, usize) {
        let available_space_without_ellipsis = available_space.saturating_sub(1);

        let (prefix, used_space) =
            Self::str_prefix_that_fits_in_available_space(s, available_space_without_ellipsis);

        // If the whole string fit, great.
        if prefix.len() == s.len() {
            return (prefix, used_space);
        }

        // Otherwise, check if the entire remainder fits in the remaining space when we don't
        // reserve space for the ellipsis.
        let remainder = &s[prefix.len()..];
        let actual_remaining_space = available_space - used_space;

        let (extra, extra_space) =
            Self::str_prefix_that_fits_in_available_space(remainder, actual_remaining_space);

        // If we fit the whole remainder, return the whole original string.
        if extra.len() == remainder.len() {
            return (s, used_space + extra_space);
        }

        // Otherwise, we'll have to put the ellipsis in, so return the first prefix.
        (prefix, used_space)
    }

    pub fn try_append_text(
        &mut self,
        text: Text,
        delimited: bool,
        source: Source,
        reserved_space_to_reclaim: usize,
    ) -> bool {
        let reserved_space = self.reserved_space.expect("should have reserved space");
        let remaining_free_space = self.remaining_space_on_current_line - reserved_space;
        let free_space_for_content = remaining_free_space + reserved_space_to_reclaim;

        if self.min_space_needed_to_show_actual_content(&text, delimited) > free_space_for_content {
            return false;
        }

        let (open_delimiter, text, close_delimiter, space_taken_by_delimiters) = if delimited {
            let len = text.len();
            let open_delimiter = text.sub_range(0..1);
            let close_delimiter = text.sub_range_from((len - 1)..);
            let text = text.into_sub_range(1..(len - 1));
            (Some(open_delimiter), text, Some(close_delimiter), 2)
        } else {
            (None, text, None, 0)
        };

        if let Some(open_delimiter) = open_delimiter {
            self.add_entire_fragment_to_current_line(Fragment {
                text: open_delimiter,
                width: 1,
                source,
            });
        }

        let text_len = text.len();

        let (prefix, used_space) = Self::take_prefix_that_fits_in_available_space_with_ellipsis(
            text.as_str(self.doc_content),
            free_space_for_content - space_taken_by_delimiters,
        );

        let prefix_len = prefix.len();
        let prefix = text.into_sub_range(0..prefix_len);

        self.add_entire_fragment_to_current_line(Fragment {
            text: prefix,
            width: used_space,
            source,
        });

        // We only fit part of the string, so now we have to add the ellipsis too.
        if prefix_len != text_len {
            self.add_entire_fragment_to_current_line(Fragment {
                text: Text::ellipsis(),
                width: 1,
                source,
            });
        }

        if let Some(close_delimiter) = close_delimiter {
            self.add_entire_fragment_to_current_line(Fragment {
                text: close_delimiter,
                width: 1,
                source,
            });
        }

        self.reserved_space = Some(reserved_space - reserved_space_to_reclaim);

        true
    }

    pub fn append_reserved_text(&mut self, text: Text, source: Source) {
        let reserved_space = self.reserved_space.expect("should have reserved space");

        let width = UnicodeWidthStr::width(text.as_str(self.doc_content));

        self.add_entire_fragment_to_current_line(Fragment {
            text,
            width,
            source,
        });

        self.reserved_space = Some(reserved_space - width);
    }
}

// Someday: Rather than having this intermediate PreHighlightingStyledSegment, could
// we just process both highlighting and which nodes are focused in a single pass?

/// A segment of styled text, with two separate styles depending on whether any
/// search matches are contained within the text. Highlighting search matches is
/// done as a post-processing step so that individual data formats don't need to
/// worry about it.
#[derive(Debug)]
pub struct PreHighlightingStyledSegment {
    pub attrs: Attrs,
    pub search_match_attrs: Attrs,
    pub content: Text,
}

/// A segment of styled text.
#[derive(Debug)]
pub struct StyledSegment {
    pub attrs: Attrs,
    pub content: Text,
}

fn separate_highlighted_and_unhighlighted_ranges(
    range: &Range<usize>,
    mut search_matches: &[Range<usize>],
) -> Vec<(Range<usize>, bool)> {
    let mut remaining_range = range.clone();
    let mut parts = vec![];

    while remaining_range.len() > 0 {
        let search_match_range =
            match search_matches.index_of_first_elem_overlapping(&remaining_range) {
                None => {
                    // The whole range is not highlighted.
                    parts.push((remaining_range, false));
                    break;
                }
                Some(index) => {
                    let range = search_matches[index].clone();
                    // Make sure we update search_matches so that if we have a 0 length
                    // match we skip past it.
                    search_matches = &search_matches[(index + 1)..];
                    range
                }
            };

        // Push an unhighlighted segment at the start.
        if remaining_range.start < search_match_range.start {
            let unhighlighted_range = remaining_range.start..search_match_range.start;
            parts.push((unhighlighted_range, false));

            remaining_range = search_match_range.start..remaining_range.end;
        }

        let highlighted_range_start = usize::max(remaining_range.start, search_match_range.start);
        let highlighted_range_end = usize::min(remaining_range.end, search_match_range.end);
        let highlighted_range = highlighted_range_start..highlighted_range_end;

        if highlighted_range.len() > 0 {
            parts.push((highlighted_range, true));
        }

        remaining_range = highlighted_range_end..remaining_range.end;
    }

    parts
}

impl PreHighlightingStyledSegment {
    pub fn highlight_search_matches(self, search_matches: &[Range<usize>]) -> Vec<StyledSegment> {
        let PreHighlightingStyledSegment {
            attrs,
            search_match_attrs,
            content,
        } = self;

        match &content {
            Text::String(_) | Text::Static(_) => {
                vec![StyledSegment { attrs, content }]
            }
            Text::SourceRange(range) => {
                separate_highlighted_and_unhighlighted_ranges(range, search_matches)
                    .into_iter()
                    .map(|(range, highlighted)| {
                        let attrs = if highlighted {
                            search_match_attrs
                        } else {
                            attrs
                        };
                        StyledSegment {
                            attrs,
                            content: Text::SourceRange(range),
                        }
                    })
                    .collect()
            }
        }
    }
}

/// Colors in a basic 16-color ANSI colorscheme.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum AnsiColor {
    Black,
    Red,
    Green,
    Yellow,
    Blue,
    Magenta,
    Cyan,
    White,
    BrightBlack,
    BrightRed,
    BrightGreen,
    BrightYellow,
    BrightBlue,
    BrightMagenta,
    BrightCyan,
    BrightWhite,
}

/// A color in the terminal.
#[derive(Copy, Clone, PartialEq, Eq, Hash, Debug)]
pub enum Color {
    /// Default color, specified by `ESC [ 39 m` or `ESC [ 49 m` for foreground or background,
    /// respectively
    Default,
    /// A color from a 16-color ANSI colorscheme.
    Ansi(AnsiColor),
    /// A color from a 256 color 8-bit colorscheme.
    /// Colors 0-15 match those in a 16-color ANSI colorscheme.
    /// Colors 16-231 come from a 6x6x6 color cube, given by `16 + (red * 36) + (green * 6) + blue`.
    /// Colors 232-255 form a grayscale range from black to white (leaving out pure black and pure white).
    Rgb256(u8),
    /// A color from a standard 24-bit colorscheme, with 8 bits for red, green and blue.
    Rgb { r: u8, g: u8, b: u8 },
}

impl Color {
    pub fn to_terminal_color(self) -> TerminalColor {
        match self {
            Color::Default => TerminalColor::Default,
            Color::Ansi(ansi_color) => TerminalColor::C16(ansi_color as u8),
            Color::Rgb256(_) | Color::Rgb { .. } => {
                panic!("Can't convert advanced colors to old terminal color module")
            }
        }
    }
}

/// Colors to use for a token depending on the context:
/// `normal`: standard style to use when rendering tokens of this type
/// `focused`: style to use when focused on a token of this type
/// `search_match`: style to use when (part of) this token matches a search input
/// `focused_search_match`: style to use when focused on a token of this type
/// and it matches a search input
#[derive(Copy, Clone, Debug)]
pub struct TokenColorScheme {
    pub normal: Attrs,
    pub focused: Attrs,
    pub search_match: Attrs,
    pub focused_search_match: Attrs,
}

#[cfg(test)]
pub mod test_helpers {
    use super::*;

    use std::collections::HashMap;
    use std::fmt::Write;

    use bstr::ByteSlice;
    use unicode_width::UnicodeWidthStr;

    fn print_composited_line<SR>(line: &Vec<Fragment<SR>>, content: &[u8]) -> String {
        let mut s = String::new();
        for fragment in line.iter() {
            s.push_str(fragment.text.as_str(content));
        }
        s
    }

    pub fn print_composited_lines<SR>(
        lines: &Vec<Vec<Fragment<SR>>>,
        doc_width: usize,
        content: &[u8],
    ) -> String {
        let mut s = String::new();
        for line in lines.iter() {
            let line = print_composited_line(line, content);
            let line_width = UnicodeWidthStr::width(line.as_str());
            let line = line.replace("\u{200b}", "<ZWSP>");
            let num_spaces = doc_width.saturating_sub(line_width);
            let _ = writeln!(s, "|{line}{:num_spaces$}|", "");
        }
        s
    }

    impl<'a, SR> Compositor<'a, SR> {
        pub fn print_composited_lines(&self) -> String {
            print_composited_lines(&self.lines, self.doc_width.get(), self.doc_content)
        }

        pub fn availability(&self) -> String {
            format!(
                "remaining: {}, reserved: {}",
                self.remaining_space_on_current_line,
                self.reserved_space
                    .as_ref()
                    .map(usize::to_string)
                    .unwrap_or("-".to_string()),
            )
        }
    }

    pub fn build_style_map(
        token_styles: Vec<(TokenColorScheme, &'static str)>,
    ) -> HashMap<Attrs, String> {
        let mut map = HashMap::new();

        for (token_style, style_name) in token_styles.into_iter() {
            let normal = format!("{style_name}");
            let focused = format!("{style_name} (focused)");

            if let Some(prev_name) = map.insert(token_style.normal, normal) {
                panic!(
                    "{style_name} conflicts with {prev_name} in style map; both have attrs {:?}",
                    token_style.normal,
                );
            }

            if let Some(prev_name) = map.insert(token_style.focused, focused) {
                panic!(
                    "{style_name} conflicts with {prev_name} in style map; both have attrs {:?}",
                    token_style.focused,
                );
            }
        }

        map
    }

    pub fn dump_segments(
        segments: Vec<PreHighlightingStyledSegment>,
        content: &[u8],
        style_map: &HashMap<Attrs, String>,
    ) -> String {
        let mut text_line = "text: ".to_string();
        let mut key_line = "      ".to_string();
        let mut segment_styles = vec![];

        for (i, segment) in segments.into_iter().enumerate() {
            let style_name = match style_map.get(&segment.attrs) {
                None => format!("??? ({:?})", segment.attrs),
                Some(style_name) => style_name.clone(),
            };

            let segment_text = format!("{}", segment.content.as_bytes(content).as_bstr());
            let segment_width = UnicodeWidthStr::width(segment_text.as_str());
            let segment_key = char::from_digit(i as u32, 36).unwrap();
            let segment_kind = match segment.content {
                Text::SourceRange(range) => format!("range({range:?})"),
                Text::String((_, range)) => format!("string({range:?})"),
                Text::Static(_) => "static".to_string(),
            };

            if segment_width == 0 {
                continue;
            }

            let _ = text_line.write_str(&segment_text);
            let _ = key_line.write_char(segment_key);
            for _ in 0..(segment_width - 1) {
                let _ = key_line.write_char('.');
            }

            segment_styles.push(format!("{segment_key}: {style_name:<25} : {segment_kind}"));
        }

        let segment_styles = segment_styles.join("\n");

        format!("{text_line}\n{key_line}\n{segment_styles}")
    }

    pub fn format_styled_segments(styled_segments: &[StyledSegment], content: &[u8]) -> String {
        let mut s = String::new();

        for styled_segment in styled_segments.iter() {
            s.push_str(styled_segment.content.as_str(content));
        }

        s
    }

    #[cfg(test)]
    mod tests {
        use super::super::*;
        use super::*;

        use insta::assert_snapshot;

        #[test]
        fn test_dump_segments() {
            let default = Attrs::default();
            let inverted = default.invert();
            let red = Attrs::from_ansi_fg(AnsiColor::Red);
            let blue = Attrs::from_ansi_fg(AnsiColor::Blue);

            let normal_token = TokenColorScheme {
                normal: default,
                focused: inverted,
                search_match: default,
                focused_search_match: default,
            };

            let color_token = TokenColorScheme {
                normal: red,
                focused: blue,
                search_match: default,
                focused_search_match: default,
            };

            let style_map =
                build_style_map(vec![(normal_token, "default"), (color_token, "color")]);

            let dumped = dump_segments(
                vec![
                    PreHighlightingStyledSegment {
                        attrs: normal_token.normal,
                        search_match_attrs: default,
                        content: Text::SourceRange(0..6),
                    },
                    PreHighlightingStyledSegment {
                        attrs: normal_token.focused,
                        search_match_attrs: default,
                        content: Text::spaces(5),
                    },
                    PreHighlightingStyledSegment {
                        attrs: color_token.normal,
                        search_match_attrs: default,
                        content: Text::SourceRange(6..11),
                    },
                    PreHighlightingStyledSegment {
                        attrs: normal_token.focused,
                        search_match_attrs: default,
                        content: Text::spaces(1),
                    },
                    PreHighlightingStyledSegment {
                        attrs: color_token.focused,
                        search_match_attrs: default,
                        content: Text::String((std::rc::Rc::new("🦀".to_string()), 0.."🦀".len())),
                    },
                ],
                b"hello,world",
                &style_map,
            );

            assert_snapshot!(dumped, @r"
            text: hello,     world 🦀
                  0.....1....2....34.
            0: default                   : range(0..6)
            1: default (focused)         : static
            2: color                     : range(6..11)
            3: default (focused)         : static
            4: color (focused)           : string(0..4)
            ");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::ops::Range;

    use insta::assert_snapshot;

    fn compositor(doc: &'static [u8], width: usize) -> Compositor<'static, ()> {
        Compositor::new(doc, NonZeroUsize::new(width).unwrap())
    }

    #[test]
    fn test_basic_compositor() {
        let mut c = compositor(b"abcdefghijklmnopqrstuvwxyz", 5);

        c.append_text(Text::SourceRange(0..3), ());
        c.append_text(Text::Static("111222333"), ());
        c.append_text(Text::SourceRange(23..26), ());
        c.append_text(Text::String((Rc::new(".---.".to_string()), 1..4)), ());

        assert_snapshot!(c.print_composited_lines(), @r"
        |abc11|
        |12223|
        |33xyz|
        |---  |
        ");
    }

    #[test]
    fn test_compositor_with_wide_chars() {
        let mut c = compositor(b"", 5);

        c.append_text(Text::Static("1🦀45a"), ());
        c.append_text(Text::Static("bcd👀34"), ());
        c.append_text(Text::Static("5\u{200b}abc"), ());

        // Eyes get pushed to next line because not enough room; ZWSP gets appened
        // to the current line because it has 0 width.
        assert_snapshot!(c.print_composited_lines(), @r"
        |1🦀45|
        |abcd |
        |👀345<ZWSP>|
        |abc  |
        ");
    }

    #[test]
    fn test_compositor_with_single_col() {
        let mut c = compositor(b"", 1);
        c.append_text(Text::Static("a🦀b"), ());

        // Wide characters get replaced with an ellipsis when there's only
        // a single column.
        assert_snapshot!(c.print_composited_lines(), @r"
        |a|
        |…|
        |b|
        ");
    }

    #[test]
    fn test_compositor_min_size_to_show_strings() {
        let f = Compositor::<()>::min_space_needed_to_show_str;
        assert_eq!(f(""), 0);
        assert_eq!(f("a"), 1);
        assert_eq!(f("abc"), 2);
        assert_eq!(f("🦀"), 2);
        assert_eq!(f("🦀abc"), 3);
        assert_eq!(f("\u{200b}abc"), 2);
    }

    #[test]
    fn test_compositor_reservations() {
        let mut c = compositor(b"", 11);
        c.append_text(Text::Static("1"), ());
        assert!(!c.start_reserving_space(11));
        assert!(c.start_reserving_space(5));

        assert!(!c.reserve_more_space(6));
        assert!(c.reserve_more_space(5));
        c.give_back_reserved_space(1);

        assert!(!c.try_append_text(Text::Static("abc"), false, (), 0));
        c.give_back_reserved_space(1);
        assert!(c.try_append_text(Text::Static("abc"), false, (), 0));

        assert_snapshot!(c.print_composited_lines(), @"|1a…        |");
        assert_snapshot!(c.availability(), @"remaining: 8, reserved: 8");

        c.give_back_reserved_space(1);
        assert!(!c.try_append_text(Text::Static("🦀"), false, (), 0));
        // With the one free space, and the one reclaimed space, now it can print it.
        assert!(c.try_append_text(Text::Static("🦀"), false, (), 1));

        assert_snapshot!(c.print_composited_lines(), @"|1a…🦀      |");
        assert_snapshot!(c.availability(), @"remaining: 6, reserved: 6");

        c.give_back_reserved_space(2);
        assert_snapshot!(c.availability(), @"remaining: 6, reserved: 4");

        assert!(c.try_append_text(Text::Static("ab"), false, (), 1));

        assert_snapshot!(c.print_composited_lines(), @"|1a…🦀ab    |");
        assert_snapshot!(c.availability(), @"remaining: 4, reserved: 3");

        c.append_reserved_text(Text::Static("x"), ());

        assert_snapshot!(c.print_composited_lines(), @"|1a…🦀abx   |");
        assert_snapshot!(c.availability(), @"remaining: 3, reserved: 2");
    }

    #[test]
    fn test_compositor_delimited_content() {
        let mut c = compositor(b"", 10);
        assert!(c.start_reserving_space(7));
        assert!(!c.try_append_text(Text::Static("'123'"), true, (), 0));

        c.give_back_reserved_space(1);
        assert!(c.try_append_text(Text::Static("'123'"), true, (), 0));

        assert_snapshot!(c.print_composited_lines(), @"|'1…'      |");
        assert_snapshot!(c.availability(), @"remaining: 6, reserved: 6");

        assert!(c.try_append_text(Text::Static("()"), true, (), 2));

        assert_snapshot!(c.print_composited_lines(), @"|'1…'()    |");
        assert_snapshot!(c.availability(), @"remaining: 4, reserved: 4");

        c.give_back_reserved_space(2);
        assert!(!c.try_append_text(Text::Static("[🦀]"), true, (), 1));
        assert!(c.try_append_text(Text::Static("[🦀]"), true, (), 2));

        assert_snapshot!(c.print_composited_lines(), @"|'1…'()[🦀]|");
        assert_snapshot!(c.availability(), @"remaining: 0, reserved: 0");
    }

    #[test]
    fn test_separate_highlighted_and_unhighlighted_ranges() {
        fn f(r: &Range<usize>, matches: &[Range<usize>]) -> String {
            separate_highlighted_and_unhighlighted_ranges(r, matches)
                .into_iter()
                .map(|(r, b)| {
                    format!(
                        "{}: {:?}",
                        if b { "  highlighted" } else { "unhighlighted" },
                        r,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        }

        let search_matches = vec![10..20, 30..40, 40..40, 40..40, 50..50, 50..60, 70..70];

        assert_snapshot!(f(&(0..35), &search_matches), @r"
        unhighlighted: 0..10
          highlighted: 10..20
        unhighlighted: 20..30
          highlighted: 30..35
        ");

        assert_snapshot!(f(&(30..40), &search_matches), @"  highlighted: 30..40");

        assert_snapshot!(f(&(30..45), &search_matches), @r"
          highlighted: 30..40
        unhighlighted: 40..45
        ");

        assert_snapshot!(f(&(35..45), &search_matches), @r"
          highlighted: 35..40
        unhighlighted: 40..45
        ");

        assert_snapshot!(f(&(40..45), &search_matches), @"unhighlighted: 40..45");

        assert_snapshot!(f(&(40..55), &search_matches), @r"
        unhighlighted: 40..50
          highlighted: 50..55
        ");

        assert_snapshot!(f(&(50..65), &search_matches), @r"
          highlighted: 50..60
        unhighlighted: 60..65
        ");

        assert_snapshot!(f(&(65..75), &search_matches), @r"
        unhighlighted: 65..70
        unhighlighted: 70..75
        ");

        assert_snapshot!(f(&(70..70), &search_matches), @"");
    }
}
