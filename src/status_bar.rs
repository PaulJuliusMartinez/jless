use std::rc::Rc;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::app::{MessageSeverity, MAX_BUFFER_SIZE};
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

pub struct StatusBarBottomLine {
    pub message: Option<(Rc<String>, MessageSeverity)>,
    pub search_state: Option<CurrentSearchState>,
    pub input_buffer: Option<Rc<String>>,
}

const SPACE_BEFORE_SEARCH_COUNT: usize = 3;
const SPACE_BEFORE_INPUT_BUFFER: usize = 3;
const SPACE_BETWEEN_INPUT_BUFFER_AND_EDGE_OF_SCREEN: usize = 1;

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
            Some(filepath) => Text::full_string(Rc::clone(filepath)),
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

            for grapheme in input_source.graphemes(true) {
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

impl StatusBarBottomLine {
    pub fn render(&self, width: usize) -> Vec<StyledSegment> {
        let mut segments = vec![];

        let space_for_input_buffer = usize::min(
            width,
            MAX_BUFFER_SIZE + SPACE_BETWEEN_INPUT_BUFFER_AND_EDGE_OF_SCREEN,
        );
        let available_space_before_input_buffer = width - space_for_input_buffer;
        let available_space_for_message_or_search_state =
            available_space_before_input_buffer.saturating_sub(SPACE_BEFORE_INPUT_BUFFER);
        let size_of_space_before_input_buffer =
            available_space_before_input_buffer - available_space_for_message_or_search_state;

        if let Some((message, message_severity)) = &self.message {
            Self::render_message(
                Rc::clone(message),
                *message_severity,
                &mut segments,
                available_space_for_message_or_search_state,
            );
        } else if let Some(search_state) = &self.search_state {
            Self::render_search_state(
                search_state,
                &mut segments,
                available_space_for_message_or_search_state,
            );
        } else {
            segments.push(StyledSegment {
                content: Text::spaces(available_space_for_message_or_search_state),
                attrs: Attrs::default(),
            });
        }

        segments.push(StyledSegment {
            content: Text::spaces(size_of_space_before_input_buffer),
            attrs: Attrs::default(),
        });

        if let Some(input_buffer) = &self.input_buffer {
            Self::render_input_buffer(
                Rc::clone(input_buffer),
                &mut segments,
                space_for_input_buffer,
            );
        }

        segments
    }

    fn render_message(
        message: Rc<String>,
        message_severity: MessageSeverity,
        segments: &mut Vec<StyledSegment>,
        width: usize,
    ) {
        let space_available_for_message = width.saturating_sub(SPACE_BEFORE_INPUT_BUFFER);
        let space_used = Self::render_left_aligned_truncated(
            message,
            message_severity.attrs(),
            segments,
            space_available_for_message,
        );

        let unused_space = width - space_used;

        segments.push(StyledSegment {
            content: Text::spaces(unused_space),
            attrs: Attrs::default(),
        });
    }

    fn render_search_state(
        search_state: &CurrentSearchState,
        segments: &mut Vec<StyledSegment>,
        width: usize,
    ) {
        let wrap_and_search_count = if let Some(last_jump) = &search_state.last_jump {
            let wrap_indicator = if last_jump.just_wrapped { "W" } else { " " };
            let match_num = last_jump.match_jumped_to + 1;
            let num_matches = search_state.num_matches;
            Some(format!("{wrap_indicator} [{match_num}/{num_matches}]"))
        } else {
            None
        };

        let space_for_search_prompt = if let Some(wrap_and_search_count) = &wrap_and_search_count {
            width
                .saturating_sub(wrap_and_search_count.len())
                .saturating_sub(SPACE_BEFORE_SEARCH_COUNT)
        } else {
            width
        };

        let mut used_for_search_prompt = 0;
        if space_for_search_prompt > 1 {
            segments.push(StyledSegment {
                content: Text::Static(search_state.search_direction.prompt_str()),
                attrs: Attrs::default(),
            });

            used_for_search_prompt += 1;
            let space_for_search_term = space_for_search_prompt - 1;
            let used_for_search_term = Self::render_left_aligned_truncated(
                Rc::clone(&search_state.search_input),
                Attrs::default(),
                segments,
                space_for_search_term,
            );
            used_for_search_prompt += used_for_search_term;

            if used_for_search_term == 0 && space_for_search_term > 0 {
                // Normally don't just put a plain ellipsis with no content,
                // but we will in this case since we're showing the prompt char.
                segments.push(StyledSegment {
                    content: Text::ellipsis(),
                    attrs: Attrs::default(),
                });
                used_for_search_prompt += 1;
            }
        }

        if let Some(wrap_and_search_count) = wrap_and_search_count {
            let space_for_search_prompt_and_space =
                width.saturating_sub(wrap_and_search_count.len());

            segments.push(StyledSegment {
                content: Text::spaces(space_for_search_prompt_and_space - used_for_search_prompt),
                attrs: Attrs::default(),
            });

            let available_space = width - space_for_search_prompt_and_space;
            let space_used_for_wrap_and_search_count = Self::render_left_aligned_truncated(
                Rc::new(wrap_and_search_count),
                Attrs::default(),
                segments,
                available_space,
            );

            if available_space > 0 {
                segments.push(StyledSegment {
                    content: Text::spaces(available_space - space_used_for_wrap_and_search_count),
                    attrs: Attrs::default(),
                });
            }
        } else {
            segments.push(StyledSegment {
                content: Text::spaces(space_for_search_prompt - used_for_search_prompt),
                attrs: Attrs::default(),
            });
        }
    }

    fn render_input_buffer(
        input_buffer: Rc<String>,
        segments: &mut Vec<StyledSegment>,
        width: usize,
    ) {
        // This is the last thing, so we don't need to do any padding afterwards.
        let _ =
            Self::render_left_aligned_truncated(input_buffer, Attrs::default(), segments, width);
    }

    fn render_left_aligned_truncated(
        s: Rc<String>,
        attrs: Attrs,
        segments: &mut Vec<StyledSegment>,
        width: usize,
    ) -> usize {
        let str_width = UnicodeWidthStr::width(s.as_str());

        if str_width <= width {
            segments.push(StyledSegment {
                content: Text::full_string(s),
                attrs,
            });
            return str_width;
        }

        // Don't just show an ellipsis.
        if width <= 1 {
            return 0;
        }

        let mut space_used = 0;
        let mut bytes_used = 0;

        for grapheme in s.as_str().graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);

            if space_used + grapheme_width > width - 1 {
                break;
            }

            space_used += grapheme_width;
            bytes_used += grapheme.len();
        }

        segments.push(StyledSegment {
            content: Text::String((s, 0..bytes_used)),
            attrs,
        });

        segments.push(StyledSegment {
            content: Text::ellipsis(),
            attrs,
        });

        space_used + 1
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

    #[test]
    fn test_status_bar_bottom_line() {
        fn check(
            message: Option<&str>,
            search_state: Option<CurrentSearchState>,
            input_buffer: Option<&str>,
            width: usize,
        ) -> String {
            let message = message.map(|s| (Rc::new(s.to_string()), MessageSeverity::Info));
            let input_buffer = input_buffer.map(|s| Rc::new(s.to_string()));

            let bottom_line = StatusBarBottomLine {
                message,
                search_state,
                input_buffer,
            };

            with_borders(bottom_line.render(width), width)
        }

        assert_snapshot!(check(None, None, None, 15),                                     @"|               |");
        assert_snapshot!(check(None, None, Some(&"1"), 15),                               @"|     1         |");

        // Messages
        assert_snapshot!(check(Some("An error occurred"), None, Some(&"1"), 15),          @"|     1         |");
        assert_snapshot!(check(Some("An error occurred"), None, Some(&"1"), 16),         @"|      1         |");
        assert_snapshot!(check(Some("An error occurred"), None, Some(&"1"), 17),        @"|       1         |");
        assert_snapshot!(check(Some("An error occurred"), None, Some(&"1"), 18),       @"|A…      1         |");
        assert_snapshot!(check(Some("An error occurred"), None, Some(&"1"), 19),      @"|An…      1         |");
        assert_snapshot!(check(Some("An error occurred"), None, Some(&"1"), 20),     @"|An …      1         |");
        assert_snapshot!(check(Some("An error occurred"), None, Some(&"1"), 21),    @"|An e…      1         |");
        assert_snapshot!(check(Some("Bad ❌ happened"),   None, Some(&"1"), 22),   @"|Bad …       1         |");
        assert_snapshot!(check(Some("Bad ❌ happened"),   None, Some(&"1"), 23),  @"|Bad ❌…      1         |");

        // Search state
        let mut search_state = CurrentSearchState {
            search_direction: SearchDirection::Forward,
            search_input: Rc::new("abc".to_string()),
            last_jump: Some(LastJump {
                match_jumped_to: 5,
                just_wrapped: true,
            }),
            num_matches: 15,
        };

        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 28), @"|/abc   W [6/15]   1         |");
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 27),  @"|/a…   W [6/15]   1         |");
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 26),   @"|/…   W [6/15]   1         |");
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 25),    @"|    W [6/15]   1         |");

        search_state.search_direction = SearchDirection::Reverse;
        search_state.last_jump.as_mut().unwrap().just_wrapped = false;
        // Search term can't fill up the space left by the 'W'.
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 27),  @"|?a…     [6/15]   1         |");

        search_state.last_jump = None;
        search_state.search_input = Rc::new("abcdefghijklm".to_string());
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 27), @"|?abcdefghijklm   1         |");
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 26),  @"|?abcdefghijk…   1         |");

        search_state.search_input = Rc::new("ab🪼cd".to_string());
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 19), @"|?ab🪼…   1         |");
        assert_snapshot!(check(None, Some(search_state.clone()), Some(&"1"), 18),  @"|?ab…    1         |");
    }
}
