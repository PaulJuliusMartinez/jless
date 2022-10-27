use std::cmp::Ordering;
use std::fmt;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

/// This module provides functionality for truncating strings,
/// displaying them, and manipulating which portion of the string
/// is visible.

/// A TruncatedStrView represents an attempt to fit a string within
/// a given amount of available space. When `range` is None, it
/// signifies that the string cannot be represented at all in the
/// given space.
///
/// If the available space is negative, no string can be represented.
/// If there is 0 available space, then only an empty string can be
/// represented. Above that, every string can be represented with
/// ellipses representing truncated content.
///
/// When the string is representable, the view will also track how
/// much space the view takes up, including ellipses.
#[derive(Debug, Copy, Clone)]
pub struct TruncatedStrView {
    pub range: Option<TruncatedRange>,
    available_space: isize,
}

/// A TruncatedRange is a range intended to represent a slice of
/// a string, along with some additional metadata that is useful
/// when manipulating a range.
///
/// The visible portion of the string is represented by the
/// range [start..end).
///
/// When we are representing a string in just 2 or 3 columns,
/// wide characters are unrepresentable. For example, while we can
/// represent the prefix of the string "abc🦀" in just two columns
/// ("a…"), we cannot represent the suffix because the '🦀'
/// character is two columns wide, so we wouldn't have room for the
/// ellipsis.
///
/// In this situation we choose to display the Unicode replacement
/// character: '�', which is to be used for unrepresentable
/// characters.
///
/// A similar situation occurs when a wide character appears in the
/// middle of a string and we have just 3 columns to display the
/// character.
///
/// Since showing a replacement character is less than ideal, when
/// we are showing other characters, we will opt for not using the
/// entire available space, rather than including the replacement
/// character.
///
/// This range also keeps track of how much space it takes up.
#[derive(Debug, Copy, Clone)]
pub struct TruncatedRange {
    pub start: usize,
    pub end: usize,
    pub showing_replacement_character: bool,
    used_space: isize,
}

/// A TruncatedStrView doesn't keep a reference to the `str` it
/// is intended to display.
///
/// This is a helper struct used to actually generate the printed
/// intended representation of the TruncatedStrView.
pub struct TruncatedStrSlice<'a, 'b> {
    pub s: &'a str,
    pub truncated_view: &'b TruncatedStrView,
}

// When manipulating a TruncatedStrView, we use this helper struct
// to keep track of state and maintain a reference to the string
// the view is representing.
//
// Note that the RangeAdjuster does *not* keep track of whether it is
// showing a replacement character. We can determine whether or
// not to display a replacement character when we decide we want to
// "materialize" the RangeAdjuster into a TruncatedStrView based on
// whether the visible portion is "empty" and there is space to
// display a replacement character.
#[derive(Clone, Debug)]
struct RangeAdjuster<'a> {
    s: &'a str,
    used_space: isize,
    available_space: isize,

    start: usize,
    end: usize,
}

impl TruncatedRange {
    // Create a RangeAdjuster representing the current state of the
    // TruncatedRange.
    fn adjuster<'a, 'b>(&'a self, s: &'b str, available_space: isize) -> RangeAdjuster<'b> {
        let mut used_space = self.used_space;
        // The adjuster doesn't keep track of the replacement character.
        if self.showing_replacement_character {
            used_space -= 1;
        }

        RangeAdjuster {
            s,
            used_space,
            available_space,
            start: self.start,
            end: self.end,
        }
    }

    /// Check whether this is a view of a string that is totally elided,
    /// that is, it is represented by a single ellipsis.
    pub fn is_completely_elided(&self) -> bool {
        self.used_space == 1 && self.start == self.end
    }

    /// Check whether this is a truncated view of a string.
    pub fn is_truncated(&self, s: &str) -> bool {
        self.start != 0 || self.end != s.len() || self.showing_replacement_character
    }

    pub fn print_leading_ellipsis(&self) -> bool {
        self.start != 0
    }

    pub fn print_trailing_ellipsis(&self, s: &str) -> bool {
        self.end != s.len()
    }
}

