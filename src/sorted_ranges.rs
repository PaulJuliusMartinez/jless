use std::ops::Range;

// The `partition_point` function can be used to effectively perform binary search through
// a slice, but it can be tricky to use. Depending on exactly which element you're looking
// for, it may also require inverting the condition you're using, or subtracting 1 from
// the result. It also returns the length of the input in some cases where returning an
// `Option` may be more natural.
//
// From the docs of `partition_point`:
//
// > Returns the index of the partition point according to the given predicate (the index of
// > the first element of the second partition).
//
// > The slice is assumed to be partitioned according to the given predicate. This means that
// > all elements for which the predicate returns true are at the start of the slice and all
// > elements for which the predicate returns false are at the end.
//
// i.e., if running the predicate returns [true, true, true, false, false], then
// `partition_point` will return 3.
//
// Also:
// > If all elements of the slice match the predicate, including if the slice is empty, then
// > the length of the slice will be returned

// We first define a couple helpers:

// If `pred(elem)` returns true, it must return true for all elements after `elem` as well.
fn index_of_first<E, P>(elems: &[E], mut pred: P) -> Option<usize>
where
    P: FnMut(&E) -> bool,
{
    // P returns false at first, and then starts returning true. That's the opposite of what
    // `partition_point` expects, so we can just invert it.
    let index = elems.partition_point(|elem| !pred(elem));
    if index == elems.len() {
        None
    } else {
        Some(index)
    }
}

// If `pred(elem)` returns true, it must return true for all elements before `elem` as well.
fn index_of_last<E, P>(elems: &[E], pred: P) -> Option<usize>
where
    P: FnMut(&E) -> bool,
{
    // Giving the predicate to `partition_point` will return the first value where it's false, so
    // we want to go back one index. (If it returns 0, then no elements matched `pred`, and
    // we'll return `None`.)
    let index = elems.partition_point(pred);
    if index == 0 {
        None
    } else {
        Some(index - 1)
    }
}

// See documentation of `index_of_first_elem_overlapping` for the definition of "overlap".
fn overlap(r1: &Range<usize>, r2: &Range<usize>) -> bool {
    // r1: { r1.start } ∪ [r1.start, r1.end)
    // r2: { r2.start } ∪ [r2.start, r2.end)
    //
    // r1 ∩ r2 =
    //     ({ r1.start }       ∩ { r2.start }      )
    //   ∪ ({ r1.start }       ∩ [r2.start, r2.end))
    //   ∪ ([r1.start, r1.end) ∩ { r2.start }      )
    //   ∪ ([r1.start, r1.end) ∩ [r2.start, r2.end))
    //
    // `range.contains(x)` returns true if `range.start <= x && x < range.end`.

    // We don't need to check the fourth condition; if it is true, then one of
    // the two middle conditions must be true.
    r1.start == r2.start || r1.contains(&r2.start) || r2.contains(&r1.start)
}

/// A helper trait that provides functions on sorted lists of non-overlapping
/// (but possibly zero-size) ranges.
pub trait SortedRanges {
    type Elem;

    fn elems(&self) -> &[Self::Elem];

    fn elem(&self, index: usize) -> &Self::Elem {
        &self.elems()[index]
    }

    fn elem_start(elem: &Self::Elem) -> usize;
    fn elem_end(elem: &Self::Elem) -> usize;

    /// Returns the index of the elem where `elem.start <= index && index < elem.end`, or `None`
    /// if no such element exists.
    fn index_of_elem_containing(&self, index: usize) -> Option<usize> {
        let elem_index = self.index_of_first_elem_ending_after(index)?;

        // We know `index < elem.end`, now we need to check if `elem.start <= index`.
        if Self::elem_start(self.elem(elem_index)) <= index {
            Some(elem_index)
        } else {
            None
        }
    }

