use std::borrow::Cow;
use std::ops::Range;

use regex::bytes::{Regex as ByteRegex, RegexBuilder as ByteRegexBuilder};
use regex::{Captures as StrCaptures, Regex as StrRegex};

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum SearchDirection {
    Forward,
    Reverse,
}

impl SearchDirection {
    pub fn prompt_str(&self) -> &'static str {
        match self {
            SearchDirection::Forward => "/",
            SearchDirection::Reverse => "?",
        }
    }
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum JumpDirection {
    Next,
    Prev,
}

#[derive(Debug)]
pub struct SearchState {
    search_input: String,
    search_regex: ByteRegex,
    matches: Vec<Range<usize>>,
    len_of_searched_input: usize,
    direction: SearchDirection,
    last_jump: Option<LastJump>,
}

#[derive(Debug)]
struct LastJump {
    match_jumped_to: usize,
    // Needed to show 'W' next to current match number.
    just_wrapped: bool,
    // Needed to know to jump *past* a collapsed container, even though
    // there are matches in between the current focus and the end of that
    // container.
    jumped_into_collapsed_container: bool,
}

#[derive(Debug, Copy, Clone)]
pub struct InvertedPairedDelimeters {
    pub square_brackets: bool,
    pub curly_braces: bool,
    pub parentheses: bool,
}

// By default, we *don't* want paired delimiters to have their usual meaning in
// a regex, to make it easier for users to search the document and anchor on the
// structural syntax (e.g., searching for '"foo": []', to find instances where
// "foo" is an empty array).
//
// To handle this, we'll loop over all the paired delimiters in the input, matching
// on both escaped ones (e.g. '\['), and plain ones ('['), and invert them to the
// the normal escaped state expected by the regex crate.
//
// We put the escaped versions first so that they are matched first. We also match
// on escaped backslashes, to make sure we don't think the trailing one was used to
// escape the delimiter. E.g. the typed input '\\[' is a search for the literal '\[',
// not for '\' and then the opening of a chacter class.
lazy_static::lazy_static! {
    static ref ESCAPED_AND_NAKED_PAIRED_DELIMITERS: StrRegex = StrRegex::new(r#"(?x)
               # matched term
        ( \\\\ # \\
        | \\\[ # \[
        |   \[ #  [
        | \\\] # \]
        |   \] #  ]
        | \\\{ # \{
        |   \{ #  {
        | \\\} # \}
        |   \} #  }
        | \\\( # \(
        |   \( #  (
        | \\\) # \)
        |   \) #  )
        )"#).unwrap();
}

#[rustfmt::skip]
fn invert_paired_delimiters(regex: &str, inverted: InvertedPairedDelimeters) -> Cow<'_, str> {
    ESCAPED_AND_NAKED_PAIRED_DELIMITERS.replace_all(regex, |caps: &StrCaptures| match &caps[0] {
        r"\\" => r"\\", // Keep escaped backslashes as is.
        r"\[" => if inverted.square_brackets {   "[" } else { r"\[" },
          "[" => if inverted.square_brackets { r"\[" } else {   "[" },
        r"\]" => if inverted.square_brackets {   "]" } else { r"\]" },
          "]" => if inverted.square_brackets { r"\]" } else {   "]" },
        r"\{" => if inverted.curly_braces    {   "{" } else { r"\{" },
          "{" => if inverted.curly_braces    { r"\{" } else {   "{" },
        r"\}" => if inverted.curly_braces    {   "}" } else { r"\}" },
          "}" => if inverted.curly_braces    { r"\}" } else {   "}" },
        r"\(" => if inverted.parentheses     {   "(" } else { r"\(" },
          "(" => if inverted.parentheses     { r"\(" } else {   "(" },
        r"\)" => if inverted.parentheses     {   ")" } else { r"\)" },
          ")" => if inverted.parentheses     { r"\)" } else {   ")" },
        _ => unreachable!(),
    })
}

lazy_static::lazy_static! {
    static ref UPPER_CASE: StrRegex = StrRegex::new("[[:upper:]]").unwrap();
}

// By default, searches will be "smart case", i.e., case-sensitive if there are any
// uppercase letters in the input, and case-insensitive otherwise. But to force a
// case sensitive match on an input with only lowercase letters, "/s" can be appended
// to the search term. A trailing "/" (without the 's') can also be appened (as in vim),
// and it will simply be ignored.
fn extract_regex_input_and_case_sensitivity(search_input: &str) -> (&str, bool) {
    let regex_input;
    let mut case_sensitive_specified = false;

    if let Some(stripped_of_slash) = search_input.strip_suffix('/') {
        regex_input = stripped_of_slash;
    } else if let Some(stripped_of_slash_s) = search_input.strip_suffix("/s") {
        regex_input = stripped_of_slash_s;
        case_sensitive_specified = true;
    } else {
        regex_input = search_input;
    }

    let case_sensitive = if case_sensitive_specified {
        true
    } else {
        UPPER_CASE.is_match(regex_input)
    };

    (regex_input, case_sensitive)
}

