use std::collections::BTreeMap;
use std::iter::DoubleEndedIterator;

use crate::document::{CursorRange, Document};
use crate::sexp::core::{AtomKind, DocCore, DocumentToken, ListKind, ListMetadata, NodeIndex};
use crate::sexp::layout::{self as layout, LogicalLine};

use ocaml_sexplib::input::InputRef;
use ocaml_sexplib::tokenizer::{BasicTapeTokenizer, RawTokenTape};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CollapseState {
    Collapsed,
    Expanded,
}

use CollapseState::*;

pub struct SexpDocument {
    width: usize,
    tokenizer: BasicTapeTokenizer,
    core: DocCore,
    next_top_level_node_index: NodeIndex,
    starts_of_logical_lines: BTreeMap<NodeIndex, (NodeIndex, usize)>,
    collapsible_nodes: BTreeMap<NodeIndex, CollapseState>,
}

impl SexpDocument {
    fn process_additional_data(&mut self, current_data: Option<&[u8]>) {
        while let Some(witness) = self.tokenizer.has_enough_data_to_produce_tokens() {
            let current_data = current_data.map(|b| InputRef::Transient(b));
            match self.tokenizer.next_raw_token(witness, current_data) {
                Ok(Some(raw_token)) => self.core.append_raw_token(raw_token),
                Ok(None) => {
                    println!("Got to EOF!");
                    self.core.append_eof();
                    break;
                }
                Err(err) => unimplemented!("TODO: appending errors to sexp::core::DocCore"),
            }
        }

        // Add data any new top level nodes.
        let Some(last_completed_top_level_sexp) =
            self.core.node_index_of_last_completed_top_level_sexp
        else {
            // No data yet
            return;
        };

        while self.next_top_level_node_index <= last_completed_top_level_sexp {
            let logical_lines =
                layout::layout_fully_expanded_node(&self.core, self.next_top_level_node_index);

            for LogicalLine {
                indentation,
                start_index,
                end_index,
            } in logical_lines.iter()
            {
                self.starts_of_logical_lines
                    .insert(*start_index, (*end_index, *indentation));
            }

            self.next_top_level_node_index = logical_lines.last().unwrap().end_index + 1;

            for logical_line in logical_lines.iter() {
                // For a given LogicalLine, we can collapse some of the content underneath it if
                // we have a container (i.e., a record, variant tuple/record, or a plain list)
                // that starts on that line, but does not _end_ on that line.
                //
                // If something is wrapped in a singleton, then we'll end up with two "containers"
                // that both start on the same line, but then both end together on a different line.
                // It doesn't make sense to be able to collapse _both_ of these, so we'll say the
                // first one is collapsible (so that "more" content, in terms of more parentheses,
                // get collapsed).

                // We'll keep track of where the previous collapsible node ended so we can do this
                // comparison. Use NodeIndex(0), because that's impossible, and avoids an awkward
                // option comparison later.
                let mut start_index_of_end_line_of_previous_collapsible_node = NodeIndex(0);

                for node_index in logical_line.node_indexes() {
                    let token = self.core.token(node_index);

                    let Some(list_end_index) = token.list_end_index() else {
                        continue;
                    };

                    // If the list ends on the same line, we can stop here and stop processing
                    // additional nodes in this line, because they are nested inside and will
                    // also end on this line.
                    if list_end_index <= logical_line.end_index {
                        break;
                    }

                    // DO need to check list kind here; RecordField might span multiple lines,
                    // but is not collapsible.
                    match token.list_kind().unwrap() {
                        // Not collapsible:
                        ListKind::DateTime | ListKind::RecordField => continue,
                        // Definitely collapsible:
                        ListKind::Record
                        | ListKind::VariantRecord
                        | ListKind::VariantTuple
                        | ListKind::Plain
                        // Technically collapsible if they end up across multiple lines
                        | ListKind::Singleton
                        | ListKind::Unit => (),
                    }

                    let start_of_end_logical_line =
                        self.logical_line_of_node_index(list_end_index).start_index;

                    if start_of_end_logical_line
                        != start_index_of_end_line_of_previous_collapsible_node
                    {
                        self.collapsible_nodes.insert(node_index, Expanded);
                        start_index_of_end_line_of_previous_collapsible_node =
                            start_of_end_logical_line;
                    }
                }
            }
        }
    }

