use std::collections::BTreeMap;

use crate::sexp::core::{DocCore, DocumentToken, EndOfListMetadata, ListKind, NodeIndex};
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

use CollapseState::*;

pub struct DocState {
    tokenizer: BasicTapeTokenizer,
    pub core: DocCore,
    next_top_level_node_index: NodeIndex,
    pub starts_of_logical_lines: OSBTreeMap<NodeIndex, (NodeIndex, usize)>,
    pub collapsible_nodes: BTreeMap<NodeIndex, CollapseState>,
    initial_nested_collapse_state_for_top_level_nodes: InitialNestedCollapseStateForTopLevelNodes,
}

// When streaming in input, if the user hits 'C' to deeply collapse all input, then
// as new top-level nodes arrive, they should also be deeply collapsed. And if the user
// then hits 'e' to shallowly expand the top level nodes, then new top-level nodes should
// appear expanded one level, then deeply collapsed after that, as if they had been
// present all along.
//
// To achieve this, we don't need to remember every call to `collapse_node_and_siblings`,
// but can just maintain the desired deep collapsed state, and any shallow collapse states
// in order of decreasing depth. Running an expand/collapse command to a depth N overrides
// all previous commands to depths < N.
#[derive(Debug)]
struct InitialNestedCollapseStateForTopLevelNodes {
    eventual_collapse_state: CollapseState,
    shallow_collapse_states: Vec<(CollapseState, usize)>,
}

impl InitialNestedCollapseStateForTopLevelNodes {
    fn new() -> Self {
        InitialNestedCollapseStateForTopLevelNodes {
            eventual_collapse_state: Expanded,
            shallow_collapse_states: vec![],
        }
    }

    fn update(&mut self, collapse_state: CollapseState, depth: Option<usize>) {
        match depth {
            None => {
                self.eventual_collapse_state = collapse_state;
                self.shallow_collapse_states.clear();
            }
            Some(depth) => {
                // Pop states that apply to a lesser depth; these are now overridden.
                while let Some((_, other_depth)) = self.shallow_collapse_states.last() {
                    if *other_depth <= depth {
                        self.shallow_collapse_states.pop();
                    } else {
                        break;
                    }
                }

                // If the next state is the same one we're applying, we don't need to push our new
                // state.
                let next_state = match self.shallow_collapse_states.last() {
                    Some((state, _)) => *state,
                    None => self.eventual_collapse_state,
                };

                if collapse_state != next_state {
                    self.shallow_collapse_states.push((collapse_state, depth));
                }
            }
        }
    }
}

impl DocState {
    pub fn new() -> Self {
        DocState {
            tokenizer: BasicTapeTokenizer::new(),
            core: DocCore::new(),
            next_top_level_node_index: NodeIndex(0),
            starts_of_logical_lines: OSBTreeMap::new(),
            collapsible_nodes: BTreeMap::new(),
            initial_nested_collapse_state_for_top_level_nodes:
                InitialNestedCollapseStateForTopLevelNodes::new(),
        }
    }

    pub fn append(&mut self, data: &[u8]) {
        self.tokenizer.feed_more_data(data);
        self.process_additional_tokens(Some(data), false);
        self.maybe_add_new_top_level_nodes();
    }

    pub fn eof(&mut self) {
        self.tokenizer.eof();
        self.process_additional_tokens(None, true);
        self.maybe_add_new_top_level_nodes();
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

    pub fn maybe_add_new_top_level_nodes(&mut self) {
        let Some(last_completed_top_level_sexp) =
            self.core.node_index_of_last_completed_top_level_sexp
        else {
            // No top level nodes yet.
            return;
        };

        let eventual_collapse_state = self
            .initial_nested_collapse_state_for_top_level_nodes
            .eventual_collapse_state;

        while self.next_top_level_node_index <= last_completed_top_level_sexp {
            let top_level_node_index = self.next_top_level_node_index;

            let logical_lines = self.add_logical_lines_for_top_level_node(top_level_node_index);

            self.next_top_level_node_index = logical_lines.last().unwrap().end_index + 1;

            for logical_line in logical_lines.iter() {
                self.create_collapsible_nodes(logical_line, eventual_collapse_state);
            }

            self.initialize_shallow_collapse_states(top_level_node_index);
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
        eventual_collapse_state: CollapseState,
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
                    .insert(node_index, eventual_collapse_state);
                start_index_of_end_line_of_previous_collapsible_node = start_of_end_logical_line;
            }
        }
    }

