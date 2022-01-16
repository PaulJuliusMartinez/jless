use std::fmt;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// Scrolling long strings:
// - ScreenWriter needs to keep track of scrolling
//   - Map<LineNumber, HorizontalScrollState>
//   - Only keep track for lines that were printed last time
// - Need imperative function to update scroll state
//   - .scroll_right() (.) / .scroll_left() (,) / .jump_to_end() (;)
//   - Need to pass in viewer
// - Need to update HorizontalScrollStates when dimensions change
//   - .update_dimension()
// - HorizontalScrollState will also change when jumping to a search match
//   - .focus_search_match()

// HorizontalScrollState (enum)
//
// - Able to fit entire value
// - Value not printed at all (key already overflows)
// - Scrolled (start..end)
//   - Represents range that gets printed
//
// - scroll_right
//   - eats one grapheme from start, unless end == str.end, then pushes end back
//     the same width that it consumed
//
//     if start is at 0, need to consume two characters, otherwise the ellipsis
//     just replaces the first character (what if first character is wide?)
// - scroll_left
//   - backs up end one grapheme, unless start == 0, then pushes start back
//     same width it consumed
//
// Note that the current range may be leave space if next character is wide:
//   Before: [1][2][1]... ([2]...), room for 5
//   After should be: [2][1][2]... (...)
//
//
// - jump_to_end
//   - if end == end
//       start == 0
//     else
//       end == end
//
// Nothing happens unless state == scrolled
//
const REPLACMENT_CHARACTER: &'static str = "�";
const ELLIPSIS: &'static str = "…";

#[derive(Clone, Debug)]
pub enum HorizontalScrollState {
    // Value isn't displayed at all
    OffScreen,
    // Entire value is displayed
    Fits,
    // Only room for one character, so entire value is ellided; doesn't support scrolling
    Ellided,
    // A prefix of the value is shown
    Prefix {
        end: usize,
        replacement: bool,
    },
    // A suffix of the value is shown
    Suffix {
        start: usize,
        replacement: bool,
    },
    // A middle portion of the value is shown
    Middle {
        range: Range<usize>,
        replacement: bool,
    },
}

use HorizontalScrollState::*;

#[derive(Debug)]
pub struct ScrolledString<'a> {
    string: &'a str,
    scroll_state: &'a HorizontalScrollState,
}

impl<'a> fmt::Display for ScrolledString<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.scroll_state {
            Fits => f.write_str(self.string),
            OffScreen => Ok(()),
            Ellided => f.write_str(ELLIPSIS),
            Prefix { end, replacement } => {
                f.write_str(&self.string[..*end])?;
                if *replacement {
                    f.write_str(REPLACMENT_CHARACTER)?;
                }
                f.write_str(ELLIPSIS)
            }
            Suffix { start, replacement } => {
                f.write_str(ELLIPSIS)?;
                if *replacement {
                    f.write_str(REPLACMENT_CHARACTER)?;
                }
                f.write_str(&self.string[*start..])
            }
            Middle { range, replacement } => {
                f.write_str(ELLIPSIS)?;
                if *replacement {
                    f.write_str(REPLACMENT_CHARACTER)?;
                }
                f.write_str(&self.string[range.start..range.end])?;
                f.write_str(ELLIPSIS)
            }
        }
    }
}

macro_rules! define_init {
    ($fn_name:ident, $iter_fn:ident, $make_prefix:expr) => {
        pub fn $fn_name(value: &str, available_space: isize) -> HorizontalScrollState {
            if available_space < 0 || (available_space == 0 && value.len() != 0) {
                return OffScreen;
            }

            // Optimization
            if value.len() as isize <= available_space {
                return Fits;
            }

            let mut used_space = 0;
            let mut offset = 0;

            let mut graphemes = value.graphemes(true);
            while let Some(grapheme) = graphemes.$iter_fn() {
                let grapheme_width = UnicodeWidthStr::width(grapheme) as isize;
                let mut space_with_grapheme = used_space + grapheme_width;
                if offset + grapheme.len() < value.len() {
                    space_with_grapheme += 1;
                }

                if space_with_grapheme <= available_space {
                    used_space += grapheme_width;
                    offset += grapheme.len();
                } else {
                    break;
                }
            }

            if offset == value.len() {
                Fits
            } else if available_space == 1 {
                // We still go through the above processing
                // in case the value has a width of one.
                Ellided
            } else {
                // If there's room for a character and an ellipsis (avilable_space >= 2),
                // then we'll show a replacement if we couldn't show anything.
                if $make_prefix {
                    Prefix {
                        end: offset,
                        replacement: offset == 0,
                    }
                } else {
                    Suffix {
                        start: value.len() - offset,
                        replacement: offset == 0,
                    }
                }
            }
        }
    };
}

