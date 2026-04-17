use std::borrow::Cow;
use std::default::Default;
use std::num::NonZeroUsize;
use std::ops::Range;
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
    /// A span of spaces
    Spaces(usize),
    /// A specific range of bytes from the internal representation of the document
    /// (as returned by `Document::raw_bytes_for_searching`).
    SourceRange(Range<usize>),
    /// A portion of a generic string.
    String((Rc<String>, Range<usize>)),
    /// A static string.
    Static(&'static str),
}

const LOTS_OF_SPACES: [u8; 1024] = [b' '; 1024];

impl Text {
    pub fn len(&self) -> usize {
        match self {
            Text::Spaces(count) => *count,
            Text::SourceRange(range) => range.len(),
            Text::String((_, range)) => range.len(),
            Text::Static(s) => s.len(),
        }
    }

    pub fn bytes<'a>(&'a self, source: &'a [u8]) -> Cow<'a, [u8]> {
        match self {
            Text::Spaces(count) => {
                if *count < LOTS_OF_SPACES.len() {
                    Cow::Borrowed(&LOTS_OF_SPACES[0..*count])
                } else {
                    Cow::Owned(vec![b' '; *count])
                }
            }
            Text::SourceRange(range) => Cow::Borrowed(&source[range.clone()]),
            Text::String((s, range)) => Cow::Borrowed(&s.as_bytes()[range.clone()]),
            Text::Static(s) => Cow::Borrowed(s.as_bytes()),
        }
    }
}

/// A segment of text, along with some other data:
/// - the kind of segment this is, used for applying appropriate styling later
/// - how wide the segment is when printed to a termina
/// - a reference back into the original document
#[derive(Clone, Debug)]
pub struct Segment<DocRef, Kind> {
    pub content: Text,
    pub kind: Kind,
    pub terminal_width: usize,
    pub doc_ref: Option<DocRef>,
}

pub struct Compositor<'a, DocRef, Kind> {
    doc_content: &'a [u8],
    doc_width: NonZeroUsize,
    lines: Vec<Vec<Segment<DocRef, Kind>>>,
    remaining_space_on_current_line: usize,
}

impl<'a, DocRef: Copy, Kind: Copy> Compositor<'a, DocRef, Kind> {
    pub fn new(doc_content: &'a [u8], doc_width: NonZeroUsize) -> Self {
        Compositor {
            doc_content,
            doc_width,
            lines: vec![vec![]],
            remaining_space_on_current_line: doc_width.get(),
        }
    }

    pub fn finish(self) -> Vec<Vec<Segment<DocRef, Kind>>> {
        self.lines
    }

    fn add_entire_segment_to_current_line(&mut self, segment: Segment<DocRef, Kind>) {
        debug_assert!(self.remaining_space_on_current_line >= segment.terminal_width);

        self.remaining_space_on_current_line -= segment.terminal_width;
        self.lines.last_mut().unwrap().push(segment);
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

    fn take_prefix_that_fits_in_available_space(
        s: &str,
        mut available_space: usize,
    ) -> (&str, usize) {
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

    pub fn append_spaces(&mut self, n: usize, kind: Kind, doc_ref: Option<DocRef>) {
        let mut content_remaining = n;

        while content_remaining > 0 {
            self.maybe_start_new_line();

            if content_remaining > self.remaining_space_on_current_line {
                content_remaining -= self.remaining_space_on_current_line;

                self.add_entire_segment_to_current_line(Segment {
                    content: Text::Spaces(self.remaining_space_on_current_line),
                    kind,
                    terminal_width: self.remaining_space_on_current_line,
                    doc_ref,
                });
            } else {
                self.add_entire_segment_to_current_line(Segment {
                    content: Text::Spaces(content_remaining),
                    kind,
                    terminal_width: content_remaining,
                    doc_ref,
                });

                break;
            }
        }
    }

    pub fn append_content(&mut self, content: Text, kind: Kind, doc_ref: Option<DocRef>) {
        // Handle spaces separately, since it's simpler.
        if let Text::Spaces(n) = content {
            self.append_spaces(n, kind, doc_ref);
            return;
        };

        let mut processed_bytes = 0;

        while processed_bytes < content.len() {
            let remaining_s = match &content {
                Text::Spaces(_) => unreachable!(),
                Text::SourceRange(range) => {
                    str::from_utf8(&self.doc_content[(range.start + processed_bytes)..range.end])
                        .expect("doc_content should be valid utf8")
                }
                Text::String((s, range)) => &s[(range.start + processed_bytes)..range.end],
                Text::Static(s) => &s[processed_bytes..],
            };

            self.maybe_start_new_line();

            let (portion, used_width) = Self::take_prefix_that_fits_in_available_space(
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
                    self.add_entire_segment_to_current_line(Segment {
                        content: Text::Static("…"),
                        kind,
                        terminal_width: 1,
                        doc_ref,
                    });

                    let next_grapheme_len = remaining_s.graphemes(true).next().unwrap().len();
                    processed_bytes += next_grapheme_len;

                    continue;
                }
            }

            let content_portion = match &content {
                Text::Spaces(_) => unreachable!(),
                Text::SourceRange(range) => {
                    let portion_range_start = range.start + processed_bytes;
                    let portion_range_end = portion_range_start + portion.len();
                    Text::SourceRange(portion_range_start..portion_range_end)
                }
                Text::String((s, range)) => {
                    let portion_range_start = range.start + processed_bytes;
                    let portion_range_end = portion_range_start + portion.len();
                    Text::String((s.clone(), portion_range_start..portion_range_end))
                }
                Text::Static(s) => {
                    let portion_range_start = processed_bytes;
                    let portion_range_end = portion_range_start + portion.len();
                    Text::Static(&s[portion_range_start..portion_range_end])
                }
            };

            self.add_entire_segment_to_current_line(Segment {
                content: content_portion,
                kind,
                terminal_width: used_width,
                doc_ref,
            });

            processed_bytes += portion.len();
        }
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
            Text::Spaces(_) | Text::String(_) | Text::Static(_) => {
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

            let segment_text = format!("{}", segment.content.bytes(content).as_bstr());
            let segment_width = UnicodeWidthStr::width(segment_text.as_str());
            let segment_key = char::from_digit(i as u32, 36).unwrap();
            let segment_kind = match segment.content {
                Text::Spaces(_) => "spaces".to_string(),
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
                        content: Text::Spaces(5),
                    },
                    PreHighlightingStyledSegment {
                        attrs: color_token.normal,
                        search_match_attrs: default,
                        content: Text::SourceRange(6..11),
                    },
                    PreHighlightingStyledSegment {
                        attrs: normal_token.focused,
                        search_match_attrs: default,
                        content: Text::Spaces(1),
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
            1: default (focused)         : spaces
            2: color                     : range(6..11)
            3: default (focused)         : spaces
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