impl TruncatedStrView {
    /// If we have a least one column, we can always represent a string
    /// by at least an ellipsis that represents the entirety of the string.
    ///
    /// If we have exactly 0 columns, then only an empty string can be
    /// represented. (We take the conservative approach here and ignore
    /// the possibility of strings solely consisting of zero-width spaces
    /// and other zero-width Unicode oddities.)
    ///
    /// The `range` of a TruncatedStrView should be present if and only if
    /// this function, when called on the string the TruncatedStrView is
    /// associated with, returns true.
    pub fn can_str_fit_at_all(s: &str, available_space: isize) -> bool {
        available_space > 0 || (available_space == 0 && s.is_empty())
    }

    /// Create a truncated view of a string that shows the beginning of
    /// the string and elides the end if there is not sufficient space.
    pub fn init_start(s: &str, available_space: isize) -> TruncatedStrView {
        if !Self::can_str_fit_at_all(s, available_space) {
            return Self::init_no_view(available_space);
        }

        let mut adj = RangeAdjuster::init_start(s, available_space);
        adj.fill_right();
        adj.to_view()
    }

    /// Create a truncated view of a string that shows the end of the
    /// string and elides the beginning if there is not sufficient space.
    pub fn init_back(s: &str, available_space: isize) -> TruncatedStrView {
        if !Self::can_str_fit_at_all(s, available_space) {
            return Self::init_no_view(available_space);
        }

        let mut adj = RangeAdjuster::init_back(s, available_space);
        adj.fill_left();
        adj.to_view()
    }

    // Create a TruncatedStrView that indicates that the string cannot
    // be represented in the available space.
    fn init_no_view(available_space: isize) -> TruncatedStrView {
        TruncatedStrView {
            range: None,
            available_space,
        }
    }

    /// Return the amount of space used by a string view, if the string
    /// is representable.
    pub fn used_space(&self) -> Option<isize> {
        self.range
            .map(|TruncatedRange { used_space, .. }| used_space)
    }

    /// Check whether this is a view of a string that is totally elided,
    /// that is, it is represented by a single ellipsis.
    pub fn is_completely_elided(&self) -> bool {
        self.range.map_or(false, |r| r.is_completely_elided())
    }

    /// Check whether this is a view of a string that fits in the available
    /// space and shows at least one character (i.e., isn't totally elided).
    pub fn any_contents_visible(&self) -> bool {
        self.range.map_or(false, |r| !r.is_completely_elided())
    }