    /// Returns the first index of an elem that overlaps with the given range, or `None` if
    /// no elems overlap with the given range.
    ///
    /// What does it mean to "overlap"? We want something that will support empty ranges,
    /// so if we pass in the range `10..20`, and there's an element `15..15`, we want to
    /// return that.
    ///
    /// If we interpret `a..b` not just as the half-open range `[a, b)` but instead as `{a} ∪ [a, b)`,
    /// and say two elements "overlap" if these corresponding sets have a non-empty intersection,
    /// then that gives us a more intuitive notion of overlapping:
    ///
    /// ```text
    ///           0     1     2     3     4     5
    ///        |_____|_____|_____|_____|_____|_____|
    /// 0..1:  |_____
    /// 1..4:        |_____|_____|_____|_____
    /// 1..1:        |
    /// 2..2:              |
    /// 4..4:                                |
    /// 4..5:                                |_____
    /// ```
    ///
    /// So we can see that `1..1` and `2..2` overlap with `1..4`, but `4..4` does not.
    fn index_of_first_elem_overlapping(&self, range: &Range<usize>) -> Option<usize> {
        // If an elem ends at or before `range.start`, it definitely doesn't intersect, so
        // we find the first one that ends after `range.start`. If it intersects range, it will
        // be the first intersection.
        //
        // If that elem starts after
        // the range, and the elems just skipped over the range, and there's no intersection.
        // but if the elem starts before the end, then max(elem.start, range.start) is
        // guaranteed to be in both ranges, so we've found an intersection.
        // We found what we're looking for. Otherwise, its start
        match self.index_of_first_elem_ending_after(range.start) {
            None => None,
            Some(elem_index) => {
                let elem = self.elem(elem_index);
                let elem_range = Self::elem_start(elem)..Self::elem_end(elem);
                if overlap(range, &elem_range) {
                    Some(elem_index)
                } else {
                    None
                }
            }
        }
    }

    /// Returns the index of the first elem where `index <= elem.start`, or `None` if no such
    /// element exists.
    fn index_of_first_elem_starting_at_or_after(&self, index: usize) -> Option<usize> {
        index_of_first(self.elems(), |elem| index <= Self::elem_start(elem))
    }

    /// Returns the index of the first elem where `index < elem.end`, or `None if no such
    /// element exists.
    fn index_of_first_elem_ending_after(&self, index: usize) -> Option<usize> {
        index_of_first(self.elems(), |elem| index < Self::elem_end(elem))
    }

    /// Returns the index of the last elem where `elem.start <= index`, or `None` if no such
    /// element exists.
    fn index_of_last_elem_starting_at_or_before(&self, index: usize) -> Option<usize> {
        index_of_last(self.elems(), |elem| Self::elem_start(elem) <= index)
    }

    /// Returns the index of the last elem where `elem.end <= index`, or `None` if no such element
    /// exists. Note `<=` and not `<`, because the ranges are half-open and the end is not included
    /// in the elem.
    fn index_of_last_elem_ending_at_or_before(&self, index: usize) -> Option<usize> {
        index_of_last(self.elems(), |elem| Self::elem_end(elem) <= index)
    }
}

impl SortedRanges for [Range<usize>] {
    type Elem = Range<usize>;

    fn elems(&self) -> &[Self::Elem] {
        self
    }

    fn elem_start(elem: &Self::Elem) -> usize {
        elem.start
    }

