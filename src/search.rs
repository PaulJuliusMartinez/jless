use regex::Regex;
use std::ops::Range;
use std::slice::Iter;

use crate::flatjson::{FlatJson, Index};

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum SearchDirection {
    Forward,
    Reverse,
}

impl SearchDirection {
    pub fn prompt_char(&self) -> char {
        match self {
            SearchDirection::Forward => '/',
            SearchDirection::Reverse => '?',
        }
    }
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum JumpDirection {
    Next,
    Prev,
}

pub struct SearchState {
    pub direction: SearchDirection,

    pub search_term: String,
    compiled_regex: Regex,

    matches: Vec<Range<usize>>,

    immediate_state: ImmediateSearchState,
}

pub enum ImmediateSearchState {
    NotSearching,
    ActivelySearching {
        last_match_jumped_to: usize,
        last_search_into_collapsed_container: bool,
        just_wrapped: bool,
    },
}

pub type MatchRangeIter<'a> = std::slice::Iter<'a, Range<usize>>;
const STATIC_EMPTY_SLICE: &'static [Range<usize>] = &[];

impl SearchState {
    pub fn empty() -> SearchState {
        SearchState {
            direction: SearchDirection::Forward,
            search_term: "".to_owned(),
            compiled_regex: Regex::new("").unwrap(),
            matches: vec![],
            immediate_state: ImmediateSearchState::NotSearching,
        }
    }

    pub fn initialize_search(
        needle: String,
        haystack: &str,
        direction: SearchDirection,
    ) -> SearchState {
        let regex = Regex::new(&needle).unwrap();
        let matches: Vec<Range<usize>> = regex.find_iter(haystack).map(|m| m.range()).collect();

        SearchState {
            direction,
            search_term: needle,
            compiled_regex: regex,
            matches,
            immediate_state: ImmediateSearchState::NotSearching,
        }
    }

    pub fn active_search_state(&self) -> Option<(usize, bool)> {
        match self.immediate_state {
            ImmediateSearchState::NotSearching => None,
            ImmediateSearchState::ActivelySearching {
                last_match_jumped_to,
                just_wrapped,
                ..
            } => Some((last_match_jumped_to, just_wrapped)),
        }
    }

    pub fn num_matches(&self) -> usize {
        self.matches.len()
    }

    pub fn set_no_longer_actively_searching(&mut self) {
        self.immediate_state = ImmediateSearchState::NotSearching;
    }

    pub fn jump_to_match(
        &mut self,
        focused_row: Index,
        flatjson: &FlatJson,
        jump_direction: JumpDirection,
    ) -> usize {
        if self.matches.is_empty() {
            eprintln!("NEED TO HANDLE NO MATCHES");
            return 0;
        }

        let true_direction = self.true_direction(jump_direction);

        let next_match_index = self.get_next_match(focused_row, flatjson, true_direction);
        let row_containing_match = self.compute_destination_row(flatjson, next_match_index);

        // If search takes inside a collapsed object, we will show the first visible ancestor.
        let next_focused_row = flatjson.first_visible_ancestor(row_containing_match);

        let wrapped = if focused_row == next_focused_row {
            // Usually, if we end up the same place we started, that means that we
            // wrapped around because there's only a single (visible) match.
            //
            // But this can also occur if the opening of a collapsed container matches the
            // search term AND the search term appears inside the collapsed container.
            //
            // We can detect this checking if the next_match_index is different than the
            // last_jump_index.
            if let Some((last_match_index, _)) = self.active_search_state() {
                last_match_index == next_match_index
            } else {
                true
            }
        } else {
            // Otherwise wrapping depends on which direction we were going.
            match true_direction {
                SearchDirection::Forward => next_focused_row < focused_row,
                SearchDirection::Reverse => next_focused_row > focused_row,
            }
        };

        self.immediate_state = ImmediateSearchState::ActivelySearching {
            last_match_jumped_to: next_match_index,
            // We keep track of whether we searched into an object, so that
            // the next time we jump, we can jump past the collapsed container.
            last_search_into_collapsed_container: row_containing_match != next_focused_row,
            just_wrapped: wrapped,
        };

        next_focused_row
    }

    /// Return an iterator over all the stored matches. We pass in a
    /// start index that will be used to efficiently skip any matches
    /// before that index.
    pub fn matches_iter(&self, range_start: usize) -> MatchRangeIter {
        match self.immediate_state {
            ImmediateSearchState::NotSearching => STATIC_EMPTY_SLICE.iter(),
            ImmediateSearchState::ActivelySearching { .. } => {
                let search_result = self
                    .matches
                    .binary_search_by(|probe| probe.end.cmp(&range_start));
                let start_index = match search_result {
                    Ok(i) => i,
                    Err(i) => i,
                };
                self.matches[start_index..].iter()
            }
        }
    }