impl HorizontalScrollState {
    define_init!(init, next, true);
    define_init!(init_back, next_back, false);

    pub fn scroll_right(&self, value: &str, available_space: isize) -> HorizontalScrollState {
        let (mut start, mut end, replacement) = match self {
            Fits => return Fits,
            OffScreen => return OffScreen,
            Ellided => return Ellided,
            Suffix { .. } => return self.clone(),
            Prefix { end, replacement } => (0, *end, *replacement),
            Middle { range, replacement } => (range.start, range.end, *replacement),
        };

        // A Prefix or Middle scroll state shouldn't extend to the end of the string.
        debug_assert!(end < value.len());

        let curr_visible = &value[start..end];

        // + 1 for trailing ellipsis
        let mut used_space = UnicodeWidthStr::width(curr_visible) as isize + 1;
        // + 1 for leading ellipsis
        if start != 0 {
            used_space += 1;
        }

        let mut remaining_graphemes = value[end..].graphemes(true);

        // First, show a new character to the right:

        // Presumably (because of the debug assertion above) we could
        // .unwrap() here, but I'm not sure about the exact rules of
        // Unicode segmentation, so let's not assume anything.
        if let Some(grapheme) = remaining_graphemes.next() {
            end += grapheme.len();
            used_space += UnicodeWidthStr::width(grapheme) as isize;

            // No more trailing ellipsis if at end.
            if end == value.len() {
                used_space -= 1;
            }

            // If this weren't true, we should have included the character before.
            debug_assert!(if replacement {
                used_space + 1 > available_space
            } else {
                used_space > available_space
            });
        }

        // Set this AFTER we show the next character, so that we
        // can remove it (and replace it with a replacement character) if
        // it's too wide.
        let mut visible_graphemes = value[start..end].graphemes(true);

        // Then, remove things from the start to stop using too much space.
        //
        // We may already fit if we were previously showing a replacment
        // character and moved past it.
        if used_space > available_space {
            while let Some(grapheme) = visible_graphemes.next() {
                let grapheme_width = UnicodeWidthStr::width(grapheme) as isize;

                // Moving start past start of string, there will now be
                // a leading ellipsis.
                if start == 0 {
                    used_space += 1;
                }

                start += grapheme.len();
                used_space -= grapheme_width;

                if used_space <= available_space {
                    break;
                }
            }
        }

        // If we added a character, but then also consumed, that means it
        // must be a multi-width chracter, so then we want to show a replacement
        // character.
        if start == end {
            if end == value.len() {
                return Suffix {
                    start,
                    replacement: true,
                };
            } else {
                return Middle {
                    range: start..end,
                    replacement: true,
                };
            }
        }

        // Add more characters to right if possible
        while let Some(grapheme) = remaining_graphemes.next() {
            let width = UnicodeWidthStr::width(grapheme) as isize;

            let mut used_space_with_grapheme = used_space + width;
            let now_at_end = end + grapheme.len() == value.len();
            if now_at_end {
                used_space_with_grapheme -= 1;
            }

            if used_space_with_grapheme <= available_space {
                used_space = used_space_with_grapheme;
                end += grapheme.len();
            }
        }

        if end == value.len() {
            Suffix {
                start,
                replacement: false,
            }
        } else {
            Middle {
                range: start..end,
                replacement: false,
            }
        }
    }

