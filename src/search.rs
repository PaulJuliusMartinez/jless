use regex::Regex;
use std::ops::Range;

use crate::flatjson::{FlatJson, Index};
use crate::viewer::Action;

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum SearchMode {
    // Searching for an Object key, initiated by '*' or '#'.
    ObjectKey,
    // Searching for freeform text, initiated by / or ?
    Freeform,
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum SearchDirection {
    Forward,
    Reverse,
}

#[derive(PartialEq, Eq, Debug, Copy, Clone)]
pub enum JumpDirection {
    Next,
    Prev,
}

pub struct SearchState {
    mode: SearchMode,
    direction: SearchDirection,

    search_term: String,
    compiled_regex: Regex,

    matches: Vec<Range<usize>>,

    immediate_state: ImmediateSearchState,
}

pub enum ImmediateSearchState {
    NotSearching,
    ActivelySearching {
        last_match_jumped_to: usize,
        last_search_into_collapsed_container: bool,
    },
}

impl SearchState {
    pub fn empty() -> SearchState {
        SearchState {
            mode: SearchMode::Freeform,
            direction: SearchDirection::Forward,
            search_term: "".to_owned(),
            compiled_regex: Regex::new("").unwrap(),
            matches: vec![],
            immediate_state: ImmediateSearchState::NotSearching,
        }
    }

    pub fn initialize_search(
        needle: &str,
        haystack: &str,
        mode: SearchMode,
        direction: SearchDirection,
    ) -> SearchState {
        let regex = Regex::new(needle).unwrap();
        let matches: Vec<Range<usize>> = regex.find_iter(haystack).map(|m| m.range()).collect();

        SearchState {
            mode,
            direction,
            search_term: needle.to_owned(),
            compiled_regex: regex,
            matches,
            immediate_state: ImmediateSearchState::NotSearching,
        }
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
        let destination_row = self.compute_destination_row(flatjson, next_match_index);

        // TODO: Need to make sure that destination_row is not in a collapsed container.

        destination_row
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
            } => {
                let delta: isize = match true_direction {
                    SearchDirection::Forward => 1,
                    SearchDirection::Reverse => -1,
                };

                let next_match = ((last_match_jumped_to + self.matches.len()) as isize + delta)
                    as usize
                    % self.matches.len();

                next_match
            }
        }
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