    fn true_direction(&self, jump_direction: JumpDirection) -> SearchDirection {
        match (self.direction, jump_direction) {
            (SearchDirection::Forward, JumpDirection::Next) => SearchDirection::Forward,
            (SearchDirection::Forward, JumpDirection::Prev) => SearchDirection::Reverse,
            (SearchDirection::Reverse, JumpDirection::Next) => SearchDirection::Reverse,
            (SearchDirection::Reverse, JumpDirection::Prev) => SearchDirection::Forward,
        }
    }

    fn get_next_match(
        &mut self,
        focused_row: Index,
        flatjson: &FlatJson,
        true_direction: SearchDirection,
    ) -> usize {
        match self.immediate_state {
            ImmediateSearchState::NotSearching => {
                let focused_row_range = flatjson[focused_row].full_range();

                match true_direction {
                    SearchDirection::Forward => {
                        // When searching forwards, we want the first match that
                        // starts _after_ (or equal) the end of focused row.
                        let next_match = self.matches.partition_point(|match_range| {
                            match_range.start <= focused_row_range.end
                        });

                        // If NONE of the matches start after the end of the focused row,
                        // parition_point returns the length of the array, but then we
                        // want to jump back to the start in that case.
                        if next_match == self.matches.len() {
                            0
                        } else {
                            next_match
                        }
                    }
                    SearchDirection::Reverse => {
                        // When searching backwards, we want the last match that
                        // ends before the start of focused row.
                        let next_match = self.matches.partition_point(|match_range| {
                            match_range.end < focused_row_range.start
                        });

                        // If the very first match ends the start of the focused row,
                        // then partition_point will return 0, and we need to wrap
                        // around to the end of the file.
                        //
                        // But otherwise, partition_point will return the first match
                        // that didn't end before the start of the focused row, so we
                        // need to subtract 1.
                        if next_match == 0 {
                            self.matches.len() - 1
                        } else {
                            next_match - 1
                        }
                    }
                }
            }
            ImmediateSearchState::ActivelySearching {
                last_match_jumped_to,
                last_search_into_collapsed_container,
                ..
            } => {
                let delta: isize = match true_direction {
                    SearchDirection::Forward => 1,
                    SearchDirection::Reverse => -1,
                };

                if last_search_into_collapsed_container {
                    let start_match = last_match_jumped_to;
                    let mut next_match = self.cycle_match(start_match, delta);

                    // Make sure we don't infinitely loop.
                    while next_match != start_match {
                        // Convert the next match to a destination row.
                        let next_destination_row =
                            self.compute_destination_row(flatjson, next_match);
                        // Get the first visible ancestor of the next destination
                        // row, and make sure it isn't the same as the row we're
                        // currently viewing. If they're different, we've broken
                        // out of the current collapsed container.
                        let next_match_visible_ancestor =
                            flatjson.first_visible_ancestor(next_destination_row);
                        if next_match_visible_ancestor != focused_row {
                            break;
                        }
                        next_match = self.cycle_match(next_match, delta);
                    }

                    next_match
                } else {
                    self.cycle_match(last_match_jumped_to, delta)
                }
            }
        }
    }

    // Helper for modifying a match_index that handles wrapping around the start or end of the
    // matches.
    fn cycle_match(&self, match_index: usize, delta: isize) -> usize {
        ((match_index + self.matches.len()) as isize + delta) as usize % self.matches.len()
    }

    fn compute_destination_row(&self, flatjson: &FlatJson, match_index: usize) -> Index {
        let match_range = &self.matches[match_index]; // [a, b)

        // We want to jump to the last row that starts before (or at) the start of the match.
        flatjson
            .0
            .partition_point(|row| row.full_range().start <= match_range.start)
            - 1
    }
}

#[cfg(test)]
mod tests {
    use crate::flatjson::parse_top_level_json2;

    use super::JumpDirection::*;
    use super::SearchDirection::*;
    use super::SearchState;

    const SEARCHABLE: &'static str = r#"{
        "1": "aaa",
        "2": [
            "3 bbb",
            "4 aaa"
        ],
        "6": {
            "7": "aaa aaa",
            "8": "ccc",
            "9": "ddd"
        },
        "11": "bbb"
    }"#;