    // Apply any shallow collapsing/expanding that have been applied
    // to top-level nodes previously.
    fn initialize_shallow_collapse_states(&mut self, top_level_node_index: NodeIndex) {
        if matches!(
            self.core.token(top_level_node_index),
            DocumentToken::StartOfList(_)
        ) {
            let first_child_index = top_level_node_index + 1;
            // Avoiding storing a ref to self because we mutate `collapsible_nodes` as
            // we iterate.
            let range = 0..(self
                .initial_nested_collapse_state_for_top_level_nodes
                .shallow_collapse_states
                .len());
            let mut most_shallow_collapse_state = None;

            for i in range {
                let (collapse_state, depth) = self
                    .initial_nested_collapse_state_for_top_level_nodes
                    .shallow_collapse_states[i];
                most_shallow_collapse_state = Some(collapse_state);

                // Normally this would get called on NodeIndex(0) when the user hit c/C/e/E,
                // but that'd affect all the top-level nodes, and we just want to update a
                // single one, which is nested in one level, so we use `depth - 1`. The
                // top-level node itself is handled below, using the most recent applied
                // collapse state.
                self.set_collapse_state_on_first_child_and_siblings(
                    first_child_index,
                    depth - 1,
                    collapse_state,
                );
            }

            // Finally, set the collapse state on the top level node itself. If this
            // is none, then a single deep collapse state was set, and used as the
            // default, so the top-level node is already in the desired state.
            if let Some(collapse_state) = most_shallow_collapse_state {
                if let Some(state) = self.collapsible_nodes.get_mut(&top_level_node_index) {
                    *state = collapse_state;
                }
            }
        }
    }

    pub fn set_collapse_state_on_node_and_siblings(
        &mut self,
        node_index: NodeIndex,
        depth: Option<usize>,
        desired_state: CollapseState,
    ) -> NodeIndex {
        // If we're updating the collapse state of all the top level nodes, we need to update
        // our initial state to apply to new top-level nodes that stream in.
        if self.core.parent_index(node_index).is_none() {
            self.initial_nested_collapse_state_for_top_level_nodes
                .update(desired_state, depth);
        }

        match depth {
            None => {
                // We implement deep collapsing/expanding in a more efficient manner by simply
                // updating all the collapsible nodes in a certain range, rather than traversing
                // the whole subtree and having to check how nested we are.
                self.deep_set_collapse_state_on_node_and_siblings(node_index, desired_state);
            }
            Some(n) => {
                let first_child = match self.core.parent_index(node_index) {
                    None => NodeIndex(0),
                    Some(parent_index) => parent_index + 1,
                };

                self.set_collapse_state_on_first_child_and_siblings(first_child, n, desired_state);
            }
        }

        // If the user was focused on the closing paren of a node, and that node is now
        // collapsed, switch to the opening paren.
        match self.core.token(node_index) {
            DocumentToken::EndOfList(EndOfListMetadata {
                list_start_index, ..
            }) => match self.collapsible_nodes.get(list_start_index) {
                Some(Collapsed) => *list_start_index,
                _ => node_index,
            },
            _ => node_index,
        }
    }

    fn deep_set_collapse_state_on_node_and_siblings(
        &mut self,
        node_index: NodeIndex,
        desired_state: CollapseState,
    ) {
        let range = match self.core.parent_index(node_index) {
            None => {
                // If we're on a top-level node, we want to update everything in the doc.
                self.collapsible_nodes.range_mut(..)
            }
            Some(parent_index) => {
                // Otherwise we'll update everything inside the parent (no matter the depth).
                let start = parent_index + 1;
                match self.core.token(parent_index).list_end_index() {
                    Some(end) => self.collapsible_nodes.range_mut(start..end),
                    None => self.collapsible_nodes.range_mut(start..),
                }
            }
        };

        for (_, state) in range {
            *state = desired_state;
        }
    }

