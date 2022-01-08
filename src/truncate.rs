use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

// This module provides helpers for formatting text to ensure it will fit in a
// certain area. These functions are unicode aware and will correctly handle
// full-width characters, such as emoji or CJK text.
//
//
// To speed up some of these functions, we often use the following insight:
//
// For UTF-8 text, the following always holds: width(str) <= str.len()
//
// This is because in UTF-8, all codepoints that represented by a single byte
// are Unicode "halfwidth" characters. Any multi-byte character has a maximum
// width of 2, so the length of the string will always be >= the display width.

#[derive(Eq, PartialEq, Debug)]
pub enum TruncationResult<'a> {
    NoTruncation(isize),
    Truncated(&'a str, isize),
    DoesntFit,
}

// Given an input string, an amount of available space, and a replacement string
// to put in place of any truncated text, try to truncate the input string so
// that it will fit along with the replacement string in the available space.
//
// Returns an enum with three possible values:
// - NoTruncation, which means that the entire input string will fit
//   unmodified. It will return the display width of the entire input string.
// - Truncated, which means that the input string had to be truncated to fit
//   in the available space. It will return a &str reference to a prefix of the
//   input string, and the combined width taken up by the prefix string and the
//   replacement string (which is not necessarily
//   available_space - replacement.len().)
// - DoesntFit, which means that even if none of the input string were
//   displayed, it still wouldn't fit in the available space.
//
// For example, given the input string "hello world", an available space of 6,
// and a replacement string of "...", this will return Truncated(&"hel", 3),
// because "hel..." has a length of 6.
//
// This function is Unicode aware and will handle full-width characters, such as
// emoji.

macro_rules! define_truncate {
    ($fn_name:ident, $iter_fn:ident) => {
        pub fn $fn_name<'a>(
            input: &'a str,
            available_space: isize,
            replacement: &'a str,
        ) -> TruncationResult<'a> {
            // Negative available space means NOTHING can fit, but 0 available_space
            // means a empty string may still fit.
            if available_space < 0 {
                return TruncationResult::DoesntFit;
            }

            let input_width = UnicodeWidthStr::width(input) as isize;
            let replacement_width = UnicodeWidthStr::width(replacement) as isize;

            // Not quite as fast base case.
            if input_width <= available_space {
                return TruncationResult::NoTruncation(input_width);
            }

            // Compute the current width taken up by the input and its replacement.
            let mut current_width = input_width + replacement_width;
            let mut remaining_width = input_width;

            // Iterate over all the graphemes in the input so we don't break the input
            // string improperly.
            //
            // Argument to graphemes indicates we should use "extended grapheme clusters"
            // rather than "legacy grapheme clusters", as recommended.
            //
            // https://unicode-rs.github.io/unicode-segmentation/unicode_segmentation/trait.UnicodeSegmentation.html#tymethod.graphemes
            let mut graphemes = input.graphemes(true);
            while let Some(grapheme) = graphemes.$iter_fn() {
                let grapheme_width = UnicodeWidthStr::width(grapheme) as isize;
                current_width -= grapheme_width;
                remaining_width -= grapheme_width;

                if current_width <= available_space {
                    break;
                }
            }

            if graphemes.as_str().len() == 0 {
                return TruncationResult::DoesntFit;
            } else {
                return TruncationResult::Truncated(
                    graphemes.as_str(),
                    remaining_width + replacement_width,
                );
            }
        }
    };
}

define_truncate!(truncate_right_to_fit, next_back);
define_truncate!(truncate_left_to_fit, next);