    // Creates a RangeAdjuster that represents the current state of
    // the TruncatedStrView. This should only be called when the string
    // is representable and we have a view.
    fn range_adjuster<'a, 'b>(&'a self, s: &'b str) -> RangeAdjuster<'b> {
        debug_assert!(self.range.is_some());
        self.range.unwrap().adjuster(s, self.available_space)
    }

    /// Scrolls a string view to the right by at least the specified
    /// number of characters (unless the end of the string is reached).
    pub fn scroll_right(&self, s: &str, count: usize) -> TruncatedStrView {
        if self.range.is_none() {
            return *self;
        }

        // If we only have two columns, we can't represent the middle
        // of the string, so when we scroll right we'll just jump to
        // the end.
        if self.available_space <= 2 {
            return Self::init_back(s, self.available_space);
        }

        let mut adjuster = self.range_adjuster(s);
        // Show another character on the right.
        adjuster.expand_right(count);
        // Shrink from the left to fit in available space again.
        adjuster.shrink_left_to_fit();
        // Since we might have gotten rid of a wide character on
        // the left, we might still have space to fill, so let's
        // expand on the right more.
        //
        // But, if start == end, that means we expanded to show
        // a multi-width character, and then shrunk past it because
        // there wasn't enough space. In this case, don't fill because
        // then we'll just skip that character. We actually want to show
        // a replacement character in this case.
        if adjuster.start != adjuster.end {
            adjuster.fill_right();
        }
        adjuster.to_view()
    }

    /// Scrolls a string view to the left by at least the specified
    /// number of characters (unless the start of the string is reached).
    pub fn scroll_left(&self, s: &str, count: usize) -> TruncatedStrView {
        if self.range.is_none() {
            return *self;
        }

        // If we only have two columns, we can't represent the middle
        // of the string, so when we scroll left we'll just jump to
        // the start.
        if self.available_space <= 2 {
            return Self::init_start(s, self.available_space);
        }

        let mut adjuster = self.range_adjuster(s);
        // Show another character on the left.
        adjuster.expand_left(count);
        // Shrink from the right to fit in available space again.
        adjuster.shrink_right_to_fit();
        // Since we might have gotten rid of a wide character on
        // the right, we might still have space to fill, so let's
        // expand on the left more.
        //
        // But, if start == end, that means we expanded to show
        // a multi-width character, and then shrunk past it because
        // there wasn't enough space. In this case, don't fill because
        // then we'll just skip that character. We actually want to show
        // a replacement character in this case.
        if adjuster.start != adjuster.end {
            adjuster.fill_left();
        }
        adjuster.to_view()
    }

    /// Jump from whatever portion of the string is currently represented
    /// to showing either the start or the end of the string.
    ///
    /// Normally we will always jump to the back of the string, unless
    /// we are already showing the back of the string, in which case we
    /// will jump to the front.
    pub fn jump_to_an_end(&self, s: &str) -> TruncatedStrView {
        match self.range {
            None => *self,
            Some(range) => {
                if range.end < s.len() {
                    TruncatedStrView::init_back(s, self.available_space)
                } else {
                    TruncatedStrView::init_start(s, self.available_space)
                }
            }
        }
    }

    /// Update the string view with a new amount of available space.
    pub fn resize(&self, s: &str, available_space: isize) -> TruncatedStrView {
        if self.range.is_none() {
            return TruncatedStrView::init_start(s, available_space);
        }

        match available_space.cmp(&self.available_space) {
            Ordering::Less => {
                if !Self::can_str_fit_at_all(s, available_space) {
                    Self::init_no_view(available_space)
                } else {
                    self.shrink(s, available_space)
                }
            }
            Ordering::Greater => self.expand(s, available_space),
            Ordering::Equal => *self,
        }
    }

    /// Expand a view to fit into more available space.
    fn expand(&self, s: &str, available_space: isize) -> TruncatedStrView {
        debug_assert!(available_space > self.available_space);
        let mut adjuster = self.range_adjuster(s);
        adjuster.available_space = available_space;

        // When showing a prefix, we want to fill on the right:
        //   a… -> abc…
        // When showing a suffix, we want to fill on the left:
        //   …z -> …xyz
        // When showing the middle of the string, we want to fill
        // along the right, and then if we still have space available,
        // fill back on the left:
        //   …m… -> …mno…
        //   …x… -> …wxyz
        if adjuster.end == s.len() {
            adjuster.fill_left();
        } else {
            adjuster.fill_right();
            // Only try to then fill in the left if we reached all the way
            // to the right. Otherwise, we might not expand to the right
            // because the next character is wide, but we could expand to
            // the left. Without this we'd do something like this:
            //
            // s = "a👍b👀c😱d"
            //
            // When expanding "…👀c…" by one column, we can't add the '😱',
            // but we don't to expand on the left and show "…b👀c…".
            if adjuster.end == s.len() {
                adjuster.fill_left();
            }
        }
        adjuster.to_view()
    }

    /// Shrink a view to fit into less available space.
    fn shrink(&self, s: &str, available_space: isize) -> TruncatedStrView {
        debug_assert!(available_space < self.available_space);
        debug_assert!(self.range.is_some());

        // Won't be enough room for multiple ellipses and a middle character
        // so just init from the beginning (or end, if we're showing a suffix).
        if available_space < 3 {
            let TruncatedRange { start, end, .. } = self.range.unwrap();
            if start > 0 && end == s.len() {
                return Self::init_back(s, available_space);
            } else {
                return Self::init_start(s, available_space);
            }
        }

        let mut adjuster = self.range_adjuster(s);
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

    /// Scroll a view so that a particular subrange is shown.
    pub fn focus(&self, s: &str, range: &Range<usize>) -> TruncatedStrView {
        if self.range.is_none() {
            return *self;
        }

        let Range { mut start, mut end } = *range;

        // Make sure our start isn't in-between a character boundary.
        while start != 0 && !s.is_char_boundary(start) {
            start -= 1;
        }
        end = end.min(s.len());

        let visible_range = self.range.unwrap();

        // If the entire match is already visible, don't do anything.
        if visible_range.start <= start && end <= visible_range.end {
            return *self;
        }

        // But otherwise, we'll just jump to the match and try to center it.
        let mut adjuster = RangeAdjuster::init_at_index(s, self.available_space, start);

        // Make sure to include entire match if possible.
        while adjuster.end < end && adjuster.used_space < self.available_space {
            adjuster.expand_right(1);
        }

        // Then fill from both sides to keep it centered.
        adjuster.fill_from_both_sides();

        adjuster.to_view()
    }
}