    fn maybe_logical_line_of_node_index(&self, node_index: NodeIndex) -> Option<LogicalLine> {
        let mut range = self
            .starts_of_logical_lines
            .range(NodeIndex(0)..=node_index);
        match range.next_back() {
            None => None,
            Some((start_index, (end_index, indentation))) => {
                if node_index <= *end_index {
                    Some(LogicalLine {
                        indentation: *indentation,
                        start_index: *start_index,
                        end_index: *end_index,
                    })
                } else {
                    None
                }
            }
        }
    }

    fn logical_line_of_node_index(&self, node_index: NodeIndex) -> LogicalLine {
        self.maybe_logical_line_of_node_index(node_index).unwrap()
    }

    fn collapsible_nodes_in_line<'a, 'b>(
        &'a self,
        logical_line: &'b LogicalLine,
    ) -> impl DoubleEndedIterator<Item = (&'a NodeIndex, &'a CollapseState)> {
        self.collapsible_nodes
            .range(logical_line.start_index..=logical_line.end_index)
    }

    // Assumes that the logical line passed in is itself visible (i.e., if a parent node is
    // collapsed, this may not return an actually visible node, but if all the parents _were_
    // expanded, then it would the correct thing).
    fn next_visible_logical_line(&self, logical_line: &LogicalLine) -> Option<LogicalLine> {
        // If something is collapsed, then the next visible line is the line immediately
        // after the end of that collapsed node. If a line contains multiple collapsed nodes, it's
        // the line after the end of first collapsed node (and thus later in doc).
        for (node_index, collapsed_state) in self.collapsible_nodes_in_line(logical_line) {
            match collapsed_state {
                Expanded => continue,
                Collapsed => {
                    let list_end_index = self.core.token(*node_index).list_end_index().unwrap();
                    let end_line = self.logical_line_of_node_index(list_end_index);
                    return self.maybe_logical_line_of_node_index(end_line.end_index + 1);
                }
            }
        }

        // If nothing is collapsed, then it's just the next logical line.
        self.maybe_logical_line_of_node_index(logical_line.end_index + 1)
    }

    fn prev_visible_logical_line(&self, logical_line: &LogicalLine) -> Option<LogicalLine> {
        if logical_line.start_index == NodeIndex(0) {
            return None;
        }
        // Imagine we are on 'h' in the below sexp, and we want the previous logical line.
        // It may be easier to think about it if we explode each of the closing parens
        // on their own line:
        //                               first record collapsed:    d) record collapsed:
        // (a              (a            (a                         (a
        //  ((b 1)          ((b 1)        (... (d (... (g 3))))      ((b 1)
        //   (c 2)           (c 2)        h)                          (c 2)
        //   (d (            (d (                                     (d (... (g 3))))
        //     (e 1)           (e 1)                                 h)
        //     (f 2)           (f 2)
        //                     (g 3
        //                     )
        //                    )
        //                   )
        //     (g 3))))     )
        //  h)
        //
        //  Starting from the bottom (i.e., last paren on previous line), if any of those
        //  containers are collapsed, then moving up will take us to the start of that container.
        //  There can't be another an earlier container in that line that is collapsed, because
        //  then it would have ended at our line or later (by the rules of choosing collapsible
        //  nodes). And by the same logic, there isn't some other even-earlier thing that is
        //  collapsed, because it would have to contain our starting line too, but we're assuming
        //  that it's visible.
        //
        //  If none of the ends of containers in the prevous line are collapsed, then the previous
        //  visible line is really just the previous line.

        let previous_logical_line = self.logical_line_of_node_index(logical_line.start_index - 1);

        for end_node_index in previous_logical_line.node_indexes().rev() {
            let Some(list_start_index) = self.core.token(end_node_index).list_start_index() else {
                // Since all the closing parens go at the end, once we see a non-end of list,
                // then we can stop checking.
                break;
            };

            match self.collapsible_nodes.get(&list_start_index) {
                None | Some(Expanded) => continue,
                Some(Collapsed) => return Some(self.logical_line_of_node_index(list_start_index)),
            }
        }

        Some(previous_logical_line)
    }
}