// Returns the minimum number of columns required to display a string
// that could be truncated.
//
// This function is useful because longer strings will always require at
// least two columns (one for the first character, then one for the ellipsis),
// but single character strings only need one column (unless that one character
// is a wide character!).
pub fn min_required_columns_for_str(s: &str) -> isize {
    if s.is_empty() {
        return 0;
    }

    let mut graphemes = s.graphemes(true);
    let first_grapheme = graphemes.next().unwrap();
    let first_grapheme_width = UnicodeWidthStr::width(first_grapheme) as isize;

    // If the string is a single grapheme, then we only need the width of that
    // grapheme, but if it's more than that, we'll also need 1 character
    //
    if first_grapheme == s {
        return first_grapheme_width;
    } else {
        first_grapheme_width + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_right() {
        assert_not_right_truncated("hello, world", 15, "...");
        assert_not_right_truncated("👍👀😱", 6, "...");

        assert_doesnt_fit_right("hello", 3, "...");
        // Can't display half of the eyes.
        assert_doesnt_fit_right("👀abc", 1, "");

        // Handle empty strings.
        assert_doesnt_fit_right("", -1, ".");
        assert_not_right_truncated("", 0, ".");

        assert_truncated_right("hello, world", 10, "", "hello, wor", 10);
        assert_truncated_right("hello, world", 10, "...", "hello, ", 10);
        // Can't display half of the emojis.
        assert_truncated_right("👍👀😱🦀", 7, "", "👍👀😱", 6);
        assert_truncated_right("👍👀😱🦀", 6, "...", "👍", 5);
    }

    #[test]
    fn test_truncate_left() {
        assert_not_left_truncated("hello, world", 15, "...");
        assert_not_left_truncated("👍👀😱", 6, "...");

        assert_doesnt_fit_left("hello", 3, "...");
        // Can't display half of the eyes.
        assert_doesnt_fit_left("abc👀", 1, "");

        // Handle empty strings.
        assert_doesnt_fit_left("", -1, ".");
        assert_not_left_truncated("", 0, ".");

        assert_truncated_left("hello, world", 10, "", "llo, world", 10);
        assert_truncated_left("hello, world", 10, "...", ", world", 10);
        // Can't display half of the emojis.
        assert_truncated_left("👍👀😱🦀", 7, "", "👀😱🦀", 6);
        assert_truncated_left("👍👀😱🦀", 6, "...", "🦀", 5);
    }

    macro_rules! define_truncate_assertions {
        (
            $assert_not_truncated:ident,
            $assert_doesnt_fit:ident,
            $assert_truncated:ident,
            $truncate_fn:ident,
        ) => {
            #[track_caller]
            fn $assert_not_truncated(input: &str, available_space: isize, replacement: &str) {
                assert!(matches!(
                    $truncate_fn(input, available_space, replacement),
                    TruncationResult::NoTruncation(_),
                ));
            }

            #[track_caller]
            fn $assert_doesnt_fit(input: &str, available_space: isize, replacement: &str) {
                assert_eq!(
                    TruncationResult::DoesntFit,
                    $truncate_fn(input, available_space, replacement)
                );
            }

            #[track_caller]
            fn $assert_truncated(
                input: &str,
                available_space: isize,
                replacement: &str,
                truncated: &str,
                width: isize,
            ) {
                assert_eq!(
                    TruncationResult::Truncated(truncated, width),
                    $truncate_fn(input, available_space, replacement)
                );
            }
        };
    }

    define_truncate_assertions!(
        assert_not_right_truncated,
        assert_doesnt_fit_right,
        assert_truncated_right,
        truncate_right_to_fit,
    );
    define_truncate_assertions!(
        assert_not_left_truncated,
        assert_doesnt_fit_left,
        assert_truncated_left,
        truncate_left_to_fit,
    );

    #[test]
    fn test_min_required_columns() {
        assert_eq!(0, min_required_columns_for_str(""));
        assert_eq!(1, min_required_columns_for_str("a"));
        assert_eq!(2, min_required_columns_for_str("👍"));
        assert_eq!(2, min_required_columns_for_str("ab"));
        assert_eq!(2, min_required_columns_for_str("hello, world!"));
        assert_eq!(3, min_required_columns_for_str("👍 we're good!"));
    }
}
