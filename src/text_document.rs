use std::cmp::{self, Ordering};
use std::ops::Range;
use std::rc::Rc;

use crate::document::{ContentRange, Document};

// Precalculated break points when displaying a long line. Each values represents
// the starting byte offset of one line.
// Someday: Maybe save info about whether the range contains non-ASCII, or
// escaped characters.
#[derive(Debug)]
struct BreakPoints(Vec<usize>);

#[derive(Clone, Debug)]
struct SegmentOfWrappedLine {
    break_points: Rc<BreakPoints>,
    index: usize,
    width: usize,
}

impl SegmentOfWrappedLine {
    fn is_start(&self) -> bool {
        self.index == 0
    }

    fn is_end(&self) -> bool {
        self.index == self.break_points.len() - 1
    }

    fn is_after_start(&self) -> bool {
        self.index > 0
    }

    fn is_before_end(&self) -> bool {
        self.index < self.break_points.len() - 1
    }

    fn next_segment(&self) -> Option<SegmentOfWrappedLine> {
        if self.is_before_end() {
            Some(SegmentOfWrappedLine {
                index: self.index + 1,
                ..self.clone()
            })
        } else {
            None
        }
    }

    fn prev_segment(&self) -> Option<SegmentOfWrappedLine> {
        if self.is_after_start() {
            Some(SegmentOfWrappedLine {
                index: self.index - 1,
                ..self.clone()
            })
        } else {
            None
        }
    }

    fn into_last(mut self) -> SegmentOfWrappedLine {
        self.index = self.break_points.len() - 1;
        self
    }

    fn make_last(&self) -> SegmentOfWrappedLine {
        self.clone().into_last()
    }
}

impl BreakPoints {
    // Someday: Handle control characters, UTF-8, etc.
    fn calculate(bytes: &[u8], width: usize) -> Option<BreakPoints> {
        let len = bytes.len();
        if len <= width {
            return None;
        }

        let mut offsets = vec![];
        let mut curr_offset = 0;
        while curr_offset < len {
            offsets.push(curr_offset);
            curr_offset += width;
        }

        Some(BreakPoints(offsets))
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn nth_segment<'a, 'b>(&'a self, bytes: &'b [u8], n: usize) -> &'b [u8] {
        let start = self.0[n];
        if n + 1 < self.0.len() {
            let end = self.0[n + 1];
            &bytes[start..end]
        } else {
            &bytes[start..]
        }
    }
}

#[derive(Clone, Debug)]
pub struct ScreenLine {
    line_index: usize,
    // If we have to wrap the line, precalculated line breaks, and the specific line wrap
    // we're showing.
    segment_of_wrapped_line: Option<SegmentOfWrappedLine>,
}

impl PartialEq for ScreenLine {
    fn eq(&self, other: &Self) -> bool {
        if self.line_index != other.line_index {
            return false;
        }

        match (
            &self.segment_of_wrapped_line,
            &other.segment_of_wrapped_line,
        ) {
            (None, None) => true,
            (None, Some(_)) => false,
            (Some(_), None) => false,
            (Some(segment1), Some(segment2)) => {
                segment1.width == segment2.width && segment1.index == segment2.index
            }
        }
    }
}

impl Eq for ScreenLine {}

impl Ord for ScreenLine {
    fn cmp(&self, other: &Self) -> Ordering {
        match self.line_index.cmp(&other.line_index) {
            Ordering::Less => return Ordering::Less,
            Ordering::Greater => return Ordering::Greater,
            Ordering::Equal => (),
        }

        match (
            &self.segment_of_wrapped_line,
            &other.segment_of_wrapped_line,
        ) {
            (None, None) => Ordering::Equal,
            (Some(segment1), Some(segment2)) => {
                if segment1.width == segment2.width {
                    segment1.index.cmp(&segment2.index)
                } else {
                    panic!(
                        "Two TextDocument::ScreenLines were wrapped with different lengths: \
                            line {}, width1: {}, width2: {}",
                        self.line_index, segment1.width, segment2.width,
                    );
                }
            }
            (None, Some(_)) | (Some(_), None) => {
                panic!(
                    "Two TextDocument::ScreenLines point to the same line ({}), but only one is wrapped",
                    self.line_index,
                );
            }
        }
    }
}