impl SearchState {
    pub fn new(
        search_input: String,
        haystack: &[u8],
        direction: SearchDirection,
        inverted_paired_delimiters: InvertedPairedDelimeters,
    ) -> Result<Self, String> {
        let (regex_input, case_sensitive) = extract_regex_input_and_case_sensitivity(&search_input);

        let inverted = invert_paired_delimiters(regex_input, inverted_paired_delimiters);

        if regex_input.is_empty() {
            return Err("Cannot search for empty string".to_string());
        }

        let search_regex = ByteRegexBuilder::new(&inverted)
            .case_insensitive(!case_sensitive)
            .build()
            .map_err(|err| err.to_string())?;

        let matches: Vec<Range<usize>> = search_regex
            .find_iter(haystack)
            .map(|m| m.range())
            .collect();

        Ok(SearchState {
            search_input,
            search_regex,
            matches,
            len_of_searched_input: haystack.len(),
            direction,
            last_jump: None,
        })
    }

    pub fn set_search_direction(&mut self, direction: SearchDirection) {
        self.direction = direction;
    }

    pub fn num_matches(&self) -> usize {
        self.matches.len()
    }

    pub fn clear_last_jump(&mut self) {
        self.last_jump = None;
    }

    pub fn jump_to_next_match(
        &mut self,
        current_focused_range: Range<usize>,
        jump_direction: JumpDirection,
        jumps: usize,
        is_match_in_collapsed_container: &dyn Fn(Range<usize>) -> bool,
    ) -> Range<usize> {
        debug_assert!(jumps != 0);

        if self.matches.is_empty() {
            panic!("Shouldn't call `jump_to_match` if no matches.");
        }

        let search_direction = self.direction_of_jump(jump_direction);

        let (next_match_index, wrapped) = match &self.last_jump {
            None => {
                let (closest_match, wrapped_while_going_to_closest_match) =
                    self.closest_match_to_range(current_focused_range, search_direction);

                let delta = match search_direction {
                    SearchDirection::Forward => (jumps - 1) as isize,
                    SearchDirection::Reverse => -((jumps - 1) as isize),
                };

                let (final_match, wrapped_while_cycling) = self.cycle_match(closest_match, delta);
                let wrapped = wrapped_while_going_to_closest_match || wrapped_while_cycling;

                (final_match, wrapped)
            }
            Some(LastJump {
                match_jumped_to,
                jumped_into_collapsed_container,
                ..
            }) => {
                let start_match = *match_jumped_to;
                let jumped_into_collapsed_container = *jumped_into_collapsed_container;

                let delta = match search_direction {
                    SearchDirection::Forward => jumps as isize,
                    SearchDirection::Reverse => -(jumps as isize),
                };

                // Jump by `delta` the first time.
                let (mut next_match, wrapped) = self.cycle_match(start_match, delta);
                let mut ever_wrapped = wrapped;

                if jumped_into_collapsed_container {
                    // If we previously jumped into a container, we'll make sure that the
                    // the cursor moves by advancing to the next match after the container.
                    // If you hit '3n', this might result in something different than hitting
                    // 'n' three times, but that's fine -- that's not the intended semantics.
                    // The semantics are: "jump N matches from previous match, then round up".
                    let unit_step = delta.signum(); // Returns 1 or -1 (or 0, but delta isn't 0).

                    // If all the matches are in a single collapsed container, this might happen.
                    // We want to make sure we don't infinitely loop.
                    while next_match != start_match {
                        // Check if we're still in the same container by getting the cursor
                        // pointed to by the match and seeing if that's the same as the current
                        // cursor.
                        if !is_match_in_collapsed_container(self.matches[next_match].clone()) {
                            break;
                        }

                        let (next_next_match, wrapped) = self.cycle_match(next_match, unit_step);
                        next_match = next_next_match;
                        ever_wrapped = ever_wrapped || wrapped;
                    }
                }

                (next_match, ever_wrapped)
            }
        };

        let next_match_range = self.matches[next_match_index].clone();

        self.last_jump = Some(LastJump {
            match_jumped_to: next_match_index,
            just_wrapped: wrapped,
            jumped_into_collapsed_container: is_match_in_collapsed_container(
                next_match_range.clone(),
            ),
        });

        next_match_range
    }

    fn direction_of_jump(&self, jump_direction: JumpDirection) -> SearchDirection {
        use JumpDirection::*;
        use SearchDirection::*;

        match (self.direction, jump_direction) {
            (Forward, Next) | (Reverse, Prev) => Forward,
            (Forward, Prev) | (Reverse, Next) => Reverse,
        }
    }

