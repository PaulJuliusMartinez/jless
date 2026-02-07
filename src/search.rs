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
    immediate_state: ImmediateSearchState,
}

#[derive(Debug, Copy, Clone)]
pub struct InvertedPairedDelimeters {
    square_brackets: bool,
    curly_braces: bool,
    parentheses: bool,
}

pub static JSON_INVERTED_PAIRED_DELIMITERS: InvertedPairedDelimeters = InvertedPairedDelimeters {
    square_brackets: true,
    curly_braces: true,
    parentheses: false,
};

pub static SEXP_INVERTED_PAIRED_DELIMITERS: InvertedPairedDelimeters = InvertedPairedDelimeters {
    square_brackets: false,
    curly_braces: false,
    parentheses: true,
};

// Someday: If we implement persistent highlighting of search matches, like vim's
// default setting (configured by e.g. `hlsearch`), then `NotSearching` and
// `MatchesVisible` should probably be tracked at a higher level, and we should
// consider the `ActivelySearching` variant as a `LastJump` option.
#[derive(Debug)]
enum ImmediateSearchState {
    // The user is actively searching, having just started the search, or jumped
    // to another match.
    ActivelySearching {
        last_match_jumped_to: usize,
        // Needed to show 'W' next to current match number.
        just_wrapped: bool,
        // Needed to know to jump *past* a collapsed container, even though
        // there are matches in between the current focus and the end of that
        // container.
        last_search_into_collapsed_container: bool,
    },
    // The user has moved their cursor away from a match; or hit escape to
    // hide search matches. In this state, matches are not highlighted.
    NotSearching,
    // If the user searches for something in a collapsed node, then expands it,
    // we still want to show matches, but they're no longer on top of match, so
    // we should jump to the next visible match when then hit 'n'.
    MatchesVisible,
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
    ) -> Option<Self> {
        let (regex_input, case_sensitive) = extract_regex_input_and_case_sensitivity(&search_input);

        if regex_input.is_empty() {
            return None;
        }

        let inverted = invert_paired_delimiters(regex_input, inverted_paired_delimiters);

        let search_regex = ByteRegexBuilder::new(&inverted)
            .case_insensitive(!case_sensitive)
            .build()
            .ok()?;

        let matches: Vec<Range<usize>> = search_regex
            .find_iter(haystack)
            .map(|m| m.range())
            .collect();

        Some(SearchState {
            search_input,
            search_regex,
            matches,
            len_of_searched_input: haystack.len(),
            direction,
            immediate_state: ImmediateSearchState::NotSearching,
        })
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
                invert_paired_delimiters(before, JSON_INVERTED_PAIRED_DELIMITERS)
            );
        }

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
                invert_paired_delimiters(before, SEXP_INVERTED_PAIRED_DELIMITERS)
            );
        }
    }
}