    pub fn scroll_left(&self, value: &str, available_space: isize) -> HorizontalScrollState {
        let (mut start, mut end, replacement) = match self {
            Fits => return Fits,
            OffScreen => return OffScreen,
            Ellided => return Ellided,
            Prefix { .. } => return self.clone(),
            Suffix { start, replacement } => (*start, value.len(), *replacement),
            Middle { range, replacement } => (range.start, range.end, *replacement),
        };

        // A Suffix or Middle scroll state shouldn't start at the start of the string.
        debug_assert!(start > 0);

        let curr_visible = &value[start..end];

        // + 1 for leading ellipsis
        let mut used_space = 1 + UnicodeWidthStr::width(curr_visible) as isize;
        // + 1 for trailing ellipsis
        if end != value.len() {
            used_space += 1;
        }

        let mut earlier_graphemes = value[..start].graphemes(true);

        // First, show a new character to the left:

        // Presumably (because of the debug assertion above) we could
        // .unwrap() here, but I'm not sure about the exact rules of
        // Unicode segmentation, so let's not assume anything.
        if let Some(grapheme) = earlier_graphemes.next_back() {
            start -= grapheme.len();
            used_space += UnicodeWidthStr::width(grapheme) as isize;

            // No more leading ellipsis if at start.
            if start == 0 {
                used_space -= 1;
            }

            // If this weren't true, we should have included the character before.
            debug_assert!(if replacement {
                used_space + 1 > available_space
            } else {
                used_space > available_space
            });
        }

        // Set this AFTER we show the next character, so that we
        // can remove it (and replace it with a replacement character) if
        // it's too wide.
        let mut visible_graphemes = value[start..end].graphemes(true);

        // Then, remove things from the end to stop using too much space.
        //
        // We may already fit if we were previously showing a replacment
        // character and moved past it.
        if used_space > available_space {
            while let Some(grapheme) = visible_graphemes.next_back() {
                let grapheme_width = UnicodeWidthStr::width(grapheme) as isize;

                // Moving end before end of string, there will now be
                // a trailing ellipsis.
                if end == value.len() {
                    used_space += 1;
                }

                end -= grapheme.len();
                used_space -= grapheme_width;

                if used_space <= available_space {
                    break;
                }
            }
        }

        // If we added a character, but then also consumed, that means it
        // must be a multi-width chracter, so then we want to show a replacement
        // character.
        if start == end {
            if start == 0 {
                return Prefix {
                    end,
                    replacement: true,
                };
            } else {
                return Middle {
                    range: start..end,
                    replacement: true,
                };
            }
        }

        // Add more characters to left if possible
        while let Some(grapheme) = earlier_graphemes.next_back() {
            let width = UnicodeWidthStr::width(grapheme) as isize;

            let mut used_space_with_grapheme = used_space + width;
            let now_at_start = start - grapheme.len() == 0;
            if now_at_start {
                used_space_with_grapheme -= 1;
            }

            if used_space_with_grapheme <= available_space {
                used_space = used_space_with_grapheme;
                start -= grapheme.len();
            }
        }

        if start == 0 {
            Prefix {
                end,
                replacement: false,
            }
        } else {
            Middle {
                range: start..end,
                replacement: false,
            }
        }
    }

    pub fn jump_to_an_end(&self, value: &str, available_space: isize) -> HorizontalScrollState {
        match self {
            Fits => Fits,
            OffScreen => OffScreen,
            Ellided => Ellided,
            Prefix { .. } => HorizontalScrollState::init_back(value, available_space),
            Middle { .. } => HorizontalScrollState::init_back(value, available_space),
            Suffix { .. } => HorizontalScrollState::init(value, available_space),
        }
    }

    pub fn expand(&self, value: &str, available_space: isize) -> HorizontalScrollState {
        let range = match self {
            Fits => return Fits,
            OffScreen => return HorizontalScrollState::init(value, available_space),
            Ellided => return HorizontalScrollState::init(value, available_space),
            Prefix { .. } => return HorizontalScrollState::init(value, available_space),
            Suffix { .. } => return HorizontalScrollState::init_back(value, available_space),
            Middle { range, .. } => range.clone(),
        };

        let start = range.start;
        let mut end = range.end;

        let curr_visible = &value[start..end];
        let mut used_space = 1 + UnicodeWidthStr::width(curr_visible) as isize;

        let mut remaining_graphemes = value[end..].graphemes(true);

        while let Some(grapheme) = remaining_graphemes.next() {
            let grapheme_width = UnicodeWidthStr::width(grapheme) as isize;
            let mut space_with_grapheme = used_space + grapheme_width;
            if end + grapheme.len() < value.len() {
                space_with_grapheme += 1;
            }

            if space_with_grapheme <= available_space {
                used_space += grapheme_width;
                end += grapheme.len();
            } else {
                break;
            }
        }

        if end == value.len() {
            // We used up all the space on the right we could, now just
            // call init_back to use up all the space on the left we can.
            HorizontalScrollState::init_back(value, available_space)
        } else {
            Middle {
                range: start..end,
                replacement: start == end,
            }
        }
    }

