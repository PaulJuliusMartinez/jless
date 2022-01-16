use std::fmt;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Copy, Clone)]
pub struct TruncatedStrView {
    view: Option<TruncatedRange>,
    available_space: isize,
}

#[derive(Debug, Copy, Clone)]
struct TruncatedRange {
    start: usize,
    end: usize,
    used_space: isize,
    showing_replacement_character: bool,
}

// Helper struct that implements Display for printing
// out TruncatedStrViews.
pub struct TruncatedStrSlice<'a, 'b> {
    pub s: &'a str,
    pub truncated_view: &'b TruncatedStrView,
}

impl TruncatedStrView {
    pub fn can_str_fit_at_all(s: &str, available_space: isize) -> bool {
        available_space > 0 || (available_space == 0 && s.len() == 0)
    }

    pub fn init_start(s: &str, available_space: isize) -> TruncatedStrView {
        if !Self::can_str_fit_at_all(s, available_space) {
            return Self::init_no_view(available_space);
        }

        let mut adj = Adjuster::init_start(s, available_space);
        adj.fill_right();
        adj.to_view()
    }

    pub fn init_back(s: &str, available_space: isize) -> TruncatedStrView {
        if !Self::can_str_fit_at_all(s, available_space) {
            return Self::init_no_view(available_space);
        }

        let mut adj = Adjuster::init_back(s, available_space);
        adj.fill_left();
        adj.to_view()
    }

    pub fn init_no_view(available_space: isize) -> TruncatedStrView {
        TruncatedStrView {
            view: None,
            available_space,
        }
    }

    pub fn used_space(&self) -> Option<isize> {
        match self.view {
            None => None,
            Some(TruncatedRange { used_space, .. }) => Some(used_space),
        }
    }

    fn to_adjuster<'a, 'b>(&'a self, s: &'b str) -> Adjuster<'b> {
        let TruncatedRange {
            start,
            end,
            mut used_space,
            showing_replacement_character,
        } = self.view.unwrap_or(TruncatedRange {
            start: 0,
            end: 0,
            used_space: 0,
            showing_replacement_character: false,
        });

        // The adjuster doesn't keep track of the replacement character.
        if showing_replacement_character {
            used_space -= 1;
        }

        Adjuster {
            s,
            used_space,
            available_space: self.available_space,
            start,
            end,
        }
    }

    fn scroll_right(&self, s: &str) -> TruncatedStrView {
        if self.view.is_none() {
            return self.clone();
        }

        let mut adjuster = self.to_adjuster(s);
        adjuster.expand_right();
        adjuster.shrink_left_to_fit();
        // If start == end, that means we shrunk past a multi-width
        // character, so don't fill up again, otherwise we'll completely
        // move past it, but we want to show the replacement character.
        if adjuster.start != adjuster.end {
            adjuster.fill_right();
        }
        adjuster.to_view()
    }

    fn scroll_left(&self, s: &str) -> TruncatedStrView {
        if self.view.is_none() {
            return self.clone();
        }

        let mut adjuster = self.to_adjuster(s);
        adjuster.expand_left();
        adjuster.shrink_right_to_fit();
        // If start == end, that means we shrunk past a multi-width
        // character, so don't fill up again, otherwise we'll completely
        // move past it, but we want to show the replacement character.
        if adjuster.start != adjuster.end {
            adjuster.fill_left();
        }
        adjuster.to_view()
    }

    fn jump_to_an_end(&self, s: &str) -> TruncatedStrView {
        match self.view {
            None => self.clone(),
            Some(range) => {
                if range.end < s.len() {
                    TruncatedStrView::init_back(s, self.available_space)
                } else {
                    TruncatedStrView::init_start(s, self.available_space)
                }
            }
        }
    }

    fn resize(&self, s: &str, available_space: isize) -> TruncatedStrView {
        if self.view.is_none() {
            return TruncatedStrView::init_start(s, available_space);
        }

        if available_space < self.available_space {
            if !Self::can_str_fit_at_all(s, available_space) {
                Self::init_no_view(available_space)
            } else {
                self.shrink(s, available_space)
            }
        } else if available_space > self.available_space {
            self.expand(s, available_space)
        } else {
            self.clone()
        }
    }

    fn expand(&self, s: &str, available_space: isize) -> TruncatedStrView {
        debug_assert!(available_space > self.available_space);
        let mut adjuster = self.to_adjuster(s);
        adjuster.available_space = available_space;

        // When showing a prefix, we want to fill on the right.
        // When showing a suffix, we want to fill on the left.
        // When showing the middle of the string, we want to fill
        // along the right, and then if we still have space available,
        // fill back on the left.
        //
        // Because fill_right on a suffix is a no-op, and fill_left on
        // a prefix is also a no-op, we can handle all the cases together
        // by just filling on the right, then the left.
        if adjuster.end == s.len() {
            adjuster.fill_left();
        } else {
            adjuster.fill_right();
            // Only try to then fill in the left if we reached all the way
            // to the right. Otherwise, we might not expand to the right
            // because the next character is wide, but we could expand to
            // the left.
            if adjuster.end == s.len() {
                adjuster.fill_left();
            }
        }
        adjuster.to_view()
    }

    fn shrink(&self, s: &str, available_space: isize) -> TruncatedStrView {
        debug_assert!(available_space < self.available_space);
        debug_assert!(self.view.is_some());

        // Won't be enough room for multiple ellipses and a middle character
        // so just init from the beginning (or end, if we're showing a suffix).
        if available_space < 3 {
            let TruncatedRange { start, end, .. } = self.view.unwrap();
            if start > 0 && end == s.len() {
                return Self::init_back(s, available_space);
            } else {
                return Self::init_start(s, available_space);
            }
        }

        let mut adjuster = self.to_adjuster(s);
        adjuster.available_space = available_space;

        // If we're showing a suffix of the string, shrink from the left
        // so we keep showing the end, otherwise shrink from the right.
        if adjuster.start > 0 && adjuster.end == s.len() {
            adjuster.shrink_left_to_fit();
        } else {
            adjuster.shrink_right_to_fit();
        }

        adjuster.to_view()
    }
}

