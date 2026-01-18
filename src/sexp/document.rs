use std::collections::BTreeMap;

use crate::document::{CursorRange, Document};
use crate::sexp::core::{AtomKind, DocCore, DocumentToken, ListKind, ListMetadata, NodeIndex};
use crate::sexp::layout::{self as layout, LogicalLine};

use ocaml_sexplib::input::InputRef;
use ocaml_sexplib::tokenizer::{BasicTapeTokenizer, RawTokenTape};

// COLLAPSIBLE NODES:
// - ListKind::Record
// - ListKind::Variant
//
// - Singleton/Unit/Plain: Yes
//
//
// If: start/end of list are on different logical lines
// And: not another collapsible with same start/end

pub struct SexpDocument {
    width: usize,
    tokenizer: BasicTapeTokenizer,
    core: DocCore,
    next_top_level_node_index: NodeIndex,
    starts_of_logical_lines: BTreeMap<NodeIndex, (NodeIndex, usize)>,
    collapsible_nodes: BTreeMap<NodeIndex, bool>,
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

            // Compute collapsible nodes
        }
        println!("Processed up to {:?}", self.next_top_level_node_index);
    }

    fn logical_line_of_node_index(&self, node_index: NodeIndex) -> Option<LogicalLine> {
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

    fn top_screen_line_and_cursor(&self) -> Option<(LogicalLine, NodeIndex)> {
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

    fn bottom_screen_line_and_cursor(&self) -> Option<(LogicalLine, NodeIndex)> {
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

    fn next_screen_line(&self, logical_line: &LogicalLine) -> Option<LogicalLine> {
        self.logical_line_of_node_index(logical_line.end_index + 1)
    }

    fn prev_screen_line(&self, logical_line: &LogicalLine) -> Option<LogicalLine> {
        if logical_line.start_index == NodeIndex(0) {
            None
        } else {
            self.logical_line_of_node_index(logical_line.start_index - 1)
        }
    }

    // 1-indexed
    fn line_number(&self, logical_line: &LogicalLine) -> usize {
        if logical_line.start_index == NodeIndex(0) {
            1
        } else {
            2
        }
    }

    fn is_wrapped_line(&self, _logical_line: &LogicalLine) -> bool {
        false
    }

    fn is_start_of_wrapped_line(&self, _logical_line: &LogicalLine) -> bool {
        false
    }

    fn is_end_of_wrapped_line(&self, _logical_line: &LogicalLine) -> bool {
        false
    }

    fn is_after_start_of_wrapped_line(&self, _logical_line: &LogicalLine) -> bool {
        false
    }

    fn is_before_end_of_wrapped_line(&self, _logical_line: &LogicalLine) -> bool {
        false
    }

    fn cursor_range(&self, cursor: &NodeIndex) -> CursorRange<LogicalLine> {
        let logical_line = self.logical_line_of_node_index(*cursor).unwrap();

        CursorRange {
            start: logical_line.clone(),
            end: logical_line.clone(),
            num_screen_lines: 1,
        }
    }

    fn convert_screen_line_to_cursor(
        &self,
        logical_line: LogicalLine,
        _prev_cursor: &NodeIndex,
    ) -> NodeIndex {
        logical_line.start_index
    }

    fn move_cursor_down(&self, lines: usize, cursor: &NodeIndex) -> Option<NodeIndex> {
        let mut logical_line = self.logical_line_of_node_index(*cursor).unwrap();
        let mut lines_moved = 0;

        while lines_moved < lines {
            let Some(next_logical_line) = self.next_screen_line(&logical_line) else {
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

    fn move_cursor_up(&self, lines: usize, cursor: &NodeIndex) -> Option<NodeIndex> {
        let mut logical_line = self.logical_line_of_node_index(*cursor).unwrap();
        let mut lines_moved = 0;

        while lines_moved < lines {
            let Some(next_logical_line) = self.prev_screen_line(&logical_line) else {
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

        let start_range = self.core.node(*start_index).data_range.clone();
        let end_range = self.core.node(*end_index).data_range.clone();

        if let (Some(start), Some(end)) = (start_range, end_range) {
            let _ = write!(
                output,
                "{}",
                self.core.pretty_printed[start.start..end.end].as_bstr()
            );
        };

        output.into_bytes()
    }
}