    fn closest_match_to_range(
        &mut self,
        range: Range<usize>,
        search_direction: SearchDirection,
    ) -> (usize, bool) {
        // Note: `partition_point` is awkward and returns the first index
        // where the predicate returns *false*.
        match search_direction {
            SearchDirection::Forward => {
                // When searching forwards, we want the first match that starts
                // _after_ the current focus. This is pretty subtle; for example,
                // if you're focused on a key/value pair where the value has multiple
                // matches, you should jump to the first of those. But if you search
                // for the key itself, it should go to the next iteration of that key.
                // We'll let `Viewer::currently_focused_content_range` deal with the
                // complexities of turning the current cursor into a reasonable range,
                // and assume that it does something like returning the range of the
                // key, but the range doesn't include the value.
                let next_match = self.matches.partition_point(|match_range| {
                    // This condition starts false, then becomes true, so we have
                    // to invert it for `partition_point`.
                    let match_starts_after_focused_range = range.end <= match_range.start;
                    !match_starts_after_focused_range
                });

                // If NONE of the matches start after the end of the focused row,
                // partition_point returns the length of the array, but then we
                // want to jump back to the start in that case.
                if next_match == self.matches.len() {
                    (0, true)
                } else {
                    (next_match, false)
                }
            }
            SearchDirection::Reverse => {
                // When searching backwards, we want the last match that
                // ends before the start of focused range.
                //
                // `partition_point` returns the first index where the condition
                // is false, so it will return the match after the one we actually
                // want.
                let match_after_prev_match = self
                    .matches
                    .partition_point(|match_range| match_range.end < range.start);

                // If the very first match ends the start of the focused row,
                // then partition_point will return 0, and we need to wrap
                // around to the end of the file.
                if match_after_prev_match == 0 {
                    (self.matches.len() - 1, true)
                } else {
                    (match_after_prev_match - 1, false)
                }
            }
        }
    }

    fn cycle_match(&self, start_index: usize, delta: isize) -> (usize, bool) {
        Self::cycle_match_impl(start_index, delta, self.matches.len())
    }

    fn cycle_match_impl(start_index: usize, delta: isize, num_matches: usize) -> (usize, bool) {
        // a % b computes the remainder of a divided by b, so if a is negative, a % b is also
        // negative. To compute the new index we want to use the "Euclidean remainder", aka
        // modulo.
        let new_index = (start_index as isize + delta).rem_euclid(num_matches as isize) as usize;

        let wrapped = match delta.signum() {
            0 => false,
            1 => num_matches <= (delta.abs() as usize) || new_index < start_index,
            -1 => num_matches <= (delta.abs() as usize) || start_index < new_index,
            _ => unreachable!(),
        };

        (new_index, wrapped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_regex_input_and_case_sensitivity() {
        let tests = vec![
            ("abc", ("abc", false)),
            ("Abc", ("Abc", true)),
            ("abc/", ("abc", false)),
            ("abc/s", ("abc", true)),
            ("abc/s/", ("abc/s", false)),
        ];

        for (input, search_term_and_case_sensitivity) in tests.into_iter() {
            assert_eq!(
                search_term_and_case_sensitivity,
                extract_regex_input_and_case_sensitivity(input),
            );
        }
    }

    #[test]
    fn test_invert_paired_delimiter_escaping() {
        let json_inverted_paired_delimiters = InvertedPairedDelimeters {
            square_brackets: true,
            curly_braces: true,
            parentheses: false,
        };

        let json_tests = vec![
            (r"[]", r"\[\]"),
            (r"{}", r"\{\}"),
            (r"()", r"()"),
            (r"\[abc\]", r"[abc]"),
            (r"\{1,3\}", r"{1,3}"),
            (r"(\[[]\])", r"([\[\]])"),
            (r"\\[ \\\[", r"\\\[ \\["),
        ];

        for (before, after) in json_tests.into_iter() {
            assert_eq!(
                after,
                invert_paired_delimiters(before, json_inverted_paired_delimiters)
            );
        }

        let sexp_inverted_paired_delimiters = InvertedPairedDelimeters {
            square_brackets: false,
            curly_braces: false,
            parentheses: true,
        };

        let sexp_tests = vec![
            (r"[]", r"[]"),
            (r"{}", r"{}"),
            (r"()", r"\(\)"),
            (r"((a 1))", r"\(\(a 1\)\)"),
            (r"\(\[\]|\{\}\)", r"(\[\]|\{\})"),
            (r"\\( \\\(", r"\\\( \\("),
        ];

        for (before, after) in sexp_tests.into_iter() {
            assert_eq!(
                after,
                invert_paired_delimiters(before, sexp_inverted_paired_delimiters)
            );
        }
    }

    #[test]
    fn test_cycle_match() {
        fn check(start: usize, delta: isize, num_matches: usize) -> (usize, bool) {
            SearchState::cycle_match_impl(start, delta, num_matches)
        }

        assert_eq!(check(0, 0, 10), (0, false));
        assert_eq!(check(0, 5, 10), (5, false));
        assert_eq!(check(0, 15, 10), (5, true));
        assert_eq!(check(5, 3, 10), (8, false));
        assert_eq!(check(7, 3, 10), (0, true));
        assert_eq!(check(7, 20, 10), (7, true));

        assert_eq!(check(9, -5, 10), (4, false));
        assert_eq!(check(9, -15, 10), (4, true));
        assert_eq!(check(5, -3, 10), (2, false));
        assert_eq!(check(1, -3, 10), (8, true));
        assert_eq!(check(7, -20, 10), (7, true));
    }
}