// Helper struct to manage state while adjusting a TruncatedStrView.
#[derive(Debug)]
struct Adjuster<'a> {
    s: &'a str,
    used_space: isize,
    available_space: isize,

    start: usize,
    end: usize,
}

impl<'a> Adjuster<'a> {
    pub fn init_start(s: &'a str, available_space: isize) -> Self {
        Adjuster::init_at_index(s, available_space, 0)
    }

    pub fn init_back(s: &'a str, available_space: isize) -> Self {
        Adjuster::init_at_index(s, available_space, s.len())
    }

    pub fn init_at_index(s: &'a str, available_space: isize, index: usize) -> Self {
        let mut space_for_ellipses = 0;
        if index > 0 {
            // We have a leading ellipsis;
            space_for_ellipses += 1;
        }
        if index < s.len() {
            // We have a trailing ellipsis;
            space_for_ellipses += 1;
        }

        Adjuster {
            s,
            used_space: space_for_ellipses,
            available_space,
            start: index,
            end: index,
        }
    }

    pub fn expand_right(&mut self) {
        let mut right_graphemes = self.s[self.end..].graphemes(true);
        if let Some(grapheme) = right_graphemes.next() {
            self.end += grapheme.len();
            self.used_space += UnicodeWidthStr::width(grapheme) as isize;
            if self.end == self.s.len() {
                // No more trailing ellipsis.
                self.used_space -= 1;
            }
        }
    }

    pub fn expand_left(&mut self) {
        let mut left_graphemes = self.s[..self.start].graphemes(true);
        if let Some(grapheme) = left_graphemes.next_back() {
            self.start -= grapheme.len();
            self.used_space += UnicodeWidthStr::width(grapheme) as isize;
            if self.start == 0 {
                // No more leading ellipsis.
                self.used_space -= 1;
            }
        }
    }

    pub fn fill_right(&mut self) {
        let mut right_graphemes = self.s[self.end..].graphemes(true);
        while let Some(grapheme) = right_graphemes.next() {
            let new_end = self.end + grapheme.len();
            let mut new_used_space = self.used_space + UnicodeWidthStr::width(grapheme) as isize;

            if new_end == self.s.len() {
                // No more trailing ellipsis.
                new_used_space -= 1;
            }

            if new_used_space > self.available_space {
                break;
            }

            self.end = new_end;
            self.used_space = new_used_space;
        }
    }