impl PartialOrd for ScreenLine {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

// Cursor is just a plain line number
pub type Cursor = usize;

pub struct TextDocument {
    data: Vec<u8>,
    complete_line_ranges: Vec<Range<usize>>,
    // Someday: It's awkward to have both `next_start` and `trailing_newline`. `next_start`
    // becomes invalid once `eof` is called and we set `trailing_newline` to `Some`.
    next_start: usize,
    trailing_newline: Option<bool>,
    width: usize,
}

impl TextDocument {
    fn num_lines(&self) -> usize {
        self.complete_line_ranges.len()
    }

    fn line_zero_indexed(&self, n: usize) -> &[u8] {
        &self.data[self.complete_line_ranges[n].clone()]
    }

    fn trailing_newline(&self) -> Option<bool> {
        self.trailing_newline
    }

    pub fn cursor_to_line_n(&self, n: usize) -> Cursor {
        n - 1
    }

    fn create_ref_to_start_of_line(&self, line_index: usize) -> ScreenLine {
        let line = self.line_zero_indexed(line_index);
        let segment_of_wrapped_line = match BreakPoints::calculate(line, self.width) {
            None => None,
            Some(break_points) => Some(SegmentOfWrappedLine {
                break_points: Rc::new(break_points),
                index: 0,
                width: self.width,
            }),
        };

        ScreenLine {
            line_index,
            segment_of_wrapped_line,
        }
    }

    fn create_ref_to_end_of_line(&self, line_index: usize) -> ScreenLine {
        let start_of_line = self.create_ref_to_start_of_line(line_index);
        match start_of_line.segment_of_wrapped_line {
            None => start_of_line,
            Some(start_segment) => ScreenLine {
                segment_of_wrapped_line: Some(start_segment.into_last()),
                ..start_of_line
            },
        }
    }

    fn screen_line_contents(&self, screen_line: &ScreenLine) -> &[u8] {
        let full_line = self.line_zero_indexed(screen_line.line_index);
        match &screen_line.segment_of_wrapped_line {
            None => full_line,
            Some(SegmentOfWrappedLine {
                break_points,
                index,
                width: _,
            }) => break_points.nth_segment(full_line, *index),
        }
    }
}

impl Document for TextDocument {
    type ScreenLine = ScreenLine;
    type Cursor = Cursor;

    fn new(width: usize) -> Self {
        TextDocument {
            data: vec![],
            complete_line_ranges: vec![],
            next_start: 0,
            trailing_newline: None,
            width,
        }
    }

    fn width(&self) -> usize {
        self.width
    }

    fn resize(&mut self, new_width: usize) {
        self.width = new_width;
    }

    fn append(&mut self, data: &[u8]) {
        let len = self.data.len();
        self.data.extend_from_slice(data);

        for newline_offset in memchr::memchr_iter(b'\n', data) {
            let end = len + newline_offset;
            let line_range = if end > 0 && self.data[end - 1] == b'\r' {
                self.next_start..(end - 1)
            } else {
                self.next_start..end
            };

            self.complete_line_ranges.push(line_range);
            self.next_start = len + newline_offset + 1;
        }
    }

    fn eof(&mut self) {
        let end = self.data.len();
        if end > self.next_start {
            self.trailing_newline = Some(false);
            self.complete_line_ranges.push(self.next_start..end);
        } else {
            self.trailing_newline = Some(true);
        }
    }

    fn top_screen_line_and_cursor(&self) -> Option<(ScreenLine, Cursor)> {
        if self.complete_line_ranges.is_empty() {
            return None;
        }

        let screen_line = self.create_ref_to_start_of_line(0);
        let cursor = 0;
        Some((screen_line, cursor))
    }

    fn bottom_screen_line_and_cursor(&self) -> Option<(ScreenLine, Cursor)> {
        if self.complete_line_ranges.is_empty() {
            return None;
        }

        let last_line = self.complete_line_ranges.len() - 1;
        let screen_line = self.create_ref_to_end_of_line(last_line);
        let cursor = last_line;
        Some((screen_line, cursor))
    }

    fn next_screen_line(&self, screen_line: &ScreenLine) -> Option<ScreenLine> {
        let num_lines = self.num_lines();
        let next_line_index = screen_line.line_index + 1;

        match &screen_line.segment_of_wrapped_line {
            None => {
                if next_line_index == num_lines {
                    None
                } else {
                    Some(self.create_ref_to_start_of_line(next_line_index))
                }
            }
            Some(segment_of_wrapped_line) => match segment_of_wrapped_line.next_segment() {
                Some(next_segment) => Some(ScreenLine {
                    line_index: screen_line.line_index,
                    segment_of_wrapped_line: Some(next_segment),
                }),
                None => {
                    if next_line_index == num_lines {
                        None
                    } else {
                        Some(self.create_ref_to_start_of_line(next_line_index))
                    }
                }
            },
        }
    }