    fn set_collapse_state_on_first_child_and_siblings(
        &mut self,
        first_child_index: NodeIndex,
        depth: usize,
        desired_state: CollapseState,
    ) {
        if depth == 0 {
            return;
        }

        let mut next_sibling = Some(first_child_index);

        while let Some(sibling_index) = next_sibling {
            let mut rec_depth = depth;
            if let Some(state) = self.collapsible_nodes.get_mut(&sibling_index) {
                *state = desired_state;
                rec_depth -= 1;
            }

            if matches!(
                self.core.token(sibling_index),
                DocumentToken::StartOfList(_),
            ) {
                let first_child = sibling_index + 1;
                self.set_collapse_state_on_first_child_and_siblings(
                    first_child,
                    rec_depth,
                    desired_state,
                );
            }

            next_sibling = self.core.node(sibling_index).next_sibling();
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

        pub fn compute_collapse_state_changes_from<T, F>(&mut self, mut f: F) -> (String, T)
        where
            F: FnMut(&mut Self) -> T,
        {
            let prev_collapsible_nodes = self.collapsible_nodes.clone();
            let x = f(self);
            let changes =
                diff_collapsible_node_states(prev_collapsible_nodes, &self.collapsible_nodes);
            (changes, x)
        }
    }

    pub fn diff_collapsible_node_states(
        prev_collapsible_nodes: BTreeMap<NodeIndex, CollapseState>,
        new_collapsible_nodes: &BTreeMap<NodeIndex, CollapseState>,
    ) -> String {
        assert_eq!(prev_collapsible_nodes.len(), new_collapsible_nodes.len());

        let mut changes = vec![];
        for (node_index, collapsed_state) in prev_collapsible_nodes.iter() {
            let new_collapsed_state = new_collapsible_nodes.get(node_index).unwrap();
            if collapsed_state != new_collapsed_state {
                let change = format!("{:?}({})", new_collapsed_state, node_index.0);
                changes.push(change);
            }
        }

        changes.join(" ")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fmt::Write;

    use insta::{allow_duplicates, assert_snapshot};

    fn new_doc(bytes: &'static [u8]) -> DocState {
        let mut doc = DocState::new();
        doc.append(bytes);
        doc.eof();
        doc
    }

    #[test]
    fn add_new_top_level_nodes_as_they_are_available() {
        let mut doc = DocState::new();
        assert_snapshot!(doc.dump_all_logical_lines(), @"");

        doc.append(b"(key1 value1)(key2 ");
        assert_snapshot!(doc.dump_all_logical_lines(), @" 0..=3  : (key1 value1)");

        doc.append(b"value2)trailing_atom");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=3  : (key1 value1)
        4..=7  : (key2 value2)
        ");

        doc.eof();
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=3  : (key1 value1)
        4..=7  : (key2 value2)
        8..=8  : trailing_atom
        ");
    }

    #[test]
    fn handle_tokenization_errors_in_doc() {
        let mut doc = DocState::new();

        doc.append(b"a |# b");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=0  : a
        1..=1  : ERR: TokenizationError(UnexpectedEndOfBlockComment)
        ");

        // Someday: The `BasicTapeTokenizer` doesn't handle receiving more input
        // after tokenization errors very well, so we get this awkward and
        // misleading `EofCalledMultipleTimes` error.
        doc.eof();
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=0  : a
        1..=1  : ERR: TokenizationError(UnexpectedEndOfBlockComment)
        2..=2  : ERR: TokenizationError(EofCalledMultipleTimes)
        ");
    }

    #[test]
    fn handle_tokenization_errors_at_eof() {
        let mut doc = DocState::new();
        doc.append(b"(\"a b");
        assert_snapshot!(doc.dump_all_logical_lines(), @"");

        doc.eof();
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=0  : (
        1..=1  :  ERR: TokenizationError(UnexpectedEofWhileInInQuotedAtom)
        2..=2  :  ERR: Unexpected EOF while parsing list
        3..=3  :
        ");

        let mut doc = DocState::new();
        doc.append(b"#| a");
        assert_snapshot!(doc.dump_all_logical_lines(), @"");

        doc.eof();
        assert_snapshot!(doc.dump_all_logical_lines(), @" 0..=0  : ERR: TokenizationError(UnexpectedEofWhileInBlockComment)");
    }

    #[test]
    fn test_collapsing_and_expanding_nodes() {
        let mut doc = new_doc(
            b"((a 1)(b ((cc 3)(dd 4)(ee ((fff 5)(ggg 6)))))(h ((ii 7)(jj 8))))((w 1)(xx yy zz))",
        );
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
         0..=4  : ((a 1)
         5..=7  :  (b (
         8..=11 :    (cc 3)
        12..=15 :    (dd 4)
        16..=18 :    (ee (
        19..=22 :      (fff 5)
        23..=30 :      (ggg 6)))))
        31..=33 :  (h (
        34..=37 :    (ii 7)
        38..=44 :    (jj 8))))
        45..=49 : ((w 1)
        50..=51 :  (xx
        52..=52 :   yy
        53..=55 :   zz))
        ");

        let mut go = |node_index: usize, collapse_state, depth| {
            let (mut result, new_index) = doc.compute_collapse_state_changes_from(|doc| {
                doc.set_collapse_state_on_node_and_siblings(
                    NodeIndex(node_index),
                    depth,
                    collapse_state,
                )
            });

            if new_index.0 != node_index {
                let _ = write!(result, " (returned {new_index:?})");
            }

            result
        };

        assert_snapshot!(go(0, Collapsed, Some(1)), @"Collapsed(0) Collapsed(45)");
        assert_snapshot!(go(0, Collapsed, Some(2)), @"Collapsed(7) Collapsed(33) Collapsed(50)");
        assert_snapshot!(go(1, Collapsed, None), @"Collapsed(18)");

        assert_snapshot!(go(1, Expanded, None), @"Expanded(7) Expanded(18) Expanded(33)");
        assert_snapshot!(go(0, Expanded, None), @"Expanded(0) Expanded(45) Expanded(50)");

        // Switch to start of list if on end and it gets collapsed.
        assert_snapshot!(go(55, Collapsed, Some(1)), @"Collapsed(0) Collapsed(45) (returned NodeIndex(45))");
    }

    #[test]
    fn test_initial_nested_collapse_state_for_top_level_nodes() {
        let mut state = InitialNestedCollapseStateForTopLevelNodes::new();

        let dump = |state: &InitialNestedCollapseStateForTopLevelNodes| {
            let eventual = format!("eventual = {:?}", state.eventual_collapse_state);
            let shallow = format!("shallow  = {:?}", state.shallow_collapse_states);
            format!("{eventual}\n{shallow}")
        };

        assert_snapshot!(dump(&state), @r"
        eventual = Expanded
        shallow  = []
        ");

        state.update(Collapsed, None);
        assert_snapshot!(dump(&state), @r"
        eventual = Collapsed
        shallow  = []
        ");

        state.update(Collapsed, Some(5));
        assert_snapshot!(dump(&state), @r"
        eventual = Collapsed
        shallow  = []
        ");

        state.update(Expanded, Some(3));
        assert_snapshot!(dump(&state), @r"
        eventual = Collapsed
        shallow  = [(Expanded, 3)]
        ");

        state.update(Expanded, Some(1));
        assert_snapshot!(dump(&state), @r"
        eventual = Collapsed
        shallow  = [(Expanded, 3)]
        ");

        state.update(Collapsed, Some(2));
        assert_snapshot!(dump(&state), @r"
        eventual = Collapsed
        shallow  = [(Expanded, 3), (Collapsed, 2)]
        ");

        state.update(Expanded, Some(5));
        assert_snapshot!(dump(&state), @r"
        eventual = Collapsed
        shallow  = [(Expanded, 5)]
        ");

        state.update(Collapsed, Some(6));
        assert_snapshot!(dump(&state), @r"
        eventual = Collapsed
        shallow  = []
        ");
    }

    #[test]
    fn test_deep_collapsing_and_expanding_applies_to_new_top_level_nodes() {
        fn check(actions: Vec<(CollapseState, Option<usize>)>) -> String {
            allow_duplicates! {
                let mut doc = DocState::new();
                doc.append(b"start ");

                for (collapse_state, depth) in actions.into_iter() {
                    doc.set_collapse_state_on_node_and_siblings(NodeIndex(0), depth, collapse_state);
                }

                doc.append(b"((1 (4 (6 (8 _)))))");
                assert_snapshot!(doc.dump_all_logical_lines(), @r"
                 0..=0  : start
                 1..=3  : ((1
                 4..=5  :   (4
                 6..=7  :    (6
                 8..=9  :     (8
                10..=15 :      _)))))
                ");

                let collapse_states = doc
                    .collapsible_nodes
                    .iter()
                    .map(|(node_index, state)| format!("{} => {state:?}", node_index.0))
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{:?}", collapse_states)
            }
        }

        assert_snapshot!(
            check(vec![]),
            @r#""1 => Expanded, 4 => Expanded, 6 => Expanded, 8 => Expanded""#,
        );

        assert_snapshot!(
            check(vec![(Collapsed, None)]),
            @r#""1 => Collapsed, 4 => Collapsed, 6 => Collapsed, 8 => Collapsed""#,
        );

        assert_snapshot!(
            check(vec![(Collapsed, None), (Expanded, Some(1))]),
            @r#""1 => Expanded, 4 => Collapsed, 6 => Collapsed, 8 => Collapsed""#,
        );

        assert_snapshot!(
            check(vec![(Collapsed, None), (Expanded, Some(3)), (Collapsed, Some(2)), (Expanded, Some(1))]),
            @r#""1 => Expanded, 4 => Collapsed, 6 => Expanded, 8 => Collapsed""#,
        );
    }
}
