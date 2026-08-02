use std::borrow::Cow;
use std::ops::Range;

use regex::bytes::{Regex as ByteRegex, RegexBuilder as ByteRegexBuilder};
use regex::{Captures as StrCaptures, Regex as StrRegex};

use crate::sorted_ranges::SortedRanges;

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

    fn signed_jump_size(&self) -> isize {
        match self {
            SearchDirection::Forward => 1,
            SearchDirection::Reverse => -1,
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
    should_show_matches: bool,
}

#[derive(Clone, Debug)]
pub struct LastJump {
    pub match_jumped_to: usize,
    // Needed to show 'W' next to current match number.
    pub just_wrapped: bool,
}

#[derive(Debug, Copy, Clone, Default)]
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

#[rustfmt::skip]
fn escape_literal(text: &str, inverted: InvertedPairedDelimeters) -> String {
    let escaped = regex::escape(text);
    invert_paired_delimiters(escaped.as_str(), inverted).to_string()
}

lazy_static::lazy_static! {
    static ref STARTS_WITH_WORD: StrRegex = StrRegex::new(r"\A\w").unwrap();
    static ref ENDS_WITH_WORD: StrRegex = StrRegex::new(r"\w\z").unwrap();
}

/// Creates a regex that will match the literal text passed in as a "word", adding
/// word boundary escapes ("\<" and "\>") unless the text already starts or ends
/// with a non-word character.
pub fn escape_literal_and_maybe_add_word_boundaries(
    text: &str,
    inverted: InvertedPairedDelimeters,
) -> String {
    let escaped = escape_literal(text, inverted);

    let add_start_boundary = STARTS_WITH_WORD.is_match(escaped.as_ref());
    let add_end_boundary = ENDS_WITH_WORD.is_match(escaped.as_ref());

    format!(
        "{}{escaped}{}",
        if add_start_boundary { r"\<" } else { "" },
        if add_end_boundary { r"\>" } else { "" }
    )
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
            .map_err(|err| err.to_string().replace('\n', " "))?;

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
            should_show_matches: false,
        })
    }

    pub fn find_additional_matches(&mut self, haystack: &[u8]) {
        if haystack.len() == self.len_of_searched_input {
            return;
        }

        // We don't want to search the whole input again, but we also want to make sure the
        // regex engine is sync'd up as if we had started searching at the beginning of the
        // input. We'll do this by searching at the start of the last match using `Regex::find_at`,
        // which takes into account context, so, in most cases, it should find the same match,
        // but it could be different if the regex was ended with ".*" or "$". (We could even
        // fail to find a match at all.)
        //
        // So we'll pop the previous last match, and start searching at the same place,
        // expecting to find the same match. But if we don't, we have to consider clearing
        // `last_jump` if it was pointing to this last match.
        match self.matches.pop() {
            None => {
                // Simple case, just search the whole input again.
                self.matches = self
                    .search_regex
                    .find_iter(haystack)
                    .map(|m| m.range())
                    .collect();
            }
            Some(last_match_range) => {
                let prev_last_match_index = self.matches.len();

                let mut start_index = last_match_range.start;
                while let Some(match_) = self.search_regex.find_at(haystack, start_index) {
                    self.matches.push(match_.range());
                    start_index = match_.end();
                }

                if let Some(last_jump) = &self.last_jump {
                    if last_jump.match_jumped_to == prev_last_match_index {
                        // The last jump was to the last match. We have to make sure
                        // the last match is unchanged, otherwise we should clear it.
                        match self.matches.get(prev_last_match_index) {
                            None => self.last_jump = None,
                            Some(new_last_match_range) => {
                                if last_match_range != *new_last_match_range {
                                    self.last_jump = None;
                                }
                            }
                        }
                    }
                }
            }
        }

        self.len_of_searched_input = haystack.len();
    }

    pub fn search_input(&self) -> &str {
        self.search_input.as_str()
    }

    pub fn set_search_direction(&mut self, direction: SearchDirection) {
        self.direction = direction;
    }

    pub fn search_direction(&self) -> SearchDirection {
        self.direction
    }

    pub fn num_matches(&self) -> usize {
        self.matches.len()
    }

    pub fn last_jump(&self) -> Option<&LastJump> {
        self.last_jump.as_ref()
    }

    pub fn last_match_range(&self) -> Option<Range<usize>> {
        self.last_jump
            .as_ref()
            .map(|lj| self.matches[lj.match_jumped_to].clone())
    }

    pub fn search_match_ranges(&self) -> &[Range<usize>] {
        &self.matches
    }

    pub fn should_show_matches(&self) -> bool {
        self.should_show_matches
    }

    pub fn stop_searching(&mut self) {
        self.last_jump = None;
        self.should_show_matches = false;
    }

    pub fn clear_last_jump_but_keep_showing_matches(&mut self) {
        self.last_jump = None;
    }

    pub fn jump_to_next_match(
        &mut self,
        current_focused_range: Range<usize>,
        jump_direction: JumpDirection,
        jumps: usize,
        cursor_will_move: &dyn Fn(Range<usize>) -> bool,
        is_match_visible: &dyn Fn(Range<usize>) -> bool,
    ) -> Range<usize> {
        debug_assert!(jumps != 0);

        if self.matches.is_empty() {
            panic!("Shouldn't call `jump_to_match` if no matches.");
        }

        let search_direction = self.direction_of_jump(jump_direction);

        // When the user jumps to a match, they should see some visible indication that their
        // action did something. This means that one of the following must be happen if possible:
        // - the cursor must move; or
        // - the match jumped to must be visible
        //
        // (Note that if all the matches are in the same collapsed container then obviously we
        // can't force one of these to happen.)
        //
        // Note that if the cursor doesn't move, and there was a visible match, but then there's
        // no longer a visible match, then there is a visible indication that something happened,
        // but it just sort of feels like searching was turned off.
        //
        //
        // Enforcing this rule ensures the correct behavior around collapsed containers.
        //
        // Suppose there's a collapsed container that contains two possible matches. When
        // we jump to the first match, the cursor will move, but that match won't be visible.
        // When we jump again, if we tried to jump to the second match in the container,
        // nothing would happen, so we make sure that we jump to a match after the container.
        //
        // Similarly, if we start a search on a collapsed container, even if there's a match
        // inside, jumping to that wouldn't do anything, so in that case we also jump to the
        // first match after the container.

        // We'll find the prospective next match based on how many jumps the user wants to
        // go, then, if the "something must visibly change" criteria isn't met, we'll just
        // keep advancing one match at a time until it is.
        let (prospective_match, wrapped) = match &self.last_jump {
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
                match_jumped_to, ..
            }) => {
                let delta = match search_direction {
                    SearchDirection::Forward => jumps as isize,
                    SearchDirection::Reverse => -(jumps as isize),
                };

                self.cycle_match(*match_jumped_to, delta)
            }
        };

        let mut next_match = prospective_match;
        let mut ever_wrapped = wrapped;

        let unit_step = search_direction.signed_jump_size();

        loop {
            let cursor_moved = cursor_will_move(self.matches[next_match].clone());
            let next_match_is_visible = is_match_visible(self.matches[next_match].clone());

            if cursor_moved || next_match_is_visible {
                break;
            }

            let (next_prospective_match, wrapped) = self.cycle_match(next_match, unit_step);
            next_match = next_prospective_match;
            ever_wrapped = ever_wrapped || wrapped;

            // We've looped, so we'll just stop where we started.
            if next_match == prospective_match {
                break;
            }
        }

        let next_match_range = self.matches[next_match].clone();

        self.last_jump = Some(LastJump {
            match_jumped_to: next_match,
            just_wrapped: wrapped,
        });
        self.should_show_matches = true;

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
                let next_match = self
                    .matches
                    .index_of_first_elem_starting_at_or_after(range.end);

                // If NONE of the matches start after the end of the focused node, then we
                // want to jump back to the start in that case.
                match next_match {
                    None => (0, true),
                    Some(index) => (index, false),
                }
            }
            SearchDirection::Reverse => {
                // When searching backwards, we want the last match that
                // ends before the start of focused range.
                let prev_match = self
                    .matches
                    .index_of_last_elem_ending_at_or_before(range.start);

                // If there are no matches before the start of the focused node, we need to
                // wrap around to the end of the file.
                match prev_match {
                    None => (self.matches.len() - 1, true),
                    Some(index) => (index, false),
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
            1 => num_matches <= delta.unsigned_abs() || new_index < start_index,
            -1 => num_matches <= delta.unsigned_abs() || start_index < new_index,
            _ => unreachable!(),
        };

        (new_index, wrapped)
    }
}