    #[test]
    fn test_basic_search_forward() {
        let fj = parse_top_level_json2(SEARCHABLE.to_owned()).unwrap();
        let mut search = SearchState::initialize_search("aaa".to_owned(), &fj.1, Forward);
        assert_eq!(search.jump_to_match(0, &fj, Next), 1);
        assert_eq!(search.jump_to_match(1, &fj, Next), 4);
        assert_eq!(search.jump_to_match(4, &fj, Next), 7);
        assert_eq!(search.jump_to_match(7, &fj, Next), 7);
        assert_wrapped_state(&search, false);
        assert_eq!(search.jump_to_match(7, &fj, Next), 1);
        assert_wrapped_state(&search, true);
        assert_eq!(search.jump_to_match(1, &fj, Prev), 7);
        assert_wrapped_state(&search, true);
        assert_eq!(search.jump_to_match(7, &fj, Prev), 7);
        assert_wrapped_state(&search, false);
        assert_eq!(search.jump_to_match(7, &fj, Prev), 4);
        assert_eq!(search.jump_to_match(4, &fj, Prev), 1);
        assert_eq!(search.jump_to_match(1, &fj, Prev), 7);
    }

    #[test]
    fn test_basic_search_backwards() {
        let fj = parse_top_level_json2(SEARCHABLE.to_owned()).unwrap();
        let mut search = SearchState::initialize_search("aaa".to_owned(), &fj.1, Reverse);
        assert_eq!(search.jump_to_match(0, &fj, Next), 7);
        assert_wrapped_state(&search, true);
        assert_eq!(search.jump_to_match(7, &fj, Next), 7);
        assert_eq!(search.jump_to_match(7, &fj, Next), 4);
        assert_eq!(search.jump_to_match(4, &fj, Next), 1);
        assert_wrapped_state(&search, false);
        assert_eq!(search.jump_to_match(1, &fj, Prev), 4);
        assert_eq!(search.jump_to_match(4, &fj, Prev), 7);
        assert_eq!(search.jump_to_match(7, &fj, Prev), 7);
        assert_eq!(search.jump_to_match(7, &fj, Prev), 1);
        assert_wrapped_state(&search, true);
        assert_eq!(search.jump_to_match(1, &fj, Prev), 4);
        assert_wrapped_state(&search, false);
    }

    #[test]
    fn test_search_collapsed_forward() {
        let mut fj = parse_top_level_json2(SEARCHABLE.to_owned()).unwrap();
        let mut search = SearchState::initialize_search("aaa".to_owned(), &fj.1, Forward);
        fj.collapse(6);
        assert_eq!(search.jump_to_match(0, &fj, Next), 1);
        assert_eq!(search.jump_to_match(1, &fj, Next), 4);
        assert_eq!(search.jump_to_match(4, &fj, Next), 6);
        assert_eq!(search.jump_to_match(6, &fj, Next), 1);
        assert_eq!(search.jump_to_match(1, &fj, Next), 4);
        assert_eq!(search.jump_to_match(4, &fj, Prev), 1);
        assert_eq!(search.jump_to_match(1, &fj, Prev), 6);
        assert_eq!(search.jump_to_match(6, &fj, Prev), 4);
    }

    #[test]
    fn test_search_collapsed_backwards() {
        let mut fj = parse_top_level_json2(SEARCHABLE.to_owned()).unwrap();
        let mut search = SearchState::initialize_search("aaa".to_owned(), &fj.1, Reverse);
        fj.collapse(6);
        assert_eq!(search.jump_to_match(0, &fj, Next), 6);
        assert_eq!(search.jump_to_match(6, &fj, Next), 4);
        assert_eq!(search.jump_to_match(4, &fj, Next), 1);
        assert_eq!(search.jump_to_match(1, &fj, Next), 6);
        assert_eq!(search.jump_to_match(6, &fj, Prev), 1);
        assert_eq!(search.jump_to_match(1, &fj, Prev), 4);
        assert_eq!(search.jump_to_match(4, &fj, Prev), 6);
        assert_eq!(search.jump_to_match(6, &fj, Prev), 1);
    }

    #[test]
    fn test_no_wrap_when_opening_of_collapsed_container_and_contents_match_search() {
        const TEST: &'static str = r#"{
            "term": [
                "term"
            ],
            "key": "term"
        }"#;
        let mut fj = parse_top_level_json2(TEST.to_owned()).unwrap();
        let mut search = SearchState::initialize_search("term".to_owned(), &fj.1, Forward);
        fj.collapse(1);
        assert_eq!(search.jump_to_match(0, &fj, Next), 1);
        assert_wrapped_state(&search, false);
        assert_eq!(search.jump_to_match(1, &fj, Next), 1);
        assert_wrapped_state(&search, false);
        assert_eq!(search.jump_to_match(1, &fj, Next), 4);
        assert_wrapped_state(&search, false);
        assert_eq!(search.jump_to_match(4, &fj, Next), 1);
        assert_wrapped_state(&search, true);
    }

    #[track_caller]
    fn assert_wrapped_state(search: &SearchState, expected: bool) {
        if let Some((_, wrapped)) = search.active_search_state() {
            assert_eq!(wrapped, expected);
        } else {
            assert!(false, "Not in an active search state");
        }
    }
}