    fn elem_end(elem: &Self::Elem) -> usize {
        elem.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_overlap() {
        assert!(overlap(&(1..4), &(0..2)));
        assert!(overlap(&(1..4), &(2..5)));
        assert!(overlap(&(1..4), &(1..1)));
        assert!(overlap(&(1..4), &(2..2)));
        assert!(!overlap(&(1..4), &(4..4)));
        assert!(!overlap(&(1..4), &(0..1)));
        assert!(!overlap(&(1..4), &(4..5)));

        assert!(overlap(&(0..2), &(1..4)));
        assert!(overlap(&(2..5), &(1..4)));
        assert!(overlap(&(1..1), &(1..4)));
        assert!(overlap(&(2..2), &(1..4)));
        assert!(!overlap(&(4..4), &(1..4)));
        assert!(!overlap(&(0..1), &(1..4)));
        assert!(!overlap(&(4..5), &(1..4)));
    }

    #[test]
    fn test_find_elem_containing() {
        let ranges = vec![5..10, 12..12, 15..20, 20..20, 20..30];

        assert_eq!(ranges.index_of_elem_containing(0), None);
        assert_eq!(ranges.index_of_elem_containing(5), Some(0));
        assert_eq!(ranges.index_of_elem_containing(10), None);
        assert_eq!(ranges.index_of_elem_containing(12), None);
        assert_eq!(ranges.index_of_elem_containing(17), Some(2));
        assert_eq!(ranges.index_of_elem_containing(20), Some(4));
        assert_eq!(ranges.index_of_elem_containing(30), None);
        assert_eq!(ranges.index_of_elem_containing(40), None);
    }

    fn test_find_overlapping() {
        let ranges = vec![5..10, 12..15, 15..15, 15..18, 20..20, 20..25];

        assert_eq!(ranges.index_of_first_elem_overlapping(&(0..30)), Some(0));
        assert_eq!(ranges.index_of_first_elem_overlapping(&(0..5)), None);
        assert_eq!(ranges.index_of_first_elem_overlapping(&(7..20)), Some(0));
        assert_eq!(ranges.index_of_first_elem_overlapping(&(10..12)), None);
        assert_eq!(ranges.index_of_first_elem_overlapping(&(10..20)), Some(1));
        assert_eq!(ranges.index_of_first_elem_overlapping(&(15..30)), Some(2));
        assert_eq!(ranges.index_of_first_elem_overlapping(&(16..30)), Some(3));
        assert_eq!(ranges.index_of_first_elem_overlapping(&(20..30)), Some(4));
        assert_eq!(ranges.index_of_first_elem_overlapping(&(30..40)), None);
    }

    #[test]
    fn test_find_first_last_elems() {
        let ranges = vec![5..10, 15..20, 20..20, 20..20, 20..30];

        assert_eq!(ranges.index_of_first_elem_starting_at_or_after(0), Some(0));
        assert_eq!(ranges.index_of_first_elem_starting_at_or_after(5), Some(0));
        assert_eq!(ranges.index_of_first_elem_starting_at_or_after(8), Some(1));
        assert_eq!(ranges.index_of_first_elem_starting_at_or_after(12), Some(1));
        assert_eq!(ranges.index_of_first_elem_starting_at_or_after(20), Some(2));
        assert_eq!(ranges.index_of_first_elem_starting_at_or_after(21), None);

        assert_eq!(ranges.index_of_first_elem_ending_after(5), Some(0));
        assert_eq!(ranges.index_of_first_elem_ending_after(9), Some(0));
        assert_eq!(ranges.index_of_first_elem_ending_after(10), Some(1));
        assert_eq!(ranges.index_of_first_elem_ending_after(20), Some(4));
        assert_eq!(ranges.index_of_first_elem_ending_after(30), None);
        assert_eq!(ranges.index_of_first_elem_ending_after(31), None);

        assert_eq!(ranges.index_of_last_elem_starting_at_or_before(0), None);
        assert_eq!(ranges.index_of_last_elem_starting_at_or_before(4), None);
        assert_eq!(ranges.index_of_last_elem_starting_at_or_before(5), Some(0));
        assert_eq!(ranges.index_of_last_elem_starting_at_or_before(15), Some(1));
        assert_eq!(ranges.index_of_last_elem_starting_at_or_before(20), Some(4));
        assert_eq!(ranges.index_of_last_elem_starting_at_or_before(40), Some(4));

        assert_eq!(ranges.index_of_last_elem_ending_at_or_before(0), None);
        assert_eq!(ranges.index_of_last_elem_ending_at_or_before(5), None);
        assert_eq!(ranges.index_of_last_elem_ending_at_or_before(8), None);
        assert_eq!(ranges.index_of_last_elem_ending_at_or_before(10), Some(0));
        assert_eq!(ranges.index_of_last_elem_ending_at_or_before(20), Some(3));
        assert_eq!(ranges.index_of_last_elem_ending_at_or_before(30), Some(4));
        assert_eq!(ranges.index_of_last_elem_ending_at_or_before(50), Some(4));
    }
}