    fn prev_screen_line(&self, screen_line: &ScreenLine) -> Option<ScreenLine> {
        let prev_line_index = screen_line.line_index.saturating_sub(1);

        match &screen_line.segment_of_wrapped_line {
            None => {
                if screen_line.line_index == 0 {
                    None
                } else {
                    Some(self.create_ref_to_end_of_line(prev_line_index))
                }
            }
            Some(segment_of_wrapped_line) => match segment_of_wrapped_line.prev_segment() {
                Some(prev_segment) => Some(ScreenLine {
                    line_index: screen_line.line_index,
                    segment_of_wrapped_line: Some(prev_segment),
                }),
                None => {
                    if screen_line.line_index == 0 {
                        None
                    } else {
                        Some(self.create_ref_to_end_of_line(prev_line_index))
                    }
                }
            },
        }
    }

    fn line_number(&self, screen_line: &ScreenLine) -> usize {
        screen_line.line_index + 1
    }

    fn is_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        screen_line.segment_of_wrapped_line.is_some()
    }

    fn is_start_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        screen_line
            .segment_of_wrapped_line
            .as_ref()
            .map_or(false, SegmentOfWrappedLine::is_start)
    }

    fn is_end_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        screen_line
            .segment_of_wrapped_line
            .as_ref()
            .map_or(false, SegmentOfWrappedLine::is_end)
    }

    fn is_after_start_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        screen_line
            .segment_of_wrapped_line
            .as_ref()
            .map_or(false, SegmentOfWrappedLine::is_after_start)
    }

    fn is_before_end_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        screen_line
            .segment_of_wrapped_line
            .as_ref()
            .map_or(false, SegmentOfWrappedLine::is_before_end)
    }

    fn cursor_range(&self, cursor: &Cursor) -> ContentRange<ScreenLine> {
        let start = self.create_ref_to_start_of_line(*cursor);
        let end = match &start.segment_of_wrapped_line {
            None => start.clone(),
            Some(segment_of_wrapped_line) => ScreenLine {
                line_index: start.line_index,
                segment_of_wrapped_line: Some(segment_of_wrapped_line.make_last()),
            },
        };

        let num_screen_lines = match &start.segment_of_wrapped_line {
            None => 1,
            Some(SegmentOfWrappedLine { break_points, .. }) => break_points.len(),
        };

        ContentRange {
            start,
            end,
            num_screen_lines,
        }
    }

    fn convert_screen_line_to_cursor(
        &self,
        screen_line: Self::ScreenLine,
        _prev_cursor: &Self::Cursor,
    ) -> Self::Cursor {
        screen_line.line_index
    }

    // Actions

    fn move_cursor_down(&mut self, lines: usize, cursor: &Cursor) -> Option<Cursor> {
        let max_line = self.num_lines() - 1;
        if *cursor == max_line {
            None
        } else {
            Some(cmp::min(*cursor + lines, max_line))
        }
    }

    fn move_cursor_up(&mut self, lines: usize, cursor: &Cursor) -> Option<Cursor> {
        if *cursor == 0 {
            None
        } else {
            Some(cursor.saturating_sub(lines))
        }
    }

    fn expand_or_move_cursor_right_or_down(&mut self, _cursor: &Cursor) -> Option<Cursor> {
        None
    }

    fn collapse_or_move_cursor_left_or_up(&mut self, _cursor: &Cursor) -> Option<Cursor> {
        None
    }

    // Soon: Uncomment this.
    // #[cfg(test)]
    fn debug_text_content(&self, screen_line: &Self::ScreenLine, _cursor: &Cursor) -> Vec<u8> {
        self.screen_line_contents(screen_line).to_vec()
    }

    fn raw_bytes_for_searching(&self) -> &[u8] {
        // This indicates whether we're going to accept any more input or not.
        if self.trailing_newline.is_some() {
            &self.data
        } else {
            &self.data[..self.next_start]
        }
    }

    fn raw_byte_range_of_cursor(&self, cursor: &Cursor) -> Range<usize> {
        self.complete_line_ranges[*cursor].clone()
    }

    fn raw_byte_index_to_cursor(&self, index: usize) -> Cursor {
        assert!(index < self.raw_bytes_for_searching().len());

        let first_line_starting_after_index = self
            .complete_line_ranges
            .partition_point(|line_range| line_range.start <= index);

        first_line_starting_after_index - 1
    }

    fn visible_ancestor(&self, cursor: &Cursor) -> Cursor {
        // All lines are always visible
        *cursor
    }
}

