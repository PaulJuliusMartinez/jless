use std::collections::BTreeMap;

use crate::sexp::core::{DocCore, ListKind, NodeIndex};
use crate::sexp::layout;
use crate::sexp::layout::LogicalLine;

use ocaml_sexplib::tokenizer::{BasicTapeTokenizer, RawTokenTape};
use ocaml_sexplib::Ref;
use wabi_tree::OSBTreeMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollapseState {
    Collapsed,
    Expanded,
}

pub struct DocState {
    tokenizer: BasicTapeTokenizer,
    pub core: DocCore,
    pub next_top_level_node_index: NodeIndex,
    pub starts_of_logical_lines: OSBTreeMap<NodeIndex, (NodeIndex, usize)>,
    pub collapsible_nodes: BTreeMap<NodeIndex, CollapseState>,
}

impl DocState {
    pub fn new() -> Self {
        DocState {
            tokenizer: BasicTapeTokenizer::new(),
            core: DocCore::new(),
            next_top_level_node_index: NodeIndex(0),
            starts_of_logical_lines: OSBTreeMap::new(),
            collapsible_nodes: BTreeMap::new(),
        }
    }

    pub fn append(&mut self, data: &[u8], initial_collapse_state: CollapseState) {
        self.tokenizer.feed_more_data(data);
        self.process_additional_tokens(Some(data), false);
        self.maybe_add_new_top_level_nodes(initial_collapse_state);
    }

    pub fn eof(&mut self, initial_collapse_state: CollapseState) {
        self.tokenizer.eof();
        self.process_additional_tokens(None, true);
        self.maybe_add_new_top_level_nodes(initial_collapse_state);
    }

    fn process_additional_tokens(&mut self, current_data: Option<&[u8]>, seen_eof: bool) {
        while let Some(witness) = self.tokenizer.has_enough_data_to_produce_tokens() {
            let current_data = current_data.map(Ref::Transient);
            match self.tokenizer.next_raw_token(witness, current_data) {
                Ok(Some(raw_token)) => self.core.append_raw_token(raw_token),
                Ok(None) => {
                    self.core.append_eof();
                    break;
                }
                Err(err) => {
                    self.core.append_tokenizer_error(err);
                    if seen_eof {
                        self.core.append_eof();
                    }
                    break;
                }
            }
        }
    }

    pub fn maybe_add_new_top_level_nodes(&mut self, initial_collapse_state: CollapseState) {
        let Some(last_completed_top_level_sexp) =
            self.core.node_index_of_last_completed_top_level_sexp
        else {
            // No top level nodes yet.
            return;
        };

        while self.next_top_level_node_index <= last_completed_top_level_sexp {
            let logical_lines =
                self.add_logical_lines_for_top_level_node(self.next_top_level_node_index);

            self.next_top_level_node_index = logical_lines.last().unwrap().end_index + 1;

            for logical_line in logical_lines.iter() {
                self.create_collapsible_nodes(logical_line, initial_collapse_state);
            }
        }
    }

    fn add_logical_lines_for_top_level_node(
        &mut self,
        top_level_node_index: NodeIndex,
    ) -> Vec<LogicalLine> {
        let logical_lines = layout::layout_fully_expanded_node(&self.core, top_level_node_index);

        for LogicalLine {
            indentation,
            start_index,
            end_index,
        } in logical_lines.iter()
        {
            self.starts_of_logical_lines
                .insert(*start_index, (*end_index, *indentation));
        }

        logical_lines
    }

    fn create_collapsible_nodes(
        &mut self,
        logical_line: &LogicalLine,
        initial_collapse_state: CollapseState,
    ) {
        // For a given LogicalLine, we can collapse some of the content underneath it if
        // we have a container (i.e., a record, variant tuple/record, or a plain list)
        // that starts on that line, but does not _end_ on that line.
        //
        // If something is wrapped in a singleton, then we'll end up with two "containers"
        // that both start on the same line, but then both end together on a different line.
        // It doesn't make sense to be able to collapse _both_ of these, so we'll say the
        // first one is collapsible (so that "more" content, in terms of more parentheses,
        // get collapsed).

        // We'll keep track of the line where the previous collapsible node ended so we can
        // do this comparison. Use NodeIndex(0), because that's impossible (a collapsible
        // node must span at least two lines, so the end line can't start with index 0),
        // and avoids an awkward option comparison later.
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
                // Technically collapsible if it ends up across multiple lines
                | ListKind::Singleton => (),
            }

            let start_of_end_logical_line =
                self.logical_line_of_node_index(list_end_index).start_index;

            if start_of_end_logical_line != start_index_of_end_line_of_previous_collapsible_node {
                self.collapsible_nodes
                    .insert(node_index, initial_collapse_state);
                start_index_of_end_line_of_previous_collapsible_node = start_of_end_logical_line;
            }
        }
    }

    pub fn maybe_logical_line_of_node_index(&self, node_index: NodeIndex) -> Option<LogicalLine> {
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

    pub fn logical_line_of_node_index(&self, node_index: NodeIndex) -> LogicalLine {
        self.maybe_logical_line_of_node_index(node_index).unwrap()
    }
}

#[cfg(test)]
pub(super) mod test_helpers {
    use super::*;

    use crate::sexp::layout;

    impl DocState {
        pub fn all_logical_lines(&self) -> Vec<LogicalLine> {
            self.starts_of_logical_lines
                .iter()
                .map(|(start_index, (end_index, indentation))| LogicalLine {
                    indentation: *indentation,
                    start_index: *start_index,
                    end_index: *end_index,
                })
                .collect()
        }

        pub fn dump_all_logical_lines(&self) -> String {
            layout::tests::show_logical_lines(&self.core, self.all_logical_lines())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;

    use CollapseState::*;

    use insta::assert_snapshot;

    #[test]
    fn add_new_top_level_nodes_as_they_are_available() {
        let mut doc = DocState::new();
        assert_snapshot!(doc.dump_all_logical_lines(), @"");

        doc.append(b"(key1 value1)(key2 ", Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @" 0..=3  : (key1 value1)");

        doc.append(b"value2)trailing_atom", Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=3  : (key1 value1)
        4..=7  : (key2 value2)
        ");

        doc.eof(Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=3  : (key1 value1)
        4..=7  : (key2 value2)
        8..=8  : trailing_atom
        ");
    }

    #[test]
    fn handle_tokenization_errors_in_doc() {
        let mut doc = DocState::new();

        doc.append(b"a |# b", Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=0  : a
        1..=1  : ERR: TokenizationError(UnexpectedEndOfBlockComment)
        ");

        // Someday: The `BasicTapeTokenizer` doesn't handle receiving more input
        // after tokenization errors very well, so we get this awkward and
        // misleading `EofCalledMultipleTimes` error.
        doc.eof(Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=0  : a
        1..=1  : ERR: TokenizationError(UnexpectedEndOfBlockComment)
        2..=2  : ERR: TokenizationError(EofCalledMultipleTimes)
        ");
    }

    #[test]
    fn handle_tokenization_errors_at_eof() {
        let mut doc = DocState::new();
        doc.append(b"(\"a b", Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @"");

        doc.eof(Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=0  : (
        1..=1  :  ERR: TokenizationError(UnexpectedEofWhileInInQuotedAtom)
        2..=2  :  ERR: Unexpected EOF while parsing list
        3..=3  :
        ");

        let mut doc = DocState::new();
        doc.append(b"#| a", Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @"");

        doc.eof(Expanded);
        assert_snapshot!(doc.dump_all_logical_lines(), @" 0..=0  : ERR: TokenizationError(UnexpectedEofWhileInBlockComment)");
    }
}