    pub fn fill_left(&mut self) {
        let mut left_graphemes = self.s[..self.start].graphemes(true);
        while let Some(grapheme) = left_graphemes.next_back() {
            let new_start = self.start - grapheme.len();
            let mut new_used_space = self.used_space + UnicodeWidthStr::width(grapheme) as isize;

            if new_start == 0 {
                // No more leading ellipsis.
                new_used_space -= 1;
            }

            if new_used_space > self.available_space {
                break;
            }

            self.start = new_start;
            self.used_space = new_used_space;
        }
    }

    pub fn shrink_right_to_fit(&mut self) {
        let mut visible_graphemes = self.s[self.start..self.end].graphemes(true);
        while self.used_space > self.available_space {
            let rightmost_grapheme = visible_graphemes.next_back().unwrap();
            if self.end == self.s.len() {
                // Add trailing ellipsis.
                self.used_space += 1;
            }
            self.end -= rightmost_grapheme.len();
            self.used_space -= UnicodeWidthStr::width(rightmost_grapheme) as isize;
        }
    }

    pub fn shrink_left_to_fit(&mut self) {
        let mut visible_graphemes = self.s[self.start..self.end].graphemes(true);
        while self.used_space > self.available_space {
            let leftmost_grapheme = visible_graphemes.next().unwrap();
            if self.start == 0 {
                // Add leading ellipsis.
                self.used_space += 1;
            }
            self.start += leftmost_grapheme.len();
            self.used_space -= UnicodeWidthStr::width(leftmost_grapheme) as isize;
        }
    }

    pub fn to_view(&self) -> TruncatedStrView {
        if self.available_space < 0 || (self.available_space == 0 && self.s.len() > 0) {
            panic!("adjuster used when string can't fit");
            // TruncatedStrView {
            //     view: None,
            //     available_space: self.available_space,
            // }
        } else {
            let showing_replacement_character =
                // We only show a repacement character if we're not
                // showing anything at all...
                self.start == self.end &&
                    // But we have room to showing something...
                    self.available_space > 1 &&
                        // And there's something to show.
                        self.s.len() > 0;

            let mut used_space = self.used_space;
            if showing_replacement_character {
                if used_space >= self.available_space {
                    dbg!(&self);
                }
                debug_assert!(dbg!(used_space) < dbg!(self.available_space));
                used_space += 1;
            };

            TruncatedStrView {
                view: Some(TruncatedRange {
                    start: self.start,
                    end: self.end,
                    used_space,
                    showing_replacement_character,
                }),
                available_space: self.available_space,
            }
        }
    }
}