impl Document for SexpDocument {
    type Cursor = NodeIndex;
    type ScreenLine = LogicalLine;

    fn new(width: usize) -> Self {
        SexpDocument {
            width,
            tokenizer: BasicTapeTokenizer::new(),
            core: DocCore::new(),
            next_top_level_node_index: NodeIndex(0),
            starts_of_logical_lines: BTreeMap::new(),
            collapsible_nodes: BTreeMap::new(),
        }
    }

    fn width(&self) -> usize {
        self.width
    }

    fn resize(&mut self, new_width: usize) {
        self.width = new_width;
        // TODO: Handle resizing
    }

    fn append(&mut self, data: &[u8]) {
        self.tokenizer.feed_more_data(data);
        self.process_additional_data(Some(data));
    }

    fn eof(&mut self) {
        self.tokenizer.eof();
        self.process_additional_data(None);
    }

    fn top_screen_line_and_cursor(&self) -> Option<(Self::ScreenLine, Self::Cursor)> {
        match self.starts_of_logical_lines.first_key_value() {
            None => None,
            Some((start_index, (end_index, indentation))) => Some((
                LogicalLine {
                    indentation: *indentation,
                    start_index: *start_index,
                    end_index: *end_index,
                },
                *start_index,
            )),
        }
    }

    fn bottom_screen_line_and_cursor(&self) -> Option<(Self::ScreenLine, Self::Cursor)> {
        match self.starts_of_logical_lines.last_key_value() {
            None => None,
            Some((start_index, (end_index, indentation))) => Some((
                LogicalLine {
                    indentation: *indentation,
                    start_index: *start_index,
                    end_index: *end_index,
                },
                *start_index,
            )),
        }
    }

    fn next_screen_line(&self, logical_line: &Self::ScreenLine) -> Option<Self::ScreenLine> {
        let next_visible_logical_line = self.next_visible_logical_line(logical_line);
        next_visible_logical_line
    }

    fn prev_screen_line(&self, logical_line: &Self::ScreenLine) -> Option<Self::ScreenLine> {
        self.prev_visible_logical_line(logical_line)
    }

    // TODO: This should return an Option, because it is really not required for the Document
    // interface, but it is currently used somewhere. This implementation works for that
    // one usecase though.
    fn line_number(&self, logical_line: &Self::ScreenLine) -> usize {
        if logical_line.start_index == NodeIndex(0) {
            1
        } else {
            2
        }
    }

    fn is_wrapped_line(&self, _logical_line: &Self::ScreenLine) -> bool {
        false
    }

    fn is_start_of_wrapped_line(&self, _logical_line: &Self::ScreenLine) -> bool {
        false
    }

    fn is_end_of_wrapped_line(&self, _logical_line: &Self::ScreenLine) -> bool {
        false
    }

    fn is_after_start_of_wrapped_line(&self, _logical_line: &Self::ScreenLine) -> bool {
        false
    }

    fn is_before_end_of_wrapped_line(&self, _logical_line: &Self::ScreenLine) -> bool {
        false
    }

    fn cursor_range(&self, cursor: &NodeIndex) -> CursorRange<Self::ScreenLine> {
        let logical_line = self.logical_line_of_node_index(*cursor);

        CursorRange {
            start: logical_line.clone(),
            end: logical_line.clone(),
            num_screen_lines: 1,
        }
    }

    fn convert_screen_line_to_cursor(
        &self,
        logical_line: Self::ScreenLine,
        _prev_cursor: &NodeIndex,
    ) -> NodeIndex {
        logical_line.start_index
    }

    fn move_cursor_down(&mut self, lines: usize, cursor: &NodeIndex) -> Option<NodeIndex> {
        let mut logical_line = self.logical_line_of_node_index(*cursor);
        let mut lines_moved = 0;

        while lines_moved < lines {
            let Some(next_logical_line) = self.next_visible_logical_line(&logical_line) else {
                break;
            };

            lines_moved += 1;
            logical_line = next_logical_line;
        }

        if lines_moved == 0 {
            None
        } else {
            Some(logical_line.start_index)
        }
    }