pub enum HighlightKind {
    CurrentMatch,
    OtherMatch,
    NotAMatch,
}

pub struct SearchMatchHighlighter<'a> {
    all_search_matches: &'a [Range<usize>],
    current_match_index: Option<usize>,
    // When we create a `SingleRangeHighlighter` and iterate through it, we guarantee that the first
    // match here always ends after the start of `remaining_range` (or the first match is an empty
    // range starting and ending at the start of `remaining_range`.)
    subsequent_search_matches: &'a [Range<usize>],
    match_index_of_first_subsequent_search_match: usize,
    // We assume that most of the time we'll call `highlight` on sorted ranges (e.g. on [10, 20),
    // then [20, 30), then [35, 40), etc.), and leverage this to avoid repeatedly searching
    // through `all_search_matches` to find the next range.
    //
    // But if we do backtrack and highlight something we've already passed over, we'll need
    // to reset `subsequent_search_matches` back to `all_search_matches`.
    reset_subsequent_search_matches_if_next_range_starts_before: usize,
}

struct SingleRangeHighlighter<'mh, 'a> {
    match_highlighter: &'mh mut SearchMatchHighlighter<'a>,
    remaining_range: Range<usize>,
}

impl<'a> SearchMatchHighlighter<'a> {
    pub fn new(search_matches: &'a [Range<usize>], current_match_index: Option<usize>) -> Self {
        SearchMatchHighlighter {
            all_search_matches: search_matches,
            current_match_index,
            subsequent_search_matches: search_matches,
            match_index_of_first_subsequent_search_match: 0,
            reset_subsequent_search_matches_if_next_range_starts_before: 0,
        }
    }