impl<'a, 'b> fmt::Display for TruncatedStrSlice<'a, 'b> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.truncated_view.view.is_none() {
            return Ok(());
        }

        let TruncatedRange {
            start,
            end,
            showing_replacement_character,
            ..
        } = self.truncated_view.view.unwrap();

        if start != 0 {
            f.write_str("…")?;
        }

        if showing_replacement_character {
            f.write_str("�")?;
        }

        f.write_str(&self.s[start..end])?;

        if end != self.s.len() {
            f.write_str("…")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rendered(s: &str, truncated_view: &TruncatedStrView) -> String {
        format!("{}", TruncatedStrSlice { s, truncated_view })
    }

    #[test]
    fn test_init_start_and_init_back() {
        #[track_caller]
        fn assert_init_start(string: &str, space: isize, front: &str, used_space: Option<isize>) {
            let init_state = TruncatedStrView::init_start(string, space);
            assert_eq!(front, rendered(string, &init_state), "incorrect prefix");
            assert_eq!(
                used_space,
                init_state.used_space(),
                "incorrect prefix width"
            );
        }

        #[track_caller]
        fn assert_init_back(string: &str, space: isize, back: &str, used_space: Option<isize>) {
            let init_state = TruncatedStrView::init_back(string, space);
            assert_eq!(back, rendered(string, &init_state), "incorrect suffix");
            assert_eq!(
                used_space,
                init_state.used_space(),
                "incorrect suffix width"
            );
        }

        #[track_caller]
        fn assert_init_states(
            string: &str,
            space: isize,
            front: &str,
            back: &str,
            used_space: Option<isize>,
        ) {
            assert_init_start(string, space, front, used_space);
            assert_init_back(string, space, back, used_space);
        }

        assert_init_states("abcde", -1, "", "", None);

        assert_init_states("abcde", 0, "", "", None);
        assert_init_states("", 0, "", "", Some(0));

        assert_init_states("a", 1, "a", "a", Some(1));
        assert_init_states("abc", 1, "…", "…", Some(1));

        assert_init_states("abc", 2, "a…", "…c", Some(2));
        assert_init_states("ab", 2, "ab", "ab", Some(2));

        assert_init_states("🦀abc", 2, "�…", "…c", Some(2));
        assert_init_states("abc🦀", 2, "a…", "…�", Some(2));

        assert_init_states("abc", 3, "abc", "abc", Some(3));
        assert_init_states("abcd", 3, "ab…", "…cd", Some(3));

        assert_init_states("🦀🦀abc🦀🦀", 3, "🦀…", "…🦀", Some(3));
        assert_init_states("🦀🦀abc🦀🦀", 5, "🦀🦀…", "…🦀🦀", Some(5));

        // Since we're showing a normal character, these don't use the
        // replacment, so the lengths are different for front vs. back.
        assert_init_start("a🦀bc", 3, "a…", Some(2));
        assert_init_back("a🦀bc", 3, "…bc", Some(3));

        assert_init_start("ab🦀c", 3, "ab…", Some(3));
        assert_init_back("ab🦀c", 3, "…c", Some(2));
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
    fn assert_scroll_states(s: &str, available_space: isize, states: Vec<&str>) {
        let mut curr_state = TruncatedStrView::init_start(s, available_space);
        let mut prev_formatted = rendered(s, &curr_state);
        assert_eq!(states[0], prev_formatted);

        for expected_state in states.iter().skip(1) {
            let next_state = curr_state.scroll_right(s);
            let formatted = rendered(s, &next_state);

            assert_eq!(
                expected_state, &formatted,
                "expected scroll_right({}) to be {}",
                &prev_formatted, &expected_state,
            );
            curr_state = next_state;
            prev_formatted = formatted;
        }

        let mut curr_state = TruncatedStrView::init_back(s, available_space);
        let mut prev_formatted = rendered(s, &curr_state);
        assert_eq!(states.last().unwrap(), &prev_formatted);

        for expected_state in states.iter().rev().skip(1) {
            let next_state = curr_state.scroll_left(s);
            let formatted = rendered(s, &next_state);

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
            TruncatedStrView::init_start(s, 5),
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

        let initial_state = TruncatedStrView::init_start(s, 5)
            .scroll_right(s)
            .scroll_right(s);

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
            TruncatedStrView::init_start(s, 5),
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
            TruncatedStrView::init_start(s, 5)
                .scroll_right(s)
                .scroll_right(s),
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
        initial_state: TruncatedStrView,
        mut available_space: isize,
        states: Vec<&str>,
    ) {
        let mut curr_state = initial_state;
        let mut prev_formatted = rendered(string, &curr_state);
        assert_eq!(states[0], prev_formatted);

        for expansion in states.iter().skip(1) {
            available_space += 1;
            let next_state = curr_state.expand(string, available_space);
            let formatted = rendered(string, &next_state);

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
            TruncatedStrView::init_start(s, 10),
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
            TruncatedStrView::init_start(s, 9).scroll_right(s),
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
            TruncatedStrView::init_start(s, 8).scroll_right(s),
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
            TruncatedStrView::init_start(s, 11).scroll_right(s),
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
            TruncatedStrView::init_start(s, 5),
            5,
            vec!["🦀abc", "🦀a…", "🦀…", "�…", "…"],
        );

        let s = "abc🦀";
        assert_shrinks(
            s,
            TruncatedStrView::init_back(s, 4),
            4,
            vec!["…c🦀", "…🦀", "…�", "…"],
        );
    }

    #[track_caller]
    fn assert_shrinks(
        string: &str,
        initial_state: TruncatedStrView,
        mut available_space: isize,
        states: Vec<&str>,
    ) {
        let mut curr_state = initial_state;
        let mut prev_formatted = rendered(string, &curr_state);
        assert_eq!(states[0], prev_formatted);

        for expansion in states.iter().skip(1) {
            available_space -= 1;
            let next_state = curr_state.shrink(string, available_space);
            let formatted = rendered(string, &next_state);

            assert_eq!(
                expansion, &formatted,
                "expected shrink({}) to be {}",
                &prev_formatted, &expansion,
            );

            curr_state = next_state;
            prev_formatted = formatted;
        }
    }
}