    pub fn shrink(&self, value: &str, available_space: isize) -> HorizontalScrollState {
        let range = match self {
            OffScreen => return OffScreen,
            Fits => return HorizontalScrollState::init(value, available_space),
            Ellided => return HorizontalScrollState::init(value, available_space),
            Prefix { .. } => return HorizontalScrollState::init(value, available_space),
            Suffix { .. } => return HorizontalScrollState::init_back(value, available_space),

            Middle { range, .. } => range.clone(),
        };

        // Won't be enough room for both ellipsis and a middle character; just jump to the start.
        if available_space < 3 {
            return HorizontalScrollState::init(value, available_space);
        }

        let start = range.start;
        let mut end = range.end;

        let curr_visible = &value[start..end];
        let mut used_space = 2 + UnicodeWidthStr::width(curr_visible) as isize;

        // Possible if it didn't prevously take up the whole width due to a wide character.
        if used_space <= available_space {
            return self.clone();
        }

        let mut visible_graphemes = curr_visible.graphemes(true);

        while let Some(grapheme) = visible_graphemes.next_back() {
            let grapheme_width = UnicodeWidthStr::width(grapheme) as isize;
            used_space -= grapheme_width;
            end -= grapheme.len();

            if used_space <= available_space || start == end {
                break;
            }
        }

        Middle {
            range: start..end,
            replacement: start == end,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scrolled(string: &str, scroll_state: &HorizontalScrollState) -> String {
        let scrolled_string = ScrolledString {
            string,
            scroll_state,
        };
        format!("{}", &scrolled_string)
    }

    #[test]
    fn test_init_and_init_back() {
        #[track_caller]
        fn assert_init(string: &str, space: isize, front: &str, back: &str) {
            let init_state = HorizontalScrollState::init(string, space);
            assert_eq!(front, scrolled(string, &init_state));

            let init_back_state = HorizontalScrollState::init_back(string, space);
            assert_eq!(back, scrolled(string, &init_back_state));
        }

        assert_init("abcde", -1, "", "");

        assert_init("abcde", 0, "", "");
        assert_init("", 0, "", "");

        assert_init("a", 1, "a", "a");
        assert_init("abc", 1, "…", "…");

        assert_init("abc", 2, "a…", "…c");
        assert_init("ab", 2, "ab", "ab");
        assert_init("🦀abc", 2, "�…", "…c");
        assert_init("abc🦀", 2, "a…", "…�");

        assert_init("abc", 3, "abc", "abc");
        assert_init("abcd", 3, "ab…", "…cd");

        assert_init("🦀🦀abc🦀🦀", 3, "🦀…", "…🦀");
        assert_init("🦀🦀abc🦀🦀", 5, "🦀🦀…", "…🦀🦀");
    }

    #[test]
    fn test_scroll_states() {
        let s = "abcdef";
        assert_scroll_states(s, 5, vec!["abcd…", "…cdef"]);

        let s = "abcdefgh";
        assert_scroll_states(s, 5, vec!["abcd…", "…cde…", "…def…", "…efgh"]);

        let s = "🦀bcde";
        assert_scroll_states(s, 5, vec!["🦀bc…", "…bcde"]);

        let s = "🦀bcdef";
        assert_scroll_states(s, 5, vec!["🦀bc…", "…bcd…", "…cdef"]);

        let s = "abcd🦀efghi";
        assert_scroll_states(s, 5, vec!["abcd…", "…d🦀…", "…🦀e…", "…efg…", "…fghi"]);

        let s = "abc🦀def";
        assert_scroll_states(s, 3, vec!["ab…", "…c…", "…�…", "…d…", "…ef"]);
    }

    #[track_caller]
    fn assert_scroll_states(string: &str, available_space: isize, states: Vec<&str>) {
        let mut curr_state = HorizontalScrollState::init(string, available_space);
        let mut prev_formatted = scrolled(string, &curr_state);
        assert_eq!(states[0], prev_formatted);

        for expected_state in states.iter().skip(1) {
            let next_state = curr_state.scroll_right(string, available_space);
            let formatted = scrolled(string, &next_state);

            assert_eq!(
                expected_state, &formatted,
                "expected scroll_right({}) to be {}",
                &prev_formatted, &expected_state,
            );
            curr_state = next_state;
            prev_formatted = formatted;
        }

        let mut curr_state = HorizontalScrollState::init_back(string, available_space);
        let mut prev_formatted = scrolled(string, &curr_state);
        assert_eq!(states.last().unwrap(), &prev_formatted);

        for expected_state in states.iter().rev().skip(1) {
            let next_state = curr_state.scroll_left(string, available_space);
            let formatted = scrolled(string, &next_state);

            assert_eq!(
                expected_state, &formatted,
                "expected scroll_left({}) to be {}",
                &prev_formatted, &expected_state,
            );
            curr_state = next_state;
            prev_formatted = formatted;
        }
    }

    #[test]
    fn test_expand() {
        let s = "abcdefghij";

        assert_expansions(
            s,
            HorizontalScrollState::init(s, 5),
            5,
            vec![
                "abcd…",
                "abcde…",
                "abcdef…",
                "abcdefg…",
                "abcdefgh…",
                "abcdefghij",
            ],
        );

        let initial_state = HorizontalScrollState::init(s, 5)
            .scroll_right(s, 5)
            .scroll_right(s, 5);

        assert_expansions(
            s,
            initial_state,
            5,
            vec![
                "…def…",
                "…defg…",
                "…defgh…",
                "…defghij",
                "…cdefghij",
                "abcdefghij",
            ],
        );

        let s = "a👍b👀c😱d";
        assert_expansions(
            s,
            HorizontalScrollState::init(s, 5),
            5,
            vec![
                "a👍b…",
                "a👍b…",
                "a👍b👀…",
                "a👍b👀c…",
                "a👍b👀c…",
                "a👍b👀c😱d",
            ],
        );
        let s = "a👍b👀c😱d";

        assert_expansions(
            s,
            HorizontalScrollState::init(s, 5)
                .scroll_right(s, 5)
                .scroll_right(s, 5),
            5,
            vec![
                "…👀c…",
                "…👀c…",
                "…👀c😱d",
                "…b👀c😱d",
                "…b👀c😱d",
                "a👍b👀c😱d",
            ],
        );
    }

    #[track_caller]
    fn assert_expansions(
        string: &str,
        initial_state: HorizontalScrollState,
        mut available_space: isize,
        states: Vec<&str>,
    ) {
        let mut curr_state = initial_state;
        let mut prev_formatted = scrolled(string, &curr_state);
        assert_eq!(states[0], prev_formatted);

        for expansion in states.iter().skip(1) {
            available_space += 1;
            let next_state = curr_state.expand(string, available_space);
            let formatted = scrolled(string, &next_state);

            assert_eq!(
                expansion, &formatted,
                "expected expand({}) to be {}",
                &prev_formatted, &expansion,
            );

            curr_state = next_state;
            prev_formatted = formatted;
        }
    }

    #[test]
    fn test_shrink() {
        let s = "abcdefghij";

        assert_shrinks(
            s,
            HorizontalScrollState::init(s, 10),
            10,
            vec![
                "abcdefghij",
                "abcdefgh…",
                "abcdefg…",
                "abcdef…",
                "abcde…",
                "abcd…",
                "abc…",
                "ab…",
                "a…",
                "…",
            ],
        );

        assert_shrinks(
            s,
            HorizontalScrollState::init(s, 9).scroll_right(s, 9),
            9,
            vec![
                "…cdefghij",
                "…defghij",
                "…efghij",
                "…fghij",
                "…ghij",
                "…hij",
                "…ij",
                "…j",
                "…",
            ],
        );

        assert_shrinks(
            s,
            HorizontalScrollState::init(s, 8).scroll_right(s, 8),
            8,
            vec![
                "…cdefgh…",
                "…cdefg…",
                "…cdef…",
                "…cde…",
                "…cd…",
                "…c…",
                "a…",
            ],
        );

        let s = "ab👍c👀d😱efg";
        assert_shrinks(
            s,
            HorizontalScrollState::init(s, 11).scroll_right(s, 11),
            11,
            vec![
                "…👍c👀d😱e…",
                "…👍c👀d😱…",
                "…👍c👀d…",
                "…👍c👀d…",
                "…👍c👀…",
                "…👍c…",
                "…👍c…",
                "…👍…",
                "…�…",
                "a…",
            ],
        );

        let s = "🦀abc";
        assert_shrinks(
            s,
            HorizontalScrollState::init(s, 5),
            5,
            vec!["🦀abc", "🦀a…", "🦀…", "�…", "…"],
        );

        let s = "abc🦀";
        assert_shrinks(
            s,
            HorizontalScrollState::init_back(s, 4),
            4,
            vec!["…c🦀", "…🦀", "…�", "…"],
        );
    }

    #[track_caller]
    fn assert_shrinks(
        string: &str,
        initial_state: HorizontalScrollState,
        mut available_space: isize,
        states: Vec<&str>,
    ) {
        let mut curr_state = initial_state;
        let mut prev_formatted = scrolled(string, &curr_state);
        assert_eq!(states[0], prev_formatted);

        for expansion in states.iter().skip(1) {
            available_space -= 1;
            let next_state = curr_state.shrink(string, available_space);
            let formatted = scrolled(string, &next_state);

            assert_eq!(
                expansion, &formatted,
                "expected shrink({}) to be {}",
                &prev_formatted, &expansion,
            );

            curr_state = next_state;
            prev_formatted = formatted;
        }
    }

    // Basic scrolling, room for 5:
    //
    // String is "abcdef"
    // scroll_right("abcd…") -> "…cdef"
    // scroll_left("…cdef") -> "abcd…"
    //
    // String is "abcdefgh"
    // scroll_right("abcd…") -> "…cde…"
    // scroll_right("…cde…") -> "…def…"
    // scroll_right("…def…") -> "…efgh"
    //
    // scroll_left("…efgh") -> "…def…"
    // scroll_left("…def…") -> "…cde…"
    // scroll_left("…cde…") -> "abcd…"
    //
    // String is "🦀bcde"
    // scroll_right("🦀bc…") -> "…bcd…"
    //   NOT "…cdef", rule is "show one more character"
    //
    // scroll_right("…bcd…") -> "…cdef"
    //
    // scroll_left("…cdef") -> "…bcd…"
    // scroll_left("…bcd…") -> "🦀bc…"
    //
    // String is "abcd🦀efghi"
    // scroll_right("abcd…") -> "…d🦀…"
    // scroll_right("…d🦀…") -> "…🦀e…"
    // scroll_right("…🦀e…") -> "…efg…"
    //   NOT just "…ef…", losing crab gives
    //   an extra character to expand to the right.
    // scroll_right("…efg…") -> "…fghi"
    //
    //
    // String is "abc🦀def", room for *3*
    // scroll_right("ab…") -> "…c…"
    // scroll_right("…c…") ->
    //   options (next)
    //     - "…" -> "…d…"
    //     - "……" -> "…d…"
    //     - "…�…" -> "…d…"            <---- I like this one ("unrepresentable")
    //       (start == end == index of 'd')
    //       - Need some bit to say to show replacement character in state
    //
    //     - "…d…", egh?
    //
    // String is "👍👀😱", room for 3:
    //
    // String is "🦀abc", room for *2*
    //   - Could show "…", or "…c"
    // String is "abc🦀", room for *2*
    // String is "abc🦀def", room for *2*
    //
    // If available room == 2, just jump back and forth between
    // start and end of string.
    // "👍🦀", can't fit either => "…"
    // "a🦀", "a…" and "�…" ???
    // "🦀b", "�…" and "…b" ???

    // Updating dimensions:
    //
    // Growing:
    // - Add more characters to end, handle whole string now fitting
    //
    // Shrinking:
    // - Remove characters from end in general
    //   - "abc…" -> "ab…"
    //
    // - Special cases:
    //   - Last character was the end, then remove from start
    //      - "…xyz" -> "…yz"
    //
    //   - Going from N to 3, may need to show replacement character
    //
    //   - Going to 2, skip to front or end
}
