use std::rc::Rc;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::rendering::{AnsiColor, Attrs, Color, StyledSegment, Text};
use crate::search::{LastJump, SearchDirection};

// This is all an abomination.

pub struct StatusBarTopLine {
    pub path_to_cursor: Option<Rc<String>>,
    pub filepath: Option<Rc<String>>,
}

const SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME: usize = 3;

#[derive(Clone, Debug)]
pub struct CurrentSearchState {
    pub search_direction: SearchDirection,
    pub search_input: Rc<String>,
    pub last_jump: Option<LastJump>,
    pub num_matches: usize,
}

impl StatusBarTopLine {
    pub fn render(&self, width: usize) -> Vec<StyledSegment> {
        let mut space_available = width;
        let mut segments = vec![];

        let inverted_attrs = Attrs::default().invert();
        let truncated_attrs = Attrs {
            bg: Color::Ansi(AnsiColor::BrightBlack),
            ..inverted_attrs
        };

        if let Some(path_to_cursor) = &self.path_to_cursor {
            let path_to_cursor = Rc::clone(path_to_cursor);
            let path_to_cursor_len = path_to_cursor.len();
            let path_to_cursor_width = UnicodeWidthStr::width(path_to_cursor.as_str());

            if path_to_cursor_width <= width {
                segments.push(StyledSegment {
                    content: Text::full_string(path_to_cursor),
                    attrs: inverted_attrs,
                });
                space_available -= path_to_cursor_width;

                if space_available <= SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME {
                    return segments;
                }

                space_available -= SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME;
                segments.push(StyledSegment {
                    content: Text::spaces(SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME),
                    attrs: inverted_attrs,
                });
            } else {
                let mut space_used_by_suffix = 0;
                let mut bytes_in_suffix = 0;
                let mut graphemes = path_to_cursor.graphemes(true);

                while let Some(grapheme) = graphemes.next_back() {
                    let grapheme_width = UnicodeWidthStr::width(grapheme);
                    // If we exceed width - 1 (we need space for the ellipsis),
                    // then don't include this grapheme and stop.
                    if space_used_by_suffix + grapheme_width > width - 1 {
                        break;
                    }

                    space_used_by_suffix += grapheme_width;
                    bytes_in_suffix += grapheme.len();

                    // If we've now used exactly width - 1, then break. We don't want
                    // to include any zero-length graphemes, because those tend to
                    // affect the characters before them, but we're not showing those
                    // so it'll just cause problems.
                    if space_used_by_suffix == width - 1 {
                        break;
                    }
                }

                // If we can't fit anything, we won't show anything at all, rather than
                // showing a single ellipsis. We still need to print a space though so
                // that the style gets set before clearing to the end of the line.
                if space_used_by_suffix == 0 {
                    segments.push(StyledSegment {
                        content: Text::spaces(width),
                        attrs: inverted_attrs,
                    });
                    return segments;
                }

                // Push ellipsis, then the suffix, then return
                segments.push(StyledSegment {
                    content: Text::ellipsis(),
                    attrs: truncated_attrs,
                });

                let start = path_to_cursor_len - bytes_in_suffix;
                segments.push(StyledSegment {
                    content: Text::String((path_to_cursor, start..path_to_cursor_len)),
                    attrs: inverted_attrs,
                });

                return segments;
            }
        }

        assert!(space_available > 0);

        let input_source_content = match &self.filepath {
            Some(filepath) => Text::full_string(Rc::clone(&filepath)),
            None => Text::Static("STDIN"),
        };

        // It's either a [Text::String] or [Text::Static], so the bytes we pass in don't matter.
        let input_source = input_source_content.as_str("".as_bytes());

        let input_source_width = UnicodeWidthStr::width(input_source);

        if input_source_width <= space_available {
            let extra_padding = space_available - input_source_width;
            segments.push(StyledSegment {
                content: Text::spaces(extra_padding),
                attrs: inverted_attrs,
            });
            segments.push(StyledSegment {
                content: input_source_content,
                attrs: inverted_attrs,
            });
        } else {
            // Reserve space for an ellipsis.
            space_available -= 1;

            let mut bytes_in_prefix = 0;
            let mut graphemes = input_source.graphemes(true);

            while let Some(grapheme) = graphemes.next() {
                let grapheme_width = UnicodeWidthStr::width(grapheme);
                if grapheme_width > space_available {
                    break;
                }

                space_available -= grapheme_width;
                bytes_in_prefix += grapheme.len();
            }

            // We may not be able to fit any part of the input_source. If that's the case,
            // we won't put anything, rather than putting a ellipsis all by itself.
            if bytes_in_prefix > 0 {
                // If we end up not using all the space (because the next character is too wide),
                // push an extra space so the input_source is right aligned.
                if space_available > 0 {
                    segments.push(StyledSegment {
                        content: Text::spaces(space_available),
                        attrs: inverted_attrs,
                    });
                }

                // Push prefix, then the ellipsis.
                segments.push(StyledSegment {
                    content: input_source_content.into_sub_range(0..bytes_in_prefix),
                    attrs: inverted_attrs,
                });

                segments.push(StyledSegment {
                    content: Text::ellipsis(),
                    attrs: truncated_attrs,
                });
            }
        }

        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::rendering::test_helpers::format_styled_segments;
    use crate::rendering::StyledSegment;
    use crate::test_helpers::pad_right_with_spaces;

    use insta::assert_snapshot;

    fn with_borders(styled_segments: Vec<StyledSegment>, width: usize) -> String {
        let content = pad_right_with_spaces(
            &format_styled_segments(&styled_segments, b"".as_ref()),
            width,
        );
        format!("|{content}|")
    }

    #[test]
    fn test_status_bar_top_line() {
        fn check(path_to_cursor: Option<&str>, filepath: Option<&str>, width: usize) -> String {
            let path_to_cursor = path_to_cursor.map(|s| Rc::new(s.to_string()));
            let filepath = filepath.map(|s| Rc::new(s.to_string()));

            let top_line = StatusBarTopLine {
                path_to_cursor,
                filepath,
            };

            with_borders(top_line.render(width), width)
        }

        assert_snapshot!(check(None, None, 15), @"|          STDIN|");
        assert_snapshot!(check(None, None, 3),  @"|ST…|");

        assert_snapshot!(check(None, Some("12345"), 15), @"|          12345|");
        assert_snapshot!(check(None, Some("12345"), 3),              @"|12…|");
        assert_snapshot!(check(None, Some("ab🪼cd"), 4),            @"| ab…|");
        assert_snapshot!(check(None, Some("ab🪼cde"), 5),          @"|ab🪼…|");

        assert_snapshot!(check(Some("1🦀3"), Some("ab🪼cde"), 15),  @"|1🦀3    ab🪼cde|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 14),  @"|123🦀456   123|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 13),  @"|123🦀456   1…|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 12),  @"|123🦀456    |");
        assert_snapshot!(check(Some("123🦀456"), Some("1"), 12),    @"|123🦀456   1|");

        assert_snapshot!(check(Some("123🦀456"), Some("123"), 8),  @"|123🦀456|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 7),  @"|…3🦀456|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 6),  @"|…🦀456|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 5),  @"|…456 |");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 2),  @"|…6|");
        assert_snapshot!(check(Some("123🦀"), Some("123"), 2),     @"|  |");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 1),  @"| |");
        assert_snapshot!(check(Some("1"), Some("123"), 1),         @"|1|");
    }
}