impl<'a> RangeAdjuster<'a> {
    /// Initialize a RangeAdjuster at the beginning of a string, but is
    /// not showing any part of the string.
    pub fn init_start(s: &'a str, available_space: isize) -> Self {
        RangeAdjuster::init_at_index(s, available_space, 0)
    }

    /// Initialize a RangeAdjuster at the end of a string, but is not showing
    /// any part of the string.
    pub fn init_back(s: &'a str, available_space: isize) -> Self {
        RangeAdjuster::init_at_index(s, available_space, s.len())
    }

    /// Initialize a RangeAdjuster at an arbitrary spot in a string, but
    /// is not showing any part of the string.
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

        RangeAdjuster {
            s,
            used_space: space_for_ellipses,
            available_space,
            start: index,
            end: index,
        }
    }

    /// Update the range to show another character on the right side.
    pub fn expand_right(&mut self, count: usize) {
        let mut right_graphemes = self.s[self.end..].graphemes(true);
        for _ in 0..count {
            if let Some(grapheme) = right_graphemes.next() {
                self.end += grapheme.len();
                self.used_space += UnicodeWidthStr::width(grapheme) as isize;
                if self.end == self.s.len() {
                    // No more trailing ellipsis.
                    self.used_space -= 1;
                }
            } else {
                break;
            }
        }
    }

    /// Update the range to show another character on the left side.
    pub fn expand_left(&mut self, count: usize) {
        let mut left_graphemes = self.s[..self.start].graphemes(true);
        for _ in 0..count {
            if let Some(grapheme) = left_graphemes.next_back() {
                self.start -= grapheme.len();
                self.used_space += UnicodeWidthStr::width(grapheme) as isize;
                if self.start == 0 {
                    // No more leading ellipsis.
                    self.used_space -= 1;
                }
            } else {
                break;
            }
        }
    }

    /// Add as many characters to the right side of the string as we
    /// can without exceeding the available space.
    pub fn fill_right(&mut self) {
        let right_graphemes = self.s[self.end..].graphemes(true);
        // Note that we should consider the next grapheme even if we
        // have already used up all the available space, because the
        // next grapheme might be the end of the string, and we'd no
        // longer have to show the ellipsis.
        //
        // This allows converting "…xy…" to "…xyz".
        for grapheme in right_graphemes {
            if !self.add_grapheme_to_right_if_it_will_fit(grapheme) {
                break;
            }
        }
    }

    // Adds a grapheme to the right side of a view if it will fit.
    fn add_grapheme_to_right_if_it_will_fit(&mut self, grapheme: &str) -> bool {
        let new_end = self.end + grapheme.len();
        let mut new_used_space = self.used_space + UnicodeWidthStr::width(grapheme) as isize;

        if new_end == self.s.len() {
            // No more trailing ellipsis.
            new_used_space -= 1;
        }

        if new_used_space > self.available_space {
            return false;
        }

        self.end = new_end;
        self.used_space = new_used_space;

        true
    }

    /// Add as many characters to the left side of the string as we
    /// can without exceeding the available space.
    pub fn fill_left(&mut self) {
        let mut left_graphemes = self.s[..self.start].graphemes(true);
        // Note that we should consider the previous grapheme even if
        // we have already used up all the available space, because
        // the previous grapheme might be the start of the string, and
        // we'd no longer have to show the ellipsis.
        //
        // This allows converting "…bc…" to "abc…".
        while let Some(grapheme) = left_graphemes.next_back() {
            if !self.add_grapheme_to_left_if_it_will_fit(grapheme) {
                break;
            }
        }
    }

    // Adds a grapheme to the left side of a view if it will fit.
    fn add_grapheme_to_left_if_it_will_fit(&mut self, grapheme: &str) -> bool {
        let new_start = self.start - grapheme.len();
        let mut new_used_space = self.used_space + UnicodeWidthStr::width(grapheme) as isize;

        if new_start == 0 {
            // No more leading ellipsis.
            new_used_space -= 1;
        }

        if new_used_space > self.available_space {
            return false;
        }

        self.start = new_start;
        self.used_space = new_used_space;

        true
    }

    /// Add as many characters to each side of the string, so that
    /// the initial visible portion remains centered.
    pub fn fill_from_both_sides(&mut self) {
        let mut left_graphemes = self.s[..self.start].graphemes(true);
        let mut right_graphemes = self.s[self.end..].graphemes(true);

        let mut width_added_to_left = 0;
        let mut width_added_to_right = 0;

        let mut more_on_left = true;
        let mut more_on_right = true;

        // Need to try to expand even when used_space == available_space
        // to possible consume ellipses.
        while self.used_space <= self.available_space {
            let mut added_to_left = false;
            let mut added_to_right = false;

            // Add to right first
            while !more_on_left || width_added_to_right <= width_added_to_left {
                if let Some(grapheme) = right_graphemes.next() {
                    let used_space_before = self.used_space;
                    if !self.add_grapheme_to_right_if_it_will_fit(grapheme) {
                        more_on_right = false;
                        break;
                    }
                    width_added_to_right += self.used_space - used_space_before;
                    added_to_right = true;
                } else {
                    more_on_right = false;
                    break;
                }
            }

            while !more_on_right || width_added_to_left < width_added_to_right {
                if let Some(grapheme) = left_graphemes.next_back() {
                    let used_space_before = self.used_space;
                    if !self.add_grapheme_to_left_if_it_will_fit(grapheme) {
                        more_on_left = false;
                        break;
                    }
                    width_added_to_left += self.used_space - used_space_before;
                    added_to_left = true;
                } else {
                    more_on_left = false;
                    break;
                }
            }

            if !added_to_right && !added_to_left {
                break;
            }
        }
    }

    /// Remove characters from the right side of the range until the
    /// amount of used space is within the available space.
    pub fn shrink_right_to_fit(&mut self) {
        let mut visible_graphemes = self.s[self.start..self.end].graphemes(true);
        while self.used_space > self.available_space {
            debug_assert!(self.start < self.end);
            let rightmost_grapheme = visible_graphemes.next_back().unwrap();
            if self.end == self.s.len() {
                // Add trailing ellipsis.
                self.used_space += 1;
            }
            self.end -= rightmost_grapheme.len();
            self.used_space -= UnicodeWidthStr::width(rightmost_grapheme) as isize;
        }
    }

    /// Remove characters from the left side of the range until the
    /// amount of used space is within the available space.
    pub fn shrink_left_to_fit(&mut self) {
        let mut visible_graphemes = self.s[self.start..self.end].graphemes(true);
        while self.used_space > self.available_space {
            debug_assert!(self.start < self.end);
            let leftmost_grapheme = visible_graphemes.next().unwrap();
            if self.start == 0 {
                // Add leading ellipsis.
                self.used_space += 1;
            }
            self.start += leftmost_grapheme.len();
            self.used_space -= UnicodeWidthStr::width(leftmost_grapheme) as isize;
        }
    }

    /// Convert a RangeAdjuster into a TruncatedStrView.
    pub fn to_view(&self) -> TruncatedStrView {
        debug_assert!(TruncatedStrView::can_str_fit_at_all(
            self.s,
            self.available_space
        ));

        // This DOESN'T consider the possibility that using a
        // replacement character would remove the need for an
        // ellipsis (because it's the last character), so
        // something like "🦀" is represented as "…", not "�".
        let showing_replacement_character =
            // We only show a replacement character if we're not
            // showing anything at all...
            self.start == self.end &&
                // But we have room to showing something...
                self.available_space > 1 &&
                    // And there's something to show.
                    !self.s.is_empty();

        let mut used_space = self.used_space;
        if showing_replacement_character {
            debug_assert!(used_space < self.available_space);
            used_space += 1;
        };

        TruncatedStrView {
            range: Some(TruncatedRange {
                start: self.start,
                end: self.end,
                showing_replacement_character,
                used_space,
            }),
            available_space: self.available_space,
        }
    }
}