#[cfg(test)]

mod tests {
    use super::*;

    use bstr::ByteSlice;
    use insta::assert_snapshot;

    use std::fmt::Write;

    fn print_lines(doc: &TextDocument) -> String {
        let mut s = String::new();
        for n in 0..doc.num_lines() {
            writeln!(s, "{}: {:?}", n + 1, doc.line_zero_indexed(n).as_bstr()).unwrap();
        }
        s
    }

    #[test]
    fn test_text_document() {
        let mut doc = TextDocument::new(10);
        assert_snapshot!(print_lines(&doc), @"");

        doc.append(b"abc");
        assert_snapshot!(print_lines(&doc), @"");

        doc.append(b"def\n");
        assert_snapshot!(print_lines(&doc), @r#"1: "abcdef""#);

        doc.append(b"ghi\n\njkl");
        assert_snapshot!(print_lines(&doc), @r#"
        1: "abcdef"
        2: "ghi"
        3: ""
        "#);
        assert_eq!(None, doc.trailing_newline());

        doc.eof();
        assert_snapshot!(print_lines(&doc), @r#"
        1: "abcdef"
        2: "ghi"
        3: ""
        4: "jkl"
        "#);
        assert_eq!(Some(false), doc.trailing_newline());
    }

    #[test]
    fn test_leading_and_trailing_newline() {
        let mut doc = TextDocument::new(10);
        doc.append(b"\nabc\n");
        assert_snapshot!(print_lines(&doc), @r#"
        1: ""
        2: "abc"
        "#);

        doc.eof();
        assert_snapshot!(print_lines(&doc), @r#"
        1: ""
        2: "abc"
        "#);
        assert_eq!(Some(true), doc.trailing_newline());
    }

    #[test]
    fn test_crlf_line_endings() {
        let mut doc = TextDocument::new(10);
        doc.append(b"abc\r\n");
        doc.append(b"def\r");
        doc.append(b"\nghi\r");
        doc.eof();
        assert_snapshot!(print_lines(&doc), @r#"
        1: "abc"
        2: "def"
        3: "ghi\r"
        "#);
        assert_eq!(Some(false), doc.trailing_newline());
    }

    fn print_screen_lines(doc: &TextDocument) -> String {
        let mut s = String::new();
        let mut screen_line = Some(doc.top_screen_line_and_cursor().unwrap().0);
        let width = doc.width();

        while let Some(line) = &screen_line {
            write!(
                s,
                "|{: <width$}",
                doc.screen_line_contents(line).as_bstr(),
                width = width,
            )
            .unwrap();
            writeln!(s, "|").unwrap();
            let next_screen_line = doc.next_screen_line(line);
            if let Some(next_screen_line) = &next_screen_line {
                assert_eq!(screen_line, doc.prev_screen_line(next_screen_line));
            }
            screen_line = next_screen_line
        }

        s
    }

    #[test]
    fn test_next_and_prev_line() {
        let mut doc = TextDocument::new(20);
        doc.append(b"line.1\n");
        //           0123456789012345
        doc.append(b"long.long.line.2\n");
        doc.append(b"line.3\n");

        assert_snapshot!(print_screen_lines(&doc), @r"
        |line.1              |
        |long.long.line.2    |
        |line.3              |
        ");

        doc.resize(16);
        assert_snapshot!(print_screen_lines(&doc), @r"
        |line.1          |
        |long.long.line.2|
        |line.3          |
        ");

        doc.resize(15);
        assert_snapshot!(print_screen_lines(&doc), @r"
        |line.1         |
        |long.long.line.|
        |2              |
        |line.3         |
        ");

        doc.resize(4);
        assert_snapshot!(print_screen_lines(&doc), @r"
        |line|
        |.1  |
        |long|
        |.lon|
        |g.li|
        |ne.2|
        |line|
        |.3  |
        ");
    }
}
