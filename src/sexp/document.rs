use std::collections::BTreeMap;
use std::iter::DoubleEndedIterator;
use std::ops::Range;

use crate::document::{ContentRange, Document};
use crate::search::InvertedPairedDelimeters;
use crate::sexp::core::{
    AtomKind, AtomMetadata, DocCore, DocumentNode, DocumentToken, ListKind, ListMetadata, NodeIndex,
};
use crate::sexp::layout::{self as layout, LogicalLine};

use ocaml_sexplib::input::InputRef;
use ocaml_sexplib::tokenizer::{BasicTapeTokenizer, RawTokenTape};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum CollapseState {
    Collapsed,
    Expanded,
}

use CollapseState::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FocusTargetKind {
    Normal,
    ListValueOfRecordField,
}

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
                Err(_err) => unimplemented!("TODO: appending errors to sexp::core::DocCore"),
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

    // Returns the focusable nodes in a line. We use a bunch of heuristics to decide what
    // is and isn't "focusable", trying to capture something that feels natural and intuitive
    // when thinking about where the syntax highlighting moves, and what gets copied to the
    // clipboard when using yank commands.
    //
    // We start by thinking about navigating through a basic record, where each record field
    // is composed of at least four separate tokens: "(" <key> <value> ")"
    // When the value is a simple atom, we want to treat this as a single focusable unit.
    // In general, there should be one focusable item per relevant "data unit".
    //
    // When we display multiple values inline, they all should be focusable:
    // > (list_of_values (one two three four five))
    // Here, when we focus the record key, we're also focusing the list itself, then all
    // five atoms inside are focusable too.
    //
    // Since the container (the list) is also focused when the record field has the focus,
    // it isn't focusable on its own.
    //
    // An exception here is for variants. We will support focusing the variant constructor
    // separately (though technically we'll say the '(' is the focused node index). Otherwise
    // if a variant value is inlined it feels a little weird for the focus to "jump" over the
    // constructor:
    // > (variant_value (Variant one two))
    // So here we can focus the "variant_value" record field, the actual "Variant" constructor,
    // and both atoms.
    //
    // Additionally, if there are nested containers, each one besides the direct record value
    // are focusable:
    // > (nested_value (((atom_key one) (list_key (two three)))))
    //   ^             *^^              ^          ^   ^
    //   field          |   field     field      elem  elem
    //               record
    //  * singleton is not focusable
    //
    //
    // Other considerations:
    // - comments and errors are always focusable
    // - if comments or errors break up the paren and atom of a record field + key or variant +
    // constructor, both are focusable (and both are treated as focusing the record field/variant)
    // - closing parens are not focusable, unless the line starts with them.
    fn focusable_nodes_in_line(
        &self,
        logical_line: &LogicalLine,
    ) -> Vec<(NodeIndex, FocusTargetKind)> {
        // Someday: Have this return a SmallVec instead?
        use FocusTargetKind::*;
        let mut focusable_nodes = vec![];

        let mut last_token_was_record_field = false;
        let mut last_token_was_record_key = false;
        let mut next_token_is_record_value = false;
        let mut last_token_was_variant = false;

        for (i, node_index) in logical_line.node_indexes().enumerate() {
            let mut is_this_token_record_field = false;
            let mut is_this_token_record_key = false;
            let mut is_this_token_variant = false;

            let this_token_is_record_value = next_token_is_record_value;
            next_token_is_record_value = false;

            match self.core.token(node_index) {
                // These are always focusable
                DocumentToken::LineComment
                | DocumentToken::BlockComment
                | DocumentToken::Error(_) => {
                    focusable_nodes.push((node_index, Normal));

                    is_this_token_record_field = false;
                    is_this_token_record_key = false;
                    is_this_token_variant = false;
                }
                DocumentToken::Unit { .. } => {
                    if !last_token_was_record_key {
                        focusable_nodes.push((node_index, Normal));
                    }
                }
                DocumentToken::Atom(AtomMetadata { atom_kind, .. }) => {
                    let should_skip =
                        // Skip record keys associated with record fields
                        last_token_was_record_field ||
                        // Skip constructors of variants
                        last_token_was_variant ||
                        // Skip atom record-values
                        last_token_was_record_key;

                    if !should_skip {
                        focusable_nodes.push((node_index, Normal));
                    }

                    is_this_token_record_key =
                        last_token_was_record_field && matches!(atom_kind, AtomKind::RecordKey);
                    next_token_is_record_value = is_this_token_record_key;
                }
                DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => {
                    let should_skip = last_token_was_record_key && !list_kind.is_variant();

                    let focus_target_kind = if this_token_is_record_value {
                        // We "coalesce" singletons when they are values of record fields,
                        // so we still will consider the next token as the value of a record field.
                        next_token_is_record_value = matches!(list_kind, ListKind::Singleton);
                        ListValueOfRecordField
                    } else {
                        Normal
                    };

                    if !should_skip {
                        focusable_nodes.push((node_index, focus_target_kind));
                    }

                    is_this_token_record_field = matches!(list_kind, ListKind::RecordField);
                    is_this_token_variant = list_kind.is_variant();
                }
                DocumentToken::EndOfList(_) => {
                    // Only allow focusing these at the start of the line.
                    if i == 0 {
                        focusable_nodes.push((node_index, Normal));
                    }
                }
            }

            last_token_was_record_field = is_this_token_record_field;
            last_token_was_record_key = is_this_token_record_key;
            last_token_was_variant = is_this_token_variant;
        }

        focusable_nodes
    }

    // Returns the last node with FocusTargetKind = `Normal`, or, if there are none, just
    // returns the last node in the provided list.
    fn last_normal_focusable_node_or_last_node(
        focusable_nodes: Vec<(NodeIndex, FocusTargetKind)>,
    ) -> Option<NodeIndex> {
        if let Some(last_normal_focusable_node) = focusable_nodes
            .iter()
            .filter_map(|(node_index, focus_target_kind)| {
                if *focus_target_kind == FocusTargetKind::Normal {
                    Some(*node_index)
                } else {
                    None
                }
            })
            .last()
        {
            return Some(last_normal_focusable_node);
        }

        focusable_nodes.last().map(|(node_index, _)| *node_index)
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

    fn first_visible_line_at_or_above(&self, logical_line: LogicalLine) -> LogicalLine {
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

        for end_node_index in logical_line.node_indexes().rev() {
            let Some(list_start_index) = self.core.token(end_node_index).list_start_index() else {
                // Since all the closing parens go at the end, once we see a non-end of list,
                // then we can stop checking.
                break;
            };

            match self.collapsible_nodes.get(&list_start_index) {
                None | Some(Expanded) => continue,
                Some(Collapsed) => return self.logical_line_of_node_index(list_start_index),
            }
        }

        logical_line
    }

    fn prev_visible_logical_line(&self, logical_line: &LogicalLine) -> Option<LogicalLine> {
        if logical_line.start_index == NodeIndex(0) {
            return None;
        }

        let previous_logical_line = self.logical_line_of_node_index(logical_line.start_index - 1);

        Some(self.first_visible_line_at_or_above(previous_logical_line))
    }

    // Very annoying that this can't be in the `impl Document` block...
    fn move_cursor_up_one_line(&mut self, cursor: NodeIndex) -> Option<NodeIndex> {
        let logical_line = self.logical_line_of_node_index(cursor);
        let Some(prev_logical_line) = self.prev_visible_logical_line(&logical_line) else {
            // If there's no previous line, then we'll focus the first focusable node in
            // that line. (This should only ever happen if it's the first line of the document.)
            assert_eq!(logical_line.start_index.0, 0);
            if cursor.0 != 0 {
                return Some(NodeIndex(0));
            } else {
                return None;
            }
        };

        let DocumentNode {
            parent_index,
            prev_sibling,
            ..
        } = self.core.node(cursor);

        // Normally we want to focus the last "normal" focus target in the previous line that is at
        // or before the previous sibling, assuming that the previous line is either a sibling or
        // parent of the current node. So we want:
        //
        // (1
        //  2) < Moving up from 2 to focus 1
        //
        // ((a (
        //    1))) < Moving up from 1 to focus the key "a", and not the top level list.
        //
        // ((a (Variant
        //    1))) < Moving up from 1 to focus the key "a" (but not "(Variant").
        //
        // (((a 1)
        //   (b (2 3 4)))
        //  sibling) < Moving up from here should focus the key "(b", not "4".
        let mut focusable_nodes = self.focusable_nodes_in_line(&prev_logical_line);
        let mut relative_node = None;

        if prev_sibling.is_some() && prev_logical_line.contains_node_index(prev_sibling.unwrap()) {
            relative_node = *prev_sibling;
        } else if parent_index.is_some()
            && prev_logical_line.contains_node_index(parent_index.unwrap())
        {
            relative_node = *parent_index;
        }

        if let Some(relative_node) = relative_node {
            focusable_nodes.retain(|(node_index, _)| *node_index <= relative_node);

            match Self::last_normal_focusable_node_or_last_node(focusable_nodes) {
                Some(node_index) => return Some(node_index),
                None => {
                    // Probably shouldn't ever happen; just return the start of the line.
                    return Some(prev_logical_line.start_index);
                }
            }
        } else {
            // Focus first thing in the previous line.
            return Some(focusable_nodes.first().unwrap().0);
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
            Some((start_index, (end_index, indentation))) => {
                let last_line = LogicalLine {
                    indentation: *indentation,
                    start_index: *start_index,
                    end_index: *end_index,
                };

                let last_visible_line = self.first_visible_line_at_or_above(last_line);
                let cursor = last_visible_line.start_index;
                Some((last_visible_line, cursor))
            }
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

    fn cursor_range(&self, cursor: &NodeIndex) -> ContentRange<Self::ScreenLine> {
        let logical_line = self.logical_line_of_node_index(*cursor);

        ContentRange {
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
            // At least for now, we'll always assume we want the new focus to be at the start
            // of the line, rather than figuring out offset within the line.
            Some(logical_line.start_index)
        }
    }

    fn move_cursor_up(&mut self, lines: usize, cursor: &NodeIndex) -> Option<NodeIndex> {
        let mut cursor = *cursor;
        let mut lines_moved = 0;

        // ((foo 1)
        //  (bar 2)
        //  (baz 3)
        //
        // In the above example, if the cursor starts on "(baz 3)", and we move up two lines via
        // `2k`, we also should end up on "(foo 1)", not the start of the list, so we'll implement
        // this as moving one line at a time, `lines` times.
        while lines_moved < lines {
            let Some(new_cursor) = self.move_cursor_up_one_line(cursor) else {
                break;
            };

            lines_moved += 1;
            cursor = new_cursor;
        }

        if lines_moved == 0 {
            None
        } else {
            Some(cursor)
        }
    }

    fn expand_or_move_cursor_right_or_down(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let current_line = self.logical_line_of_node_index(*cursor);

        // The name of this function gives us our priorities:
        // - Expand something,
        // - Or, move to a focusable node after the cursor,
        // - Otherwise, if the line has any collapsible nodes, move down

        // When we have a plain container (e.g. the first line below), when we're focused
        // on the line, and it's collapsed, the cursor will be the same as the node index of
        // the collapsible node.
        //
        // When we're on a collapsible value of a record key, the collapsible node will
        // appear after the cursor. If that collapsible value is a variant (e.g. second
        // line below), that's not a value we want to normally by default, so we'll expand
        // it before moving to it.
        //
        // So the logic is: find next focusable node on line, and check if there's any
        // collapsible nodes in [cursor, next_focusable_node] -- both sides inclusive. If
        // there is, expand it.
        //
        // ((a 1)
        //  (b (Variant
        //    2
        //    3)))

        let focusable_nodes = self.focusable_nodes_in_line(&current_line);

        let next_focusable_node_in_line = focusable_nodes
            .iter()
            .find(|(focusable_index, _)| *cursor < *focusable_index)
            .map(|(node_index, _)| *node_index);

        // Track whether there are any collapsible nodes, so we know whether
        // we should try to move down if there's nothing to expand, and nothing
        // to the right of the cursor.
        let mut any_collapsible_nodes = false;

        let node_to_expand = 'find_node_to_expand: {
            let collapsible_nodes_in_line = self.collapsible_nodes_in_line(&current_line);

            let end_of_range_to_check_for_collapsed_nodes =
                next_focusable_node_in_line.unwrap_or(current_line.end_index);

            'checking_collapsible_nodes: for (node_index, collapse_state) in
                collapsible_nodes_in_line
            {
                any_collapsible_nodes = true;

                // Ignore collapsible nodes before cursor
                if *node_index < *cursor {
                    continue 'checking_collapsible_nodes;
                }

                // Once we're past the end, stop checking for collapsible nodes
                if end_of_range_to_check_for_collapsed_nodes < *node_index {
                    break 'find_node_to_expand None;
                }

                if *collapse_state == Collapsed {
                    // We found a collapsed node!
                    break 'find_node_to_expand Some(*node_index);
                }
            }

            // Didn't find a node to expand.
            None
        };

        // If we found a collapsed node, expand it!
        if let Some(node_to_expand) = node_to_expand {
            let _prev_state = self.collapsible_nodes.insert(node_to_expand, Expanded);
            // Cursor doesn't move.
            return Some(*cursor);
        }

        // We didn't find something to collapse, so we'll try to move
        // to the next focusable node in the line.
        if let Some(next_focusable_node_in_line) = next_focusable_node_in_line {
            return Some(next_focusable_node_in_line);
        }

        // Nothing else to focus on this line, so if there was anything collapsible, we'll
        // move down into it, otherwise there's nothing to be done.
        if any_collapsible_nodes {
            return self.move_cursor_down(1, cursor);
        }

        None
    }

    fn collapse_or_move_cursor_left_or_up(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let current_line = self.logical_line_of_node_index(*cursor);

        // The name of this function gives us our priorities:
        // - Collapse something,
        // - Or, move the cursor left on the current line if possible
        // - Otherwise, if the focused node has a parent, try to move there (some weird
        // cases here, see below.)

        // Deciding what to collapse is complicated: In the example below, there are two
        // collapsible nodes in the first line: the top level list, and the record field
        // with the variant value. If we're focused on the top level list, and we hit
        // left, we should collapse the top level list. We'd prefer the variant itself
        // to not be focusable, but we make it so for consistency reasons. Therefore even
        // when that is focused, we want this function to move to the key first, and then
        // collapse the variant if we call this function again.
        //
        // ((a (Variant
        //    1))
        //  ((b 1)
        //   (c 2)))
        //
        // So, our logic here will be:
        // - Find first expanded node to the right of the cursor (inclusive)
        // - If that's to the right of the cursor, collapse it
        // - If that's the cursor itself, and the focus target kind is `Normal`, collapse it
        // - If that's the cursor itself, but the focus target kind is `ListValueOfRecordField`,
        // try moving left if possible; if not possible, then collapse it (not sure this can
        // ever happen)

        let node_to_collapse = self.collapsible_nodes_in_line(&current_line).find_map(
            |(node_index, collapsed_state)| {
                if *cursor <= *node_index && *collapsed_state == Expanded {
                    Some(*node_index)
                } else {
                    None
                }
            },
        );

        let focusable_nodes = self.focusable_nodes_in_line(&current_line);
        let mut cursor_focus_target_kind = FocusTargetKind::Normal;
        let mut prev_focusable_node_in_line = None;

        for (node_index, focus_target_kind) in focusable_nodes.into_iter() {
            if node_index < *cursor {
                prev_focusable_node_in_line = Some(node_index);
            } else {
                if node_index == *cursor {
                    cursor_focus_target_kind = focus_target_kind;
                }
                break;
            }
        }

        // Now we have our expandable node, and where we'd move if we can move left,
        // so we can go through the checklist from above:
        if let Some(node_to_collapse) = node_to_collapse {
            let should_collapse = {
                if *cursor < node_to_collapse {
                    true
                } else {
                    match cursor_focus_target_kind {
                        FocusTargetKind::Normal => true,
                        FocusTargetKind::ListValueOfRecordField => {
                            // Pretty sure this couldn't actually ever be none.
                            prev_focusable_node_in_line.is_none()
                        }
                    }
                }
            };

            if should_collapse {
                let _prev_state = self.collapsible_nodes.insert(node_to_collapse, Collapsed);
                // Cursor doesn't move.
                return Some(*cursor);
            }
        }

        // If we didn't collapse, try to move left:
        if let Some(prev_focusable_node_in_line) = prev_focusable_node_in_line {
            return Some(prev_focusable_node_in_line);
        }

        // Can't move further left on our line, so we'll try to move to our parent.
        if let Some(parent_index) = self.core.node(*cursor).parent_index {
            // We don't always want the focus to move to the parent_index. Specifically,
            // if the parent is the value of a record field, then we want to move the
            // cursor to record key, not the parent.
            //
            // ((Variant
            //    (foo 1)     < If cursor is here, we do want to go to the parent
            //    (bar 2))
            //
            // ((key (Variant
            //    (foo 1)     < But if cursor is here, we want to go to "key"s record field.
            //    (bar 2))
            //
            // We should be able to handle this by setting the focus to be the last `Normal`
            // focusable node in the parent's line. (The actual parent would appear to have
            // a FocusTargetKind of `ListValueOfRecordField`, which isn't what we want.)
            //
            // There's another case to consider, that's more difficult: Do we really want
            // to move to the parent, or just move left one indentation level? In the case
            // below, if we're focused on "(bar 2)", and we hit left, do we want to move
            // to focus "(key (", or do we just want to move up and focus "Variant"? I'm
            // not sure how you could detect this case and find that line, so we won't solve
            // it for now.
            //
            // ((key (
            //     ; comment
            //     Variant
            //       (foo 1)
            //       (bar 2))

            let parent_line = self.logical_line_of_node_index(parent_index);
            let mut focusable_nodes = self.focusable_nodes_in_line(&parent_line);
            focusable_nodes.retain(|(node_index, _)| {
                // The our parent is a regular list, then the first child of the list will
                // also be on that line. We don't want to focus a sibling.
                *node_index <= parent_index
            });

            match Self::last_normal_focusable_node_or_last_node(focusable_nodes) {
                Some(node_index) => return Some(node_index),
                None => {
                    // This probably shouldn't ever happen; we'll just focus the parent.
                    return Some(parent_index);
                }
            }
        }

        None
    }

    fn debug_text_content(&self, logical_line: &LogicalLine, cursor: &NodeIndex) -> Vec<u8> {
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
            let mut line_content = self.core.pretty_printed[start.start..end.end].to_vec();

            if start_index <= cursor && cursor <= end_index {
                if let Some(cursor_range) = &self.core.node(*cursor).data_range {
                    let offset = cursor_range.start - start.start;
                    if line_content[offset] == b'(' {
                        line_content[offset] = b'[';
                    } else {
                        line_content[offset] = b'*';
                    }
                }
            }

            let _ = write!(output, "{}", line_content.as_bstr());
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

    fn inverted_paired_delimiters_for_search_input() -> InvertedPairedDelimeters {
        InvertedPairedDelimeters {
            square_brackets: false,
            curly_braces: false,
            parentheses: true,
        }
    }

    fn raw_bytes_for_searching(&self) -> &[u8] {
        self.core.raw_bytes_of_complete_content()
    }

    fn raw_byte_range_of_cursor(&self, cursor: &NodeIndex) -> Range<usize> {
        // TODO: Implement me
        0..0
    }

    fn raw_byte_index_to_cursor(&self, index: usize) -> NodeIndex {
        // TODO: Implement me
        NodeIndex(0)
    }

    fn raw_byte_index_to_visible_screen_line(&self, index: usize) -> Self::ScreenLine {
        // TODO: Implement me
        self.logical_line_of_node_index(NodeIndex(0))
    }

    fn closest_visible_cursor(&self, cursor: &NodeIndex) -> NodeIndex {
        // TODO: Implement me
        *cursor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::document::Document;
    use crate::sexp::core::NodeIndex;

    use std::fmt::Write;

    use bstr::ByteSlice;
    use insta::assert_snapshot;

    const FAR_AWAY_CURSOR: NodeIndex = NodeIndex(usize::MAX);

    fn new_doc(bytes: &'static [u8]) -> SexpDocument {
        let mut doc = SexpDocument::new(100);
        doc.append(bytes);
        doc.eof();
        doc
    }

    fn dump(doc: &SexpDocument) -> String {
        let logical_lines: Vec<LogicalLine> = doc
            .starts_of_logical_lines
            .iter()
            .map(|(start_index, (end_index, indentation))| LogicalLine {
                indentation: *indentation,
                start_index: *start_index,
                end_index: *end_index,
            })
            .collect();
        crate::sexp::layout::tests::show_logical_lines(&doc.core, logical_lines)
    }

    fn show_visible_lines(doc: &SexpDocument) -> String {
        let Some((top_screen_line, _)) = doc.top_screen_line_and_cursor() else {
            return "".to_string();
        };

        let mut output = String::new();

        let mut next_visible_line = Some(top_screen_line);

        while let Some(visible_line) = &next_visible_line {
            writeln!(
                output,
                "{}",
                doc.debug_text_content(visible_line, &FAR_AWAY_CURSOR)
                    .as_bstr()
            );

            next_visible_line = doc.next_screen_line(visible_line);
        }

        output
    }

    #[test]
    fn add_new_top_level_nodes_as_they_are_available() {
        let mut doc = SexpDocument::new(100);
        assert_snapshot!(show_visible_lines(&doc), @"");

        doc.append(b"(key1 value1)(key2 ");
        assert_snapshot!(show_visible_lines(&doc), @"(key1 value1)");

        doc.append(b"value2)trailing_atom");
        assert_snapshot!(show_visible_lines(&doc), @r"
        (key1 value1)
        (key2 value2)
        ");

        doc.eof();
        assert_snapshot!(show_visible_lines(&doc), @r"
        (key1 value1)
        (key2 value2)
        trailing_atom
        ");
    }

    #[derive(Copy, Clone, Debug)]
    enum Action {
        Down(usize),
        Up(usize),
        Right,
        Left,
        FocusBottom,
    }

    use Action::*;

    impl SexpDocument {
        fn perform_action(
            &mut self,
            current_cursor: NodeIndex,
            action: Action,
        ) -> Option<NodeIndex> {
            match action {
                Down(lines) => self.move_cursor_down(lines, &current_cursor),
                Up(lines) => self.move_cursor_up(lines, &current_cursor),
                Right => self.expand_or_move_cursor_right_or_down(&current_cursor),
                Left => self.collapse_or_move_cursor_left_or_up(&current_cursor),
                FocusBottom => self
                    .bottom_screen_line_and_cursor()
                    .map(|(_, cursor)| cursor),
            }
        }
    }

    #[track_caller]
    fn show_cursor_movements(
        doc: &mut SexpDocument,
        starting_cursor: NodeIndex,
        actions: Vec<Action>,
    ) -> String {
        let mut rows = vec![];

        let mut current_cursor = starting_cursor;

        for action in actions.into_iter() {
            let prev_collapsed_states = doc.collapsible_nodes.clone();
            let new_cursor = doc.perform_action(current_cursor, action);
            let new_collapsed_states = doc.collapsible_nodes.clone();

            let action = format!("{:?} =>", action);

            let mut result = if let Some(new_cursor) = new_cursor {
                current_cursor = new_cursor;
                format!("{:?}", new_cursor)
            } else {
                "-".to_string()
            };

            assert_eq!(prev_collapsed_states.len(), new_collapsed_states.len());
            for (node_index, collapsed_state) in prev_collapsed_states.iter() {
                let new_collapsed_state = new_collapsed_states.get(node_index).unwrap();
                if collapsed_state != new_collapsed_state {
                    let _ = write!(result, " {:?}({})", new_collapsed_state, node_index.0);
                }
            }

            rows.push(vec![action, result]);
        }

        let with_borders = false;
        crate::test_helpers::format_table(&rows, with_borders)
    }

    #[test]
    fn basic_moving_cursor_up_and_down() {
        let mut doc = new_doc(b"((a 1) (b 2) (c (Variant (d 3) (e 4))))");

        assert_snapshot!(dump(&doc), @r"
         0..=4  : ((a 1)
         5..=8  :  (b 2)
         9..=12 :  (c (Variant
        13..=16 :    (d 3)
        17..=23 :    (e 4))))
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(0),
            vec![Down(1), Down(1), Down(1), Down(1), Down(1)],
        );
        assert_snapshot!(movements, @r"
        Down(1) => NodeIndex(5)
        Down(1) => NodeIndex(9)
        Down(1) => NodeIndex(13)
        Down(1) => NodeIndex(17)
        Down(1) => -
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(17),
            vec![Up(1), Up(1), Up(1), Up(1), Up(1), Up(1)],
        );
        assert_snapshot!(movements, @r"
        Up(1) => NodeIndex(13)
        Up(1) => NodeIndex(9)
        Up(1) => NodeIndex(5)
        Up(1) => NodeIndex(1)
        Up(1) => NodeIndex(0)
        Up(1) => -
        ");
    }

    #[test]
    fn focus_buttom() {
        let mut doc = new_doc(b"(1 2 3)");
        assert_snapshot!(dump(&doc), @r"
        0..=1  : (1
        2..=2  :  2
        3..=4  :  3)
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(0), vec![FocusBottom]);
        assert_snapshot!(movements, @r"FocusBottom => NodeIndex(3)");

        let movements = show_cursor_movements(&mut doc, NodeIndex(0), vec![Left, FocusBottom]);
        assert_snapshot!(movements, @r"
        Left =>        NodeIndex(0) Collapsed(0)
        FocusBottom => NodeIndex(0)
        ");
    }

    #[test]
    fn moving_around_variants() {
        let mut doc = new_doc(b"((Variant (foo 1) (bar 2)))");

        assert_snapshot!(dump(&doc), @r"
        0..=2  : ((Variant
        3..=6  :    (foo 1)
        7..=12 :    (bar 2)))
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(3), vec![Up(1)]);
        assert_snapshot!(movements, @"Up(1) => NodeIndex(1)");

        let mut doc = new_doc(b"((key (Variant (foo 1) (bar 2))))");

        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((key (Variant
        5..=8  :    (foo 1)
        9..=15 :    (bar 2))))
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(5), vec![Up(1)]);
        assert_snapshot!(movements, @"Up(1) => NodeIndex(1)");

        let mut doc = new_doc(b"((key (; comment\nVariant (foo 1) (bar 2))))");

        assert_snapshot!(dump(&doc), @r"
         0..=3  : ((key (
         4..=4  :     ; comment
         5..=5  :     Variant
         6..=9  :      (foo 1)
        10..=16 :      (bar 2))))
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(6), vec![Up(1)]);
        // This is fine, but maybe one day we'd want to do something else.
        assert_snapshot!(movements, @"Up(1) => NodeIndex(5)");
    }

    #[test]
    fn moving_across_multiple_top_level_nodes() {
        let mut doc = new_doc(b"((1))atom");

        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((1))
        5..=5  : atom
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(5), vec![Up(1)]);
        assert_snapshot!(movements, @"Up(1) => NodeIndex(0)");
    }

    #[test]
    fn expanding_and_collapsing_nodes() {
        // Basic collapsing of a list
        let mut doc = new_doc(b"(1 2 3)");

        assert_snapshot!(dump(&doc), @r"
        0..=1  : (1
        2..=2  :  2
        3..=4  :  3)
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(2),
            vec![Left, Left, Down(1), Right, Down(1)],
        );
        assert_snapshot!(movements, @r"
        Left =>    NodeIndex(0)
        Left =>    NodeIndex(0) Collapsed(0)
        Down(1) => -
        Right =>   NodeIndex(0) Expanded(0)
        Down(1) => NodeIndex(2)
        ");

        // Basic collapsing of a record
        let mut doc = new_doc(b"((a 1)(b 2))");

        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((a 1)
        5..=9  :  (b 2))
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(5),
            vec![Left, Left, Down(1), Right, Down(1)],
        );
        assert_snapshot!(movements, @r"
        Left =>    NodeIndex(0)
        Left =>    NodeIndex(0) Collapsed(0)
        Down(1) => -
        Right =>   NodeIndex(0) Expanded(0)
        Down(1) => NodeIndex(5)
        ");

        // Basic collapsing of a Variant
        let mut doc = new_doc(b"(Variant 1)");

        assert_snapshot!(dump(&doc), @r"
        0..=1  : (Variant
        2..=3  :   1)
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(2),
            vec![Left, Left, Down(1), Right, Down(1)],
        );
        assert_snapshot!(movements, @r"
        Left =>    NodeIndex(0)
        Left =>    NodeIndex(0) Collapsed(0)
        Down(1) => -
        Right =>   NodeIndex(0) Expanded(0)
        Down(1) => NodeIndex(2)
        ");
    }

    #[test]
    fn collapsing_values_of_records() {
        // Collapsing list value
        let mut doc = new_doc(b"((a 1)(b (2 3)))");

        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((a 1)
        5..=7  :  (b (
        8..=8  :    2
        9..=12 :    3)))
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(9),
            vec![Left, Left, Down(1), Left, Down(1), Right, Down(1)],
        );
        assert_snapshot!(movements, @r"
        Left =>    NodeIndex(5)
        Left =>    NodeIndex(5) Collapsed(7)
        Down(1) => -
        Left =>    NodeIndex(0)
        Down(1) => NodeIndex(5)
        Right =>   NodeIndex(5) Expanded(7)
        Down(1) => NodeIndex(8)
        ");

        // Collapsing variant value
        let mut doc = new_doc(b"((a 1)(b (Variant 2)))");

        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((a 1)
        5..=8  :  (b (Variant
        9..=12 :    2)))
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(9),
            vec![Left, Left, Down(1), Left, Down(1), Right, Down(1)],
        );
        assert_snapshot!(movements, @r"
        Left =>    NodeIndex(5)
        Left =>    NodeIndex(5) Collapsed(7)
        Down(1) => -
        Left =>    NodeIndex(0)
        Down(1) => NodeIndex(5)
        Right =>   NodeIndex(5) Expanded(7)
        Down(1) => NodeIndex(9)
        ");

        // Collapsing singleton list value
        let mut doc = new_doc(b"((a 1)(b ((2 3))))");

        assert_snapshot!(dump(&doc), @r"
         0..=4  : ((a 1)
         5..=8  :  (b ((
         9..=9  :    2
        10..=14 :    3))))
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(10), vec![Left, Left, Left]);
        assert_snapshot!(movements, @r"
        Left => NodeIndex(5)
        Left => NodeIndex(5) Collapsed(7)
        Left => NodeIndex(0)
        ");

        // Collapsing singleton variant value
        let mut doc = new_doc(b"((a 1)(b ((Variant 2))))");

        assert_snapshot!(dump(&doc), @r"
         0..=4  : ((a 1)
         5..=9  :  (b ((Variant
        10..=14 :    2))))
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(10), vec![Left, Left, Left]);
        assert_snapshot!(movements, @r"
        Left => NodeIndex(5)
        Left => NodeIndex(5) Collapsed(7)
        Left => NodeIndex(0)
        ");
    }
}