    fn move_cursor_up(&mut self, lines: usize, cursor: &NodeIndex) -> Option<NodeIndex> {
        let mut logical_line = self.logical_line_of_node_index(*cursor);
        let mut lines_moved = 0;

        while lines_moved < lines {
            let Some(next_logical_line) = self.prev_visible_logical_line(&logical_line) else {
                break;
            };

            lines_moved += 1;
            logical_line = next_logical_line;
        }

        if lines_moved == 0 {
            None
        } else {
            Some(logical_line.start_index)
        }
    }

    // THIS LOGIC WILL GET MORE COMPLICATED ONCE WE SUPPORT ACTUALLY MOVING THE CURSOR
    // WITHIN A LINE.

    fn expand_or_move_cursor_right_or_down(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let current_line = self.logical_line_of_node_index(*cursor);

        // Expand the first collapsed node on the line, otherwise move down.
        let mut first_collapsed_node = None;
        for (node_index, state) in self.collapsible_nodes_in_line(&current_line) {
            match state {
                Expanded => continue,
                Collapsed => {
                    first_collapsed_node = Some(*node_index);
                    break;
                }
            }
        }

        if let Some(node_index) = first_collapsed_node {
            let prev_collapsed_state = self.collapsible_nodes.insert(node_index, Expanded);
            Some(*cursor)
        } else {
            // TODO: Should only move down if it's a child of the current node
            self.move_cursor_down(1, cursor)
        }
    }

    fn collapse_or_move_cursor_left_or_up(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let current_line = self.logical_line_of_node_index(*cursor);

        // The user can see everything up to the first collapsed node in a line. If that
        // includes any currently expanded nodes, then we want to collapse the rightmost one.
        // Otherwise we'll move up.

        let mut last_collapsible_node = None;
        for (node_index, state) in self.collapsible_nodes_in_line(&current_line) {
            match state {
                Expanded => last_collapsible_node = Some(*node_index),
                Collapsed => break,
            }
        }

        if let Some(node_index) = last_collapsible_node {
            let prev_collapsed_state = self.collapsible_nodes.insert(node_index, Collapsed);
            Some(*cursor)
        } else {
            // TODO: Should move to *parent*
            self.move_cursor_up(1, cursor)
        }
    }

    fn debug_text_content(&self, logical_line: &LogicalLine) -> Vec<u8> {
        use std::fmt::Write;

        use bstr::ByteSlice;

        let LogicalLine {
            indentation,
            start_index,
            end_index,
        } = logical_line;

        let mut output = String::new();
        let _ = write!(output, "{: <indentation$}", "");

        let mut collapsed_node_index = None;
        let mut end_visible_index = *end_index;
        let mut collapsed_variant = false;

        for (node_index, state) in self.collapsible_nodes_in_line(logical_line) {
            match state {
                Expanded => continue,
                Collapsed => {
                    end_visible_index = *node_index;
                    if matches!(
                        self.core.token(*node_index).list_kind().unwrap(),
                        ListKind::VariantRecord | ListKind::VariantTuple
                    ) {
                        end_visible_index = *node_index + 1;
                        collapsed_variant = true;
                    }
                    collapsed_node_index = Some(*node_index);
                    break;
                }
            }
        }

        let start_range = self.core.node(*start_index).data_range.clone();
        let end_range = self.core.node(end_visible_index).data_range.clone();

        if let (Some(start), Some(end)) = (start_range, end_range) {
            let _ = write!(
                output,
                "{}",
                self.core.pretty_printed[start.start..end.end].as_bstr()
            );
        };

        if let Some(collapsed_node_index) = collapsed_node_index {
            if collapsed_variant {
                let _ = write!(output, " ");
            }
            let _ = write!(output, "...");

            let list_end_index = self
                .core
                .token(collapsed_node_index)
                .list_end_index()
                .unwrap();
            let end_of_collapsed_section = self.logical_line_of_node_index(list_end_index);
            let num_trailing_paren = end_of_collapsed_section.end_index.0 - list_end_index.0 + 1;
            for _ in 0..num_trailing_paren {
                let _ = write!(output, ")");
            }
        }

        output.into_bytes()
    }
}