impl<'a, 'b> fmt::Display for TruncatedStrSlice<'a, 'b> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.truncated_view.range.is_none() {
            return Ok(());
        }

        let TruncatedRange {
            start,
            end,
            showing_replacement_character,
            ..
        } = self.truncated_view.range.unwrap();

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
        // Note comment in to_view to understand why
        // this is a single ellipsis instead of a
        // replacement character.
        assert_init_states("🦀", 1, "…", "…", Some(1));

        assert_init_states("abc", 2, "a…", "…c", Some(2));
        assert_init_states("ab", 2, "ab", "ab", Some(2));

        assert_init_states("🦀abc", 2, "�…", "…c", Some(2));
        assert_init_states("abc🦀", 2, "a…", "…�", Some(2));

        assert_init_states("abc", 3, "abc", "abc", Some(3));
        assert_init_states("abcd", 3, "ab…", "…cd", Some(3));

        assert_init_states("🦀🦀abc🦀🦀", 3, "🦀…", "…🦀", Some(3));
        assert_init_states("🦀🦀abc🦀🦀", 5, "🦀🦀…", "…🦀🦀", Some(5));

        // Since we're showing a normal character, these don't use the
        // replacement, so the lengths are different for front vs. back.
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

        let s = "🦀z";
        assert_scroll_states(s, 2, vec!["�…", "…z"]);

        let s = "a🦀";
        assert_scroll_states(s, 2, vec!["a…", "…�"]);
    }

    #[track_caller]
    fn assert_scroll_states(s: &str, available_space: isize, states: Vec<&str>) {
        let mut curr_state = TruncatedStrView::init_start(s, available_space);
        let mut prev_formatted = rendered(s, &curr_state);
        assert_eq!(states[0], prev_formatted);

        for expected_state in states.iter().skip(1) {
            let next_state = curr_state.scroll_right(s, 1);
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
            let next_state = curr_state.scroll_left(s, 1);
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

        let initial_state = TruncatedStrView::init_start(s, 5).scroll_right(s, 2);

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
            TruncatedStrView::init_start(s, 5).scroll_right(s, 2),
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
            TruncatedStrView::init_start(s, 9).scroll_right(s, 1),
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
            TruncatedStrView::init_start(s, 8).scroll_right(s, 1),
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
            TruncatedStrView::init_start(s, 11).scroll_right(s, 1),
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

        for shrunk in states.iter().skip(1) {
            available_space -= 1;
            let next_state = curr_state.shrink(string, available_space);
            let formatted = rendered(string, &next_state);

            assert_eq!(
                shrunk, &formatted,
                "expected shrink({}) to be {}",
                &prev_formatted, &shrunk,
            );

            curr_state = next_state;
            prev_formatted = formatted;
        }
    }

    #[test]
    fn test_focus() {
        let s = "0123456789";
        let tsv = TruncatedStrView::init_start(s, 10);

        assert_focuses(
            s,
            tsv,
            vec![
                (&(0..1), "0123456789"),
                (&(4..7), "0123456789"),
                (&(8..12), "0123456789"),
            ],
        );

        let s = "0123456789abc";
        // "…34567…"
        let tsv = TruncatedStrView::init_start(s, 7).scroll_right(s, 2);

        assert_focuses(
            s,
            tsv,
            vec![
                // Focus range if not visible.
                (&(0..1), "012345…"),
                // Don't move if entire range is visible.
                (&(3..4), "…34567…"),
                (&(3..8), "…34567…"),
                (&(7..8), "…34567…"),
                // Center focused value if not all visible.
                (&(6..9), "…56789…"),
                // Start at beginning if can't fit whole focused range.
                (&(2..9), "…23456…"),
                // Focus range if not visible.
                (&(10..15), "…789abc"),
            ],
        );
    }

    #[track_caller]
    fn assert_focuses(
        string: &str,
        initial_state: TruncatedStrView,
        ranges_and_expected: Vec<(&Range<usize>, &str)>,
    ) {
        let initial_formatted = rendered(string, &initial_state);

        for (i, (range, expected_focused)) in ranges_and_expected.into_iter().enumerate() {
            let focused = initial_state.focus(string, range);
            let formatted = rendered(string, &focused);

            assert_eq!(
                expected_focused, &formatted,
                "Case {}: expected focus({}, {}..{}) to be {}",
                i, &initial_formatted, range.start, range.end, &expected_focused,
            );
        }
    }
}