    fn advance_subsequent_search_matches(&mut self) {
        self.subsequent_search_matches = &self.subsequent_search_matches[1..];
        self.match_index_of_first_subsequent_search_match += 1;
    }

    pub fn highlight<'mh>(
        &'mh mut self,
        range: Range<usize>,
    ) -> impl Iterator<Item = (Range<usize>, HighlightKind)> + use<'mh, 'a> {
        if range.start < self.reset_subsequent_search_matches_if_next_range_starts_before {
            self.subsequent_search_matches = self.all_search_matches;
            self.match_index_of_first_subsequent_search_match = 0;
            self.reset_subsequent_search_matches_if_next_range_starts_before = 0;
        }

        self.reset_subsequent_search_matches_if_next_range_starts_before = range.end;

        if !self.subsequent_search_matches.is_empty() {
            let subsequent_search_match = &self.subsequent_search_matches[0];
            if subsequent_search_match.end <= range.start {
                self.advance_subsequent_search_matches();

                // We finished processing one search match. We assume that in most cases the search
                // matches are few and far between, while the ranges we're highlighting are in close
                // proximity. So we expect the next search match (if there is one) to be ahead of
                // the highlight range, but if it's not, that possibly means we jumped past a big
                // collapsed portion with many search matches, so we'll binary search again for the
                // relevant ones.
                match self.subsequent_search_matches.get(0) {
                    None => (), // No more matches, nothing to do.
                    Some(next_match_range) => {
                        if next_match_range.end <= range.start {
                            match self
                                .all_search_matches
                                .index_of_first_elem_starting_at_or_after(range.start)
                            {
                                None => {
                                    self.subsequent_search_matches = &[];
                                    self.match_index_of_first_subsequent_search_match =
                                        self.all_search_matches.len();
                                }
                                Some(index) => {
                                    self.subsequent_search_matches =
                                        &self.all_search_matches[index..];
                                    self.match_index_of_first_subsequent_search_match = index;
                                }
                            };
                        }
                    }
                }
            }
        }

        SingleRangeHighlighter {
            match_highlighter: self,
            remaining_range: range,
        }
    }
}

impl<'mh, 'a> Iterator for SingleRangeHighlighter<'mh, 'a> {
    type Item = (Range<usize>, HighlightKind);

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining_range.is_empty() {
            return None;
        }

        if self.match_highlighter.subsequent_search_matches.is_empty() {
            let unmatched_range = self.remaining_range.clone();
            self.remaining_range = unmatched_range.end..unmatched_range.end;
            return Some((unmatched_range, HighlightKind::NotAMatch));
        }

        let search_match_range = &self.match_highlighter.subsequent_search_matches[0];

        if self.remaining_range.start < search_match_range.start {
            // Next match hasn't started yet; return up to the start of
            // the next match.
            let unmatched_end = usize::min(self.remaining_range.end, search_match_range.start);
            let unmatched_range = self.remaining_range.start..unmatched_end;
            self.remaining_range = unmatched_range.end..self.remaining_range.end;
            return Some((unmatched_range, HighlightKind::NotAMatch));
        }

        let match_end = usize::min(self.remaining_range.end, search_match_range.end);
        let match_range = self.remaining_range.start..match_end;
        self.remaining_range = match_end..self.remaining_range.end;

        let highlight_kind = match self.match_highlighter.current_match_index {
            None => HighlightKind::OtherMatch,
            Some(index) => {
                if self
                    .match_highlighter
                    .match_index_of_first_subsequent_search_match
                    == index
                {
                    HighlightKind::CurrentMatch
                } else {
                    HighlightKind::OtherMatch
                }
            }
        };

        if match_end == search_match_range.end {
            self.match_highlighter.advance_subsequent_search_matches();
        }

        if match_range.is_empty() {
            // Don't return empty ranges; just recurse.
            self.next()
        } else {
            Some((match_range, highlight_kind))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use insta::{assert_debug_snapshot, assert_snapshot};

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
    fn test_escape_literals() {
        let json_inverted_paired_delimiters = InvertedPairedDelimeters {
            square_brackets: true,
            curly_braces: true,
            parentheses: false,
        };

        let json_tests = vec![
            (r"a\b", r"a\\b"),
            (r"[]", r"[]"),
            (r"{}", r"{}"),
            (r"()", r"\(\)"),
            (r"\[abc\]", r"\\[abc\\]"),
            (r"\{1,3\}", r"\\{1,3\\}"),
            (r"\(.\)", r"\\\(\.\\\)"),
        ];

        for (before, after) in json_tests.into_iter() {
            assert_eq!(
                after,
                escape_literal(before, json_inverted_paired_delimiters)
            );
        }

        let sexp_inverted_paired_delimiters = InvertedPairedDelimeters {
            square_brackets: false,
            curly_braces: false,
            parentheses: true,
        };

        let sexp_tests = vec![
            (r"a\b", r"a\\b"),
            (r"[]", r"\[\]"),
            (r"{}", r"\{\}"),
            (r"()", r"()"),
            (r"\[abc\]", r"\\\[abc\\\]"),
            (r"\{1,3\}", r"\\\{1,3\\\}"),
            (r"\(.\)", r"\\(\.\\)"),
        ];

        for (before, after) in sexp_tests.into_iter() {
            assert_eq!(
                after,
                escape_literal(before, sexp_inverted_paired_delimiters)
            );
        }
    }

    #[test]
    fn escape_literals_and_add_word_boundaries() {
        let inverted_paired_delimiters = InvertedPairedDelimeters {
            square_brackets: true,
            curly_braces: true,
            parentheses: false,
        };

        let tests = vec![
            (r"abc", r"\<abc\>"),
            (r"[abc", r"[abc\>"),
            (r"abc]", r"\<abc]"),
            (r"(abc)", r"\(abc\)"),
        ];

        for (before, after) in tests.into_iter() {
            assert_eq!(
                after,
                escape_literal_and_maybe_add_word_boundaries(before, inverted_paired_delimiters,),
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

    #[test]
    fn test_find_additional_matches() {
        let mut search_state = SearchState::new(
            "abc abc".to_string(),
            b"-abc abc abc",
            SearchDirection::Forward,
            InvertedPairedDelimeters::default(),
        )
        .unwrap();

        assert_debug_snapshot!(search_state.matches, @r"
        [
            1..8,
        ]
        ");

        search_state.last_jump = Some(LastJump {
            match_jumped_to: 0,
            just_wrapped: false,
        });

        search_state.find_additional_matches(b"-abc abc abc abc");
        assert_debug_snapshot!(search_state.matches, @r"
        [
            1..8,
            9..16,
        ]
        ");

        assert!(search_state.last_jump.is_some());
    }

    #[test]
    fn test_find_additional_matches_last_match_is_removed() {
        let mut search_state = SearchState::new(
            r"abc\b".to_string(),
            b"-abc abc",
            SearchDirection::Forward,
            InvertedPairedDelimeters::default(),
        )
        .unwrap();

        assert_debug_snapshot!(search_state.matches, @r"
        [
            1..4,
            5..8,
        ]
        ");

        search_state.last_jump = Some(LastJump {
            match_jumped_to: 1,
            just_wrapped: false,
        });

        search_state.find_additional_matches(b"-abc abcdef");
        assert_debug_snapshot!(search_state.matches, @r"
        [
            1..4,
        ]
        ");
        assert!(search_state.last_jump.is_none());
    }

    #[test]
    fn test_find_additional_matches_last_match_is_updated() {
        let mut search_state = SearchState::new(
            r"abc\b".to_string(),
            b"-abc abc",
            SearchDirection::Forward,
            InvertedPairedDelimeters::default(),
        )
        .unwrap();

        assert_debug_snapshot!(search_state.matches, @r"
        [
            1..4,
            5..8,
        ]
        ");

        search_state.last_jump = Some(LastJump {
            match_jumped_to: 1,
            just_wrapped: false,
        });

        search_state.find_additional_matches(b"-abc abcdef abc");
        assert_debug_snapshot!(search_state.matches, @r"
        [
            1..4,
            12..15,
        ]
        ");
        assert!(search_state.last_jump.is_none());
    }

    #[test]
    fn test_match_highlighter() {
        let search_matches = vec![10..20, 30..40, 40..40, 40..40, 50..50, 50..60, 70..70];
        let mut highlighter = SearchMatchHighlighter::new(&search_matches, Some(1));

        fn f(highlighter: &mut SearchMatchHighlighter<'_>, range: Range<usize>) -> String {
            highlighter
                .highlight(range)
                .map(|(r, b)| {
                    let prefix = match b {
                        HighlightKind::CurrentMatch => "current match",
                        HighlightKind::OtherMatch => "  other match",
                        HighlightKind::NotAMatch => " not matching",
                    };
                    format!("{prefix}: {r:?}")
                })
                .collect::<Vec<_>>()
                .join("\n")
        }

        assert_snapshot!(f(&mut highlighter, 0..31), @r"
         not matching: 0..10
          other match: 10..20
         not matching: 20..30
        current match: 30..31
        ");

        // Backtrack
        assert_snapshot!(f(&mut highlighter, 32..33), @"current match: 32..33");
        assert_snapshot!(f(&mut highlighter, 33..36), @"current match: 33..36");
        assert_snapshot!(f(&mut highlighter, 37..40), @"current match: 37..40");

        // Backtrack
        assert_snapshot!(f(&mut highlighter, 30..40), @"current match: 30..40");
        assert_snapshot!(f(&mut highlighter, 40..45), @" not matching: 40..45");

        assert_snapshot!(f(&mut highlighter, 30..45), @r"
        current match: 30..40
         not matching: 40..45
        ");

        assert_snapshot!(f(&mut highlighter, 35..45), @r"
        current match: 35..40
         not matching: 40..45
        ");

        assert_snapshot!(f(&mut highlighter, 40..45), @" not matching: 40..45");

        assert_snapshot!(f(&mut highlighter, 40..55), @r"
        not matching: 40..50
         other match: 50..55
        ");

        assert_snapshot!(f(&mut highlighter, 50..65), @r"
         other match: 50..60
        not matching: 60..65
        ");

        assert_snapshot!(f(&mut highlighter, 65..75), @r"
        not matching: 65..70
        not matching: 70..75
        ");

        assert_snapshot!(f(&mut highlighter, 70..70), @"");

        // One more backtrack for good measure
        assert_snapshot!(f(&mut highlighter, 0..75), @r"
         not matching: 0..10
          other match: 10..20
         not matching: 20..30
        current match: 30..40
         not matching: 40..50
          other match: 50..60
         not matching: 60..70
         not matching: 70..75
        ");

        let search_matches = vec![10..15, 20..25, 30..35, 40..45, 50..55, 60..65];
        let mut highlighter = SearchMatchHighlighter::new(&search_matches, None);

        assert_snapshot!(f(&mut highlighter, 0..18), @r"
        not matching: 0..10
         other match: 10..15
        not matching: 15..18
        ");

        // Jump ahead one match
        assert_snapshot!(f(&mut highlighter, 26..28), @" not matching: 26..28");
        assert_snapshot!(f(&mut highlighter, 29..38), @r"
        not matching: 29..30
         other match: 30..35
        not matching: 35..38
        ");

        // Jump ahead multiple matches
        assert_snapshot!(f(&mut highlighter, 26..28), @" not matching: 26..28");
        assert_snapshot!(f(&mut highlighter, 58..68), @r"
        not matching: 58..60
         other match: 60..65
        not matching: 65..68
        ");
    }
}
