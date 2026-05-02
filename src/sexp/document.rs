use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::iter::DoubleEndedIterator;
use std::num::NonZeroUsize;
use std::ops::{Index, Range, RangeInclusive};
use std::rc::Rc;

use crate::dimensions;
use crate::document::{ContentRange, Document};
use crate::rendering::{PreHighlightingStyledSegment, Segment, Text};
use crate::search::InvertedPairedDelimeters;
use crate::sexp::color_scheme::ColorScheme;
use crate::sexp::core::{
    invariants, AtomKind, AtomMetadata, DocCore, DocumentNode, DocumentToken, EndOfListMetadata,
    ListKind, ListMetadata, NodeIndex,
};
use crate::sexp::layout;
use crate::sexp::layout::LogicalLine;
use crate::sexp::renderer;
use crate::sexp::renderer::{style_typeset_line, RenderContext, SegmentKind};

use ocaml_sexplib::tokenizer::{BasicTapeTokenizer, RawTokenTape};
use ocaml_sexplib::Ref;
use wabi_tree::OSBTreeMap;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollapseState {
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
    width: NonZeroUsize,
    tokenizer: BasicTapeTokenizer,
    core: DocCore,
    next_top_level_node_index: NodeIndex,
    starts_of_logical_lines: OSBTreeMap<NodeIndex, (NodeIndex, usize)>,
    collapsible_nodes: BTreeMap<NodeIndex, CollapseState>,
    initial_nested_collapse_state_for_top_level_nodes: InitialNestedCollapseStateForTopLevelNodes,
    // TODO: Probably put this in DocumentViewer; this only exists so I could set
    // it to false in tests and not update a bunch of them.
    include_cursor: bool,
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

#[derive(Clone, Debug)]
pub(super) struct TypesetLine(pub Vec<Segment<NodeIndex, SegmentKind>>);

#[derive(Clone, Debug)]
pub(super) struct TypesetLines(pub Vec<TypesetLine>);

impl TypesetLines {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn index_of_closest_line_to_byte_index(&self, byte_index: usize) -> usize {
        for (i, typeset_line) in self.0.iter().enumerate() {
            for segment in typeset_line.0.iter() {
                if let Text::SourceRange(range) = &segment.content {
                    if byte_index < range.end {
                        return i;
                    }
                }
            }
        }

        self.len() - 1
    }
}

impl Index<usize> for TypesetLines {
    type Output = TypesetLine;

    fn index(&self, index: usize) -> &TypesetLine {
        &self.0[index]
    }
}

#[derive(Clone, Debug)]
pub struct ScreenLine {
    pub logical_line: LogicalLine,
    pub doc_width: NonZeroUsize,
    pub typeset_lines: Rc<TypesetLines>,
    pub index: usize,
}

impl PartialEq for ScreenLine {
    fn eq(&self, other: &Self) -> bool {
        assert!(
            self.doc_width == other.doc_width,
            "eq called on `sexp::ScreenLine`s with different widths"
        );

        self.logical_line == other.logical_line && self.index == other.index
    }
}

impl Eq for ScreenLine {}

impl Ord for ScreenLine {
    fn cmp(&self, other: &Self) -> Ordering {
        assert!(
            self.doc_width == other.doc_width,
            "cmp called on `sexp::ScreenLine`s with different widths"
        );

        match self.logical_line.cmp(&other.logical_line) {
            Ordering::Less => return Ordering::Less,
            Ordering::Greater => return Ordering::Greater,
            Ordering::Equal => self.index.cmp(&other.index),
        }
    }
}

impl PartialOrd for ScreenLine {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl ScreenLine {
    fn next_typeset_line(&self) -> Option<Self> {
        if self.index == self.typeset_lines.len() - 1 {
            return None;
        }

        let mut new = self.clone();
        new.index += 1;
        Some(new)
    }

    fn prev_typeset_line(&self) -> Option<Self> {
        if self.index == 0 {
            return None;
        }

        let mut new = self.clone();
        new.index -= 1;
        Some(new)
    }

    fn into_last_typeset_line(mut self) -> Self {
        self.index = self.typeset_lines.len() - 1;
        self
    }

    fn typeset_line(&self) -> &TypesetLine {
        &self.typeset_lines.0[self.index]
    }
}

impl SexpDocument {
    fn process_additional_data(&mut self, current_data: Option<&[u8]>, seen_eof: bool) {
        while let Some(witness) = self.tokenizer.has_enough_data_to_produce_tokens() {
            let current_data = current_data.map(|b| Ref::Transient(b));
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

        // Add data any new top level nodes.
        let Some(last_completed_top_level_sexp) =
            self.core.node_index_of_last_completed_top_level_sexp
        else {
            // No data yet
            return;
        };

        let initial_collapse_state = self
            .initial_nested_collapse_state_for_top_level_nodes
            .eventual_collapse_state;

        while self.next_top_level_node_index <= last_completed_top_level_sexp {
            let new_top_level_node_index = self.next_top_level_node_index;

            let logical_lines =
                layout::layout_fully_expanded_node(&self.core, new_top_level_node_index);

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
                        // Technically collapsible if it ends up across multiple lines
                        | ListKind::Singleton => (),
                    }

                    let start_of_end_logical_line =
                        self.logical_line_of_node_index(list_end_index).start_index;

                    if start_of_end_logical_line
                        != start_index_of_end_line_of_previous_collapsible_node
                    {
                        self.collapsible_nodes
                            .insert(node_index, initial_collapse_state);
                        start_index_of_end_line_of_previous_collapsible_node =
                            start_of_end_logical_line;
                    }
                }
            }

            // Now apply any shallow collapsing/expanding that have been applied
            // to top-level nodes previously.
            if matches!(
                self.core.token(new_top_level_node_index),
                DocumentToken::StartOfList(_)
            ) {
                let first_child_index = new_top_level_node_index + 1;
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

                // If this is none, then a single deep collapse state was set, and used
                // as the default, so the top-level node is already in the desired state.
                if let Some(collapse_state) = most_shallow_collapse_state {
                    if let Some(state) = self.collapsible_nodes.get_mut(&new_top_level_node_index) {
                        *state = collapse_state;
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

    fn first_normal_focusable_node_to_left_of_node_or_node(
        &self,
        node_index: NodeIndex,
    ) -> NodeIndex {
        let logical_line = self.logical_line_of_node_index(node_index);

        let mut focusable_nodes = self.focusable_nodes_in_line(&logical_line);
        focusable_nodes.retain(|(focusable_node_index, _)| *focusable_node_index <= node_index);

        match Self::last_normal_focusable_node_or_last_node(focusable_nodes) {
            Some(focusable_node_index) => focusable_node_index,
            None => node_index,
        }
    }

    // When we're focused on record fields and variants, the cursor points to the list, but we
    // really consider the record key / constructor as currently focused as well. And for DateTimes
    // we also consider the whole list as the cursor.
    //
    // This returns the range of node indexes that we consider as currently being part of the cursor.
    fn nodes_considered_as_part_of_cursor(
        &self,
        node_index: NodeIndex,
    ) -> RangeInclusive<NodeIndex> {
        let logical_line = self.logical_line_of_node_index(node_index);

        match self.core.token(node_index) {
            // When we're focused on non-lists, we don't consider any other nodes as part of the
            // cursor.
            DocumentToken::Atom(_)
            | DocumentToken::Unit { .. }
            | DocumentToken::LineComment
            | DocumentToken::BlockComment
            | DocumentToken::Error(_)
            | DocumentToken::EndOfList(_) => node_index..=node_index,
            DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => match list_kind {
                ListKind::Record | ListKind::Singleton | ListKind::Plain => node_index..=node_index,
                ListKind::RecordField | ListKind::VariantRecord | ListKind::VariantTuple => {
                    let atom_node_index = node_index + 1;
                    if matches!(self.core.token(atom_node_index), DocumentToken::Atom(_))
                        && logical_line.contains_node_index(atom_node_index)
                    {
                        node_index..=atom_node_index
                    } else {
                        node_index..=node_index
                    }
                }
                ListKind::DateTime => {
                    // DateTimes can't be interrupted by comments or anything else.
                    node_index..=(node_index + 3)
                }
            },
        }
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

    fn closest_visible_ancestor(&self, cursor: &NodeIndex) -> NodeIndex {
        let mut closest_visible = *cursor;
        let mut curr = *cursor;

        while let Some(parent_index) = self.core.node(curr).parent_index {
            match self.collapsible_nodes.get(&parent_index) {
                Some(Collapsed) => closest_visible = parent_index,
                None | Some(Expanded) => (),
            }
            curr = parent_index;
        }

        closest_visible
    }

    fn first_visible_line_at_or_above(&self, logical_line: LogicalLine) -> LogicalLine {
        let visible_ancestor = self.closest_visible_ancestor(&logical_line.start_index);
        return self.logical_line_of_node_index(visible_ancestor);
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

    fn move_left_impl(
        &mut self,
        cursor: &NodeIndex,
        should_actually_collapse: bool,
    ) -> Option<NodeIndex> {
        let current_line = self.logical_line_of_node_index(*cursor);

        // This function is used for two actions:
        // - CollapseOrMoveCursorLeftOrUp
        // - MoveCursorLeftOrUpWithoutCollapsing
        //
        // The name of the first gives us our priorities:
        //
        // - Collapse something,
        // - Or, move the cursor left on the current line if possible
        // - Otherwise, if the focused node has a parent, try to move there (some weird
        // cases here, see below.)
        //
        // For the second action, we implement the same logic, but just skip collapsing.

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

            if should_collapse && should_actually_collapse {
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

    fn set_collapse_state_on_node_and_siblings(
        &mut self,
        node_index: NodeIndex,
        depth: Option<usize>,
        desired_state: CollapseState,
    ) -> NodeIndex {
        // If we're updating the collapse state of all the top level nodes, we need to update
        // our initial state to apply to new top-level nodes that stream in.
        if self.core.node(node_index).parent_index.is_none() {
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
                let first_child = match self.core.node(node_index).parent_index {
                    None => NodeIndex(0),
                    Some(parent_index) => parent_index + 1,
                };

                self.set_collapse_state_on_first_child_and_siblings(first_child, n, desired_state);
            }
        }

        // If the user was somehow focused on the closing paren of a node, and that node is now
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
        let range = match self.core.node(node_index).parent_index {
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

            next_sibling = self.core.node(sibling_index).next_sibling;
        }
    }

    pub fn render_context_with_color_scheme<'a>(
        &'a self,
        color_scheme: &'a ColorScheme,
        focus: NodeIndex,
    ) -> RenderContext<'a> {
        RenderContext::new(&color_scheme, &self.core, &self.collapsible_nodes, focus)
    }

    pub fn typeset_logical_line(&self, logical_line: &LogicalLine) -> TypesetLines {
        renderer::typeset_logical_line(
            &logical_line,
            self.width,
            &self.core,
            &self.collapsible_nodes,
            self.include_cursor,
        )
    }

    fn first_typeset_screen_line_for_logical_line(&self, logical_line: LogicalLine) -> ScreenLine {
        let typeset_lines = Rc::new(self.typeset_logical_line(&logical_line));

        ScreenLine {
            logical_line,
            doc_width: self.width,
            typeset_lines,
            index: 0,
        }
    }

    fn last_typeset_screen_line_for_logical_line(&self, logical_line: LogicalLine) -> ScreenLine {
        let screen_line = self.first_typeset_screen_line_for_logical_line(logical_line);
        screen_line.into_last_typeset_line()
    }
}

impl Document for SexpDocument {
    type Cursor = NodeIndex;
    type ScreenLine = ScreenLine;

    fn new() -> Self {
        SexpDocument {
            width: dimensions::DEFAULT_WIDTH,
            tokenizer: BasicTapeTokenizer::new(),
            core: DocCore::new(),
            next_top_level_node_index: NodeIndex(0),
            starts_of_logical_lines: OSBTreeMap::new(),
            collapsible_nodes: BTreeMap::new(),
            initial_nested_collapse_state_for_top_level_nodes:
                InitialNestedCollapseStateForTopLevelNodes::new(),
            include_cursor: if cfg!(test) { false } else { true },
        }
    }

    fn width(&self) -> usize {
        self.width.get()
    }

    fn resize(&mut self, new_width: NonZeroUsize) {
        self.width = new_width;
        // TODO: Handle resizing
    }

    fn append(&mut self, data: &[u8]) {
        self.tokenizer.feed_more_data(data);
        self.process_additional_data(Some(data), false);
    }

    fn eof(&mut self) {
        self.tokenizer.eof();
        self.process_additional_data(None, true);
    }

    fn top_screen_line_and_cursor(&self) -> Option<(ScreenLine, Self::Cursor)> {
        match self.starts_of_logical_lines.first_key_value() {
            None => None,
            Some((start_index, (end_index, indentation))) => Some((
                self.first_typeset_screen_line_for_logical_line(LogicalLine {
                    indentation: *indentation,
                    start_index: *start_index,
                    end_index: *end_index,
                }),
                *start_index,
            )),
        }
    }

    fn bottom_screen_line_and_cursor(&self) -> Option<(ScreenLine, Self::Cursor)> {
        match self.starts_of_logical_lines.last_key_value() {
            None => None,
            Some((start_index, (end_index, indentation))) => {
                let last_logical_line = LogicalLine {
                    indentation: *indentation,
                    start_index: *start_index,
                    end_index: *end_index,
                };

                let last_visible_logical_line =
                    self.first_visible_line_at_or_above(last_logical_line);

                let cursor = last_visible_logical_line.start_index;
                let last_screen_line =
                    self.last_typeset_screen_line_for_logical_line(last_visible_logical_line);

                Some((last_screen_line, cursor))
            }
        }
    }

    fn first_visible_cursor_at_or_before_line_index(&self, index: usize) -> Option<Self::Cursor> {
        let (start_index, (end_index, indentation)) =
            match self.starts_of_logical_lines.get_by_rank(index) {
                None => self.starts_of_logical_lines.last_key_value()?,
                Some(x) => x,
            };

        let line_at_index = LogicalLine {
            indentation: *indentation,
            start_index: *start_index,
            end_index: *end_index,
        };

        let visible_line_at_or_before_index = self.first_visible_line_at_or_above(line_at_index);

        Some(visible_line_at_or_before_index.start_index)
    }

    fn next_screen_line(&self, screen_line: &ScreenLine) -> Option<ScreenLine> {
        if let Some(next_typeset_line) = screen_line.next_typeset_line() {
            Some(next_typeset_line)
        } else {
            let next_visible_logical_line =
                self.next_visible_logical_line(&screen_line.logical_line)?;
            Some(self.first_typeset_screen_line_for_logical_line(next_visible_logical_line))
        }
    }

    fn prev_screen_line(&self, screen_line: &ScreenLine) -> Option<ScreenLine> {
        if let Some(prev_typeset_line) = screen_line.prev_typeset_line() {
            Some(prev_typeset_line)
        } else {
            let prev_visible_logical_line =
                self.prev_visible_logical_line(&screen_line.logical_line)?;
            Some(self.last_typeset_screen_line_for_logical_line(prev_visible_logical_line))
        }
    }

    fn line_number(&self, screen_line: &ScreenLine) -> usize {
        1 + self
            .starts_of_logical_lines
            .rank_of(&screen_line.logical_line.start_index)
            .expect("to find logical line start in `starts_of_logical_lines`")
    }

    fn num_lines(&self) -> usize {
        self.starts_of_logical_lines.len()
    }

    fn is_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        screen_line.typeset_lines.len() > 1
    }

    fn is_start_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        self.is_wrapped_line(screen_line) && screen_line.index == 0
    }

    fn is_end_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        self.is_wrapped_line(screen_line)
            && screen_line.index == screen_line.typeset_lines.len() - 1
    }

    fn is_after_start_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        screen_line.index > 0
    }

    fn is_before_end_of_wrapped_line(&self, screen_line: &ScreenLine) -> bool {
        self.is_wrapped_line(screen_line) && !self.is_end_of_wrapped_line(screen_line)
    }

    fn cursor_range(&self, cursor: &NodeIndex) -> ContentRange<ScreenLine> {
        let logical_line = self.logical_line_of_node_index(*cursor);
        let first_screen_line = self.first_typeset_screen_line_for_logical_line(logical_line);
        let last_screen_line = first_screen_line.clone().into_last_typeset_line();
        let num_screen_lines = first_screen_line.typeset_lines.len();

        ContentRange {
            start: first_screen_line,
            end: last_screen_line,
            num_screen_lines,
        }
    }

    fn convert_screen_line_to_cursor(
        &self,
        screen_line: ScreenLine,
        _prev_cursor: &NodeIndex,
    ) -> NodeIndex {
        // TODO: SCREEN LINE FIX THIS COULD BE IMPROVED.
        let fallback = screen_line.logical_line.start_index;
        screen_line
            .typeset_line()
            .0
            .iter()
            .find_map(|segment| segment.doc_ref)
            .unwrap_or(fallback)
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
        let should_collapse = true;
        self.move_left_impl(cursor, should_collapse)
    }

    fn move_cursor_left_or_up_without_collapsing(
        &mut self,
        cursor: &NodeIndex,
    ) -> Option<NodeIndex> {
        let should_collapse = false;
        self.move_left_impl(cursor, should_collapse)
    }

    fn move_cursor_to_first_sibling(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let Some(parent_index) = self.core.node(*cursor).parent_index else {
            // If we're focused on a top level sexp, we'll move to the first one.
            return Some(NodeIndex(0));
        };

        let DocumentToken::StartOfList(parent_list_metadata) = self.core.token(parent_index) else {
            panic!("parent_index didn't point to StartOfList");
        };

        let first_child = parent_index + 1;

        match parent_list_metadata.list_kind {
            ListKind::Record | ListKind::DateTime | ListKind::Singleton | ListKind::Plain => {
                Some(first_child)
            }
            ListKind::RecordField => {
                // This can only happen if the user deliberately hits right on a record field
                // to focus a variant constructor or nested singleton list value. Taking them
                // to the first child would take the to the record key alone, which is weird,
                // so we'll focus the actual parent instead.
                Some(parent_index)
            }
            ListKind::VariantRecord | ListKind::VariantTuple => {
                // For variants, we actually want to focus the first thing after the constructor.
                invariants::constructors_are_the_first_child_of_variants();
                self.core.node(first_child).next_sibling
            }
        }
    }

    fn move_cursor_to_last_sibling(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let Some(parent_index) = self.core.node(*cursor).parent_index else {
            // If we're focused on a top level sexp, we'll move to the last one.
            return self.core.node_index_of_last_completed_top_level_sexp;
        };

        let DocumentToken::StartOfList(list_metadata) = self.core.token(parent_index) else {
            panic!("parent_index didn't point to StartOfList");
        };

        list_metadata.last_child_index()
    }

    fn collapse_node_and_siblings(
        &mut self,
        cursor: &NodeIndex,
        depth: Option<usize>,
    ) -> Option<Self::Cursor> {
        Some(self.set_collapse_state_on_node_and_siblings(*cursor, depth, Collapsed))
    }

    fn expand_node_and_siblings(
        &mut self,
        cursor: &NodeIndex,
        depth: Option<usize>,
    ) -> Option<Self::Cursor> {
        Some(self.set_collapse_state_on_node_and_siblings(*cursor, depth, Expanded))
    }

    fn path_to_cursor(&self, cursor: &NodeIndex) -> Option<String> {
        self.core.sexp_get_style_path_to_node(*cursor)
    }

    fn debug_text_content(&self, screen_line: &ScreenLine, cursor: &NodeIndex) -> Vec<u8> {
        use unicode_segmentation::UnicodeSegmentation;

        let mut output = String::new();

        let mut highlighted_cursor = false;

        for segment in screen_line.typeset_line().0.iter() {
            match &segment.content {
                Text::SourceRange(range) => {
                    let content = std::str::from_utf8(&self.core.pretty_printed[range.clone()])
                        .unwrap_or("INVALID UTF8");

                    if !highlighted_cursor && segment.doc_ref == Some(*cursor) {
                        if content.starts_with("(") {
                            output.push('[');
                            output.push_str(&content[1..]);
                        } else {
                            output.push('*');
                            let first_grapheme_len = content
                                .graphemes(true)
                                .next()
                                .expect("there to be at least one grapheme")
                                .len();
                            output.push_str(&content[first_grapheme_len..]);
                        }
                        highlighted_cursor = true;
                    } else {
                        output.push_str(content);
                    }
                }
                Text::String((s, range)) => {
                    output.push_str(&s[range.clone()]);
                }
                Text::Static(s) => {
                    output.push_str(s);
                }
            }
        }

        output.into_bytes()
    }

    fn render_screen_line(
        &self,
        screen_line: &ScreenLine,
        cursor: &Self::Cursor,
    ) -> Option<Vec<PreHighlightingStyledSegment>> {
        let color_scheme = ColorScheme::default();
        let render_context = self.render_context_with_color_scheme(&color_scheme, *cursor);
        Some(style_typeset_line(
            &render_context,
            &screen_line.logical_line,
            screen_line.typeset_line(),
        ))
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
        let nodes = self.nodes_considered_as_part_of_cursor(*cursor);
        let start = self.core.node(*nodes.start()).data_range.start;
        let end = self.core.node(*nodes.end()).data_range.end;
        start..end
    }

    fn raw_byte_index_to_cursor(&self, byte_index: usize) -> NodeIndex {
        let closest_node_to_byte_index = self.core.closest_node_to_byte_index(byte_index);
        self.first_normal_focusable_node_to_left_of_node_or_node(closest_node_to_byte_index)
    }

    fn raw_byte_index_to_visible_screen_line(&self, byte_index: usize) -> ScreenLine {
        let closest_node_to_byte_index = self.core.closest_node_to_byte_index(byte_index);
        let closest_visible_ancestor = self.closest_visible_ancestor(&closest_node_to_byte_index);
        let logical_line = self.logical_line_of_node_index(closest_visible_ancestor);
        let typeset_lines = self.typeset_logical_line(&logical_line);
        let closest_index = typeset_lines.index_of_closest_line_to_byte_index(byte_index);

        ScreenLine {
            logical_line,
            doc_width: self.width,
            typeset_lines: Rc::new(typeset_lines),
            index: closest_index,
        }
    }

    fn is_raw_byte_range_visible(&self, byte_range: Range<usize>) -> bool {
        // This is fine for now; but might want to revisit later based on how previews
        // for collapsed nodes are displayed. Right now, if you have a record field where
        // the value is a collapsed variant (e.g. "(key (Variant ...))"), then we'll say
        // that the range representing "Variant" is visible.
        //
        // This implementation also completely ignores the end of the range, so it doesn't
        // handle searches that span multiple nodes.
        //
        // To handle especially pathological cases, we need to consider all they bytes
        // (or, more likely, NodeIndexes) in between the start and the end. Consider a
        // range that starts in one collapsed value, extends to a visible value, then ends
        // in another collapsed value.
        let cursor = self.raw_byte_index_to_cursor(byte_range.start);
        self.closest_visible_cursor(&cursor) == cursor
    }

    fn closest_visible_cursor(&self, cursor: &NodeIndex) -> NodeIndex {
        let closest_visible_ancestor = self.closest_visible_ancestor(cursor);
        self.first_normal_focusable_node_to_left_of_node_or_node(closest_visible_ancestor)
    }
}

#[cfg(test)]
pub(super) mod test_helpers {
    use super::*;

    use crate::document::Document;

    use std::fmt::Write;

    use bstr::ByteSlice;

    const FAR_AWAY_CURSOR: NodeIndex = NodeIndex(usize::MAX);

    pub fn nz(n: usize) -> NonZeroUsize {
        NonZeroUsize::new(n).unwrap()
    }

    pub fn new_partial_doc(bytes: &'static [u8]) -> SexpDocument {
        let mut doc = SexpDocument::new();
        doc.resize(nz(100));
        doc.append(bytes);
        doc
    }

    pub fn new_doc(bytes: &'static [u8]) -> SexpDocument {
        let mut doc = new_partial_doc(bytes);
        doc.eof();
        doc
    }

    pub fn logical_lines(doc: &SexpDocument) -> Vec<LogicalLine> {
        doc.starts_of_logical_lines
            .iter()
            .map(|(start_index, (end_index, indentation))| LogicalLine {
                indentation: *indentation,
                start_index: *start_index,
                end_index: *end_index,
            })
            .collect()
    }

    pub fn dump(doc: &SexpDocument) -> String {
        let logical_lines = logical_lines(doc);
        crate::sexp::layout::tests::show_logical_lines(&doc.core, logical_lines)
    }

    pub fn dump_with_byte_indexes(doc: &SexpDocument) -> String {
        let logical_lines = logical_lines(doc);
        crate::sexp::layout::tests::show_logical_lines_with_byte_indexes(&doc.core, logical_lines)
    }

    pub fn show_visible_lines(doc: &SexpDocument) -> String {
        let Some((top_screen_line, _)) = doc.top_screen_line_and_cursor() else {
            return "".to_string();
        };

        let mut output = String::new();

        let mut next_visible_line = Some(top_screen_line);

        while let Some(visible_line) = &next_visible_line {
            let _ = writeln!(
                output,
                "{}",
                doc.debug_text_content(visible_line, &FAR_AWAY_CURSOR)
                    .as_bstr()
            );

            next_visible_line = doc.next_screen_line(visible_line);
        }

        output
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::*;
    use super::*;

    use crate::document::Document;
    use crate::sexp::core::{invariants, NodeIndex};

    use std::fmt::Write;

    use bstr::ByteSlice;
    use insta::{allow_duplicates, assert_debug_snapshot, assert_snapshot};

    #[test]
    fn add_new_top_level_nodes_as_they_are_available() {
        let mut doc = SexpDocument::new();
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

    #[test]
    fn handle_tokenization_errors_in_doc() {
        let mut doc = SexpDocument::new();

        doc.append(b"a |# b");
        assert_snapshot!(show_visible_lines(&doc), @r"
        a
        TokenizationError(UnexpectedEndOfBlockComment)
        ");

        // Someday: The `BasicTapeTokenizer` doesn't handle receiving more input
        // after tokenization errors very well, so we get this awkward and
        // misleading `EofCalledMultipleTimes` error.
        doc.eof();
        assert_snapshot!(show_visible_lines(&doc), @r"
        a
        TokenizationError(UnexpectedEndOfBlockComment)
        TokenizationError(EofCalledMultipleTimes)
        ");
    }

    #[test]
    fn handle_tokenization_errors_at_eof() {
        let mut doc = SexpDocument::new();
        doc.append(b"(\"a b");
        assert_snapshot!(show_visible_lines(&doc), @"");

        doc.eof();
        assert_snapshot!(show_visible_lines(&doc), @r"
        (
         TokenizationError(UnexpectedEofWhileInInQuotedAtom)
         Unexpected EOF while parsing list
        ");

        let mut doc = SexpDocument::new();
        doc.append(b"#| a");
        assert_snapshot!(show_visible_lines(&doc), @"");

        doc.eof();
        assert_snapshot!(show_visible_lines(&doc), @"TokenizationError(UnexpectedEofWhileInBlockComment)");
    }

    #[derive(Copy, Clone, Debug)]
    enum Action {
        Down(usize),
        Up(usize),
        Right,
        Left,
        LeftNoCollapse,
        FirstSibling,
        LastSibling,
        FocusBottom,
        Collapse(Option<usize>),
        Expand(Option<usize>),
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
                LeftNoCollapse => self.move_cursor_left_or_up_without_collapsing(&current_cursor),
                FirstSibling => self.move_cursor_to_first_sibling(&current_cursor),
                LastSibling => self.move_cursor_to_last_sibling(&current_cursor),
                FocusBottom => self
                    .bottom_screen_line_and_cursor()
                    .map(|(_, cursor)| cursor),
                Collapse(depth) => self.collapse_node_and_siblings(&current_cursor, depth),
                Expand(depth) => self.expand_node_and_siblings(&current_cursor, depth),
            }
        }
    }

    fn perform_action_and_compute_collapse_state_changes(
        doc: &mut SexpDocument,
        current_cursor: NodeIndex,
        action: Action,
    ) -> (Option<NodeIndex>, String) {
        let prev_collapsed_states = doc.collapsible_nodes.clone();
        let new_cursor = doc.perform_action(current_cursor, action);
        let new_collapsed_states = doc.collapsible_nodes.clone();

        assert_eq!(prev_collapsed_states.len(), new_collapsed_states.len());

        let mut changes = vec![];
        for (node_index, collapsed_state) in prev_collapsed_states.iter() {
            let new_collapsed_state = new_collapsed_states.get(node_index).unwrap();
            if collapsed_state != new_collapsed_state {
                let change = format!("{:?}({})", new_collapsed_state, node_index.0);
                changes.push(change);
            }
        }

        (new_cursor, changes.join(" "))
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
            let (new_cursor, collapsed_or_expanded_nodes) =
                perform_action_and_compute_collapse_state_changes(doc, current_cursor, action);

            let action = format!("{:?} =>", action);

            let mut result = if let Some(new_cursor) = new_cursor {
                current_cursor = new_cursor;
                format!("{:?}", new_cursor)
            } else {
                "-".to_string()
            };

            if !collapsed_or_expanded_nodes.is_empty() {
                let _ = write!(result, " {}", collapsed_or_expanded_nodes);
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

        // This is not treated as a variant because there is a comment before the constructor.
        invariants::constructors_are_the_first_child_of_variants();
        let mut doc = new_doc(b"((key (; comment\nVariant (foo 1) (bar 2))))");

        assert_snapshot!(dump(&doc), @r"
         0..=3  : ((key (
         4..=4  :    ; comment
         5..=5  :    Variant
         6..=9  :    (foo 1)
        10..=16 :    (bar 2))))
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
    fn test_expanding_and_collapsing_nodes() {
        // Basic collapsing of a list
        let mut doc = new_doc(b"(1 2 3)");

        assert_snapshot!(dump(&doc), @r"
        0..=1  : (1
        2..=2  :  2
        3..=4  :  3)
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(2), vec![LeftNoCollapse, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        LeftNoCollapse => NodeIndex(0)
        LeftNoCollapse => -
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

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(5), vec![LeftNoCollapse, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        LeftNoCollapse => NodeIndex(0)
        LeftNoCollapse => -
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
        let mut doc = new_doc(b"(Variant 1 2)");

        assert_snapshot!(dump(&doc), @r"
        0..=1  : (Variant
        2..=2  :   1
        3..=4  :   2)
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(2), vec![LeftNoCollapse, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        LeftNoCollapse => NodeIndex(0)
        LeftNoCollapse => -
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
    fn test_collapsing_values_of_records() {
        // Collapsing list value
        let mut doc = new_doc(b"((a 1)(b (2 3)))");

        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((a 1)
        5..=7  :  (b (
        8..=8  :    2
        9..=12 :    3)))
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(9), vec![LeftNoCollapse, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        LeftNoCollapse => NodeIndex(5)
        LeftNoCollapse => NodeIndex(0)
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
        let mut doc = new_doc(b"((a 1)(b (Variant 2 3)))");

        assert_snapshot!(dump(&doc), @r"
         0..=4  : ((a 1)
         5..=8  :  (b (Variant
         9..=9  :    2
        10..=13 :    3)))
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(9), vec![LeftNoCollapse, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        LeftNoCollapse => NodeIndex(5)
        LeftNoCollapse => NodeIndex(0)
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

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(10),
            vec![LeftNoCollapse, LeftNoCollapse, LeftNoCollapse],
        );
        assert_snapshot!(movements, @r"
        LeftNoCollapse => NodeIndex(5)
        LeftNoCollapse => NodeIndex(0)
        LeftNoCollapse => -
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(10), vec![Left, Left, Left]);
        assert_snapshot!(movements, @r"
        Left => NodeIndex(5)
        Left => NodeIndex(5) Collapsed(7)
        Left => NodeIndex(0)
        ");

        // Collapsing singleton variant value
        let mut doc = new_doc(b"((a 1)(b ((Variant 2 3))))");

        assert_snapshot!(dump(&doc), @r"
         0..=4  : ((a 1)
         5..=9  :  (b ((Variant
        10..=10 :    2
        11..=15 :    3))))
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(10),
            vec![LeftNoCollapse, LeftNoCollapse, LeftNoCollapse],
        );
        assert_snapshot!(movements, @r"
        LeftNoCollapse => NodeIndex(5)
        LeftNoCollapse => NodeIndex(0)
        LeftNoCollapse => -
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(10), vec![Left, Left, Left]);
        assert_snapshot!(movements, @r"
        Left => NodeIndex(5)
        Left => NodeIndex(5) Collapsed(7)
        Left => NodeIndex(0)
        ");
    }

    #[test]
    fn test_first_visible_cursor_at_or_before_line_index() {
        let mut doc = new_doc(b"((0 1 2)(3 (4 5))(6 7 8))");
        assert_snapshot!(dump(&doc), @r"
         0..=2  : ((0
         3..=3  :   1
         4..=5  :   2)
         6..=7  :  (3
         8..=9  :   (4
        10..=12 :    5))
        13..=14 :  (6
        15..=15 :   7
        16..=18 :   8))
        ");

        let f = |doc: &mut SexpDocument, index| {
            doc.first_visible_cursor_at_or_before_line_index(index)
                .unwrap()
                .0
        };

        assert_debug_snapshot!(f(&mut doc, 0), @"0");
        assert_debug_snapshot!(f(&mut doc, 3), @"6");
        assert_debug_snapshot!(f(&mut doc, 5), @"10");
        assert_debug_snapshot!(f(&mut doc, 8), @"16");
        assert_debug_snapshot!(f(&mut doc, 9), @"16");
        assert_debug_snapshot!(f(&mut doc, 100), @"16");

        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(6));
        assert_debug_snapshot!(f(&mut doc, 3), @"6");
        assert_debug_snapshot!(f(&mut doc, 5), @"6");

        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(13));
        assert_debug_snapshot!(f(&mut doc, 100), @"13");

        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(0));
        assert_debug_snapshot!(f(&mut doc, 100), @"0");
    }

    #[test]
    fn test_focusing_first_and_last_siblings() {
        let mut doc = new_doc(b"((k1 a)(k2 b)(k3 (Variant 1 2 3))) x y z");
        assert_snapshot!(dump(&doc), @r"
         0..=4  : ((k1 a)
         5..=8  :  (k2 b)
         9..=12 :  (k3 (Variant
        13..=13 :    1
        14..=14 :    2
        15..=18 :    3)))
        19..=19 : x
        20..=20 : y
        21..=21 : z
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(5), vec![LastSibling, FirstSibling]);
        assert_snapshot!(movements, @r"
        LastSibling =>  NodeIndex(9)
        FirstSibling => NodeIndex(1)
        ");

        // First sibling on variants goes to the first argument, not to the constructor
        let movements = show_cursor_movements(&mut doc, NodeIndex(14), vec![FirstSibling]);
        assert_snapshot!(movements, @"FirstSibling => NodeIndex(13)");

        // First sibling on the value of a record field goes the record field, not the record key.
        let movements = show_cursor_movements(&mut doc, NodeIndex(11), vec![FirstSibling]);
        assert_snapshot!(movements, @"FirstSibling => NodeIndex(9)");

        // Move between top-level nodes
        let movements =
            show_cursor_movements(&mut doc, NodeIndex(19), vec![FirstSibling, LastSibling]);
        assert_snapshot!(movements, @r"
        FirstSibling => NodeIndex(0)
        LastSibling =>  NodeIndex(21)
        ");
    }

    #[test]
    fn test_collapsing_and_expanding_nodes_and_siblings() {
        let mut doc = new_doc(
            b"((a 1)(b ((cc 3)(dd 4)(ee ((fff 5)(ggg 6)))))(h ((ii 7)(jj 8))))((w 1)(xx yy zz))",
        );
        assert_snapshot!(dump(&doc), @r"
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

        let mut go = |node_index, action| {
            let (new_index, mut result) = perform_action_and_compute_collapse_state_changes(
                &mut doc,
                NodeIndex(node_index),
                action,
            );

            match new_index {
                Some(new_index) if new_index.0 == node_index => (),
                _ => {
                    let _ = write!(result, " (returned {new_index:?})");
                }
            }

            result
        };

        assert_snapshot!(go(0, Collapse(Some(1))), @"Collapsed(0) Collapsed(45)");
        assert_snapshot!(go(0, Collapse(Some(2))), @"Collapsed(7) Collapsed(33) Collapsed(50)");
        assert_snapshot!(go(1, Collapse(None)), @"Collapsed(18)");

        assert_snapshot!(go(1, Expand(None)), @"Expanded(7) Expanded(18) Expanded(33)");
        assert_snapshot!(go(0, Expand(None)), @"Expanded(0) Expanded(45) Expanded(50)");

        // Switch to start of list if on end and it gets collapsed.
        assert_snapshot!(go(55, Collapse(Some(1))), @"Collapsed(0) Collapsed(45) (returned Some(NodeIndex(45)))");
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
        fn check(actions: Vec<Action>) -> String {
            allow_duplicates! {
                let mut doc = new_partial_doc(b"start ");
                for action in actions.into_iter() {
                    doc.perform_action(NodeIndex(0), action);
                }

                doc.append(b"((1 (4 (6 (8 _)))))");
                assert_snapshot!(dump(&doc), @r"
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
            check(vec![Collapse(None)]),
            @r#""1 => Collapsed, 4 => Collapsed, 6 => Collapsed, 8 => Collapsed""#,
        );

        assert_snapshot!(
            check(vec![Collapse(None), Expand(Some(1))]),
            @r#""1 => Expanded, 4 => Collapsed, 6 => Collapsed, 8 => Collapsed""#,
        );

        assert_snapshot!(
            check(vec![Collapse(None), Expand(Some(3)), Collapse(Some(2)), Expand(Some(1))]),
            @r#""1 => Expanded, 4 => Collapsed, 6 => Expanded, 8 => Collapsed""#,
        );
    }

    #[test]
    fn test_converting_between_raw_bytes_and_cursors() {
        let doc = new_doc(b"((aa 11)(bb (Var1 22 33))(cc (44 (Var2 (dd 55)) (Var3 66))))");
        assert_snapshot!(dump_with_byte_indexes(&doc), @r"
         0..=4  :   0..8   : ((aa 11)
         5..=8  :   9..18  :  (bb (Var1
         9..=9  :  19..21  :    22
        10..=12 :  22..26  :    33))
        13..=15 :  27..32  :  (cc (
        16..=16 :  32..34  :    44
        17..=18 :  35..40  :    (Var2
        19..=23 :  41..49  :      (dd 55))
        24..=30 :  50..62  :    (Var3 66))))
        ");

        assert_debug_snapshot!(doc.raw_byte_range_of_cursor(&NodeIndex(0)), @"0..1");
        assert_eq!(doc.raw_byte_index_to_cursor(0), NodeIndex(0));
        assert_eq!(doc.raw_byte_index_to_cursor(1), NodeIndex(1));

        // Record fields include the key
        assert_debug_snapshot!(doc.raw_byte_range_of_cursor(&NodeIndex(5)), @"9..12");
        assert_eq!(doc.raw_byte_index_to_cursor(8), NodeIndex(5));
        assert_eq!(doc.raw_byte_index_to_cursor(9), NodeIndex(5));
        assert_eq!(doc.raw_byte_index_to_cursor(12), NodeIndex(5));

        assert_debug_snapshot!(doc.raw_byte_range_of_cursor(&NodeIndex(6)), @"10..12");

        assert_debug_snapshot!(doc.raw_byte_range_of_cursor(&NodeIndex(8)), @"14..18");
        // Even though "(Var1" is a focusable node, we go back to the normal focusable
        // node, the record field.
        assert_eq!(doc.raw_byte_index_to_cursor(17), NodeIndex(5));
        assert_eq!(doc.raw_byte_index_to_cursor(18), NodeIndex(9));

        assert_debug_snapshot!(doc.raw_byte_range_of_cursor(&NodeIndex(9)), @"19..21");
        assert_eq!(doc.raw_byte_index_to_cursor(20), NodeIndex(9));

        // Variant tuples and records include their constructor
        assert_debug_snapshot!(doc.raw_byte_range_of_cursor(&NodeIndex(17)), @"35..40");
        assert_eq!(doc.raw_byte_index_to_cursor(37), NodeIndex(17));
        assert_debug_snapshot!(doc.raw_byte_range_of_cursor(&NodeIndex(24)), @"50..55");
        assert_eq!(doc.raw_byte_index_to_cursor(52), NodeIndex(24));

        // Trailing parens at the end of the sexp
        assert_eq!(doc.raw_byte_index_to_cursor(60), NodeIndex(26));
    }

    #[test]
    fn test_converting_between_raw_byte_indexes_and_visible_cursors_and_screen_lines() {
        let mut doc = new_doc(
            b"(((aa 11)(bb (Var1 (x 22)))(cc (33 (Var2 (dd 44)) (Var3 (y 55)))))
            ((xx false)(yy ())(zz \"\")))",
        );
        assert_snapshot!(dump_with_byte_indexes(&doc), @r#"
         0..=5  :   0..9   : (((aa 11)
         6..=9  :  10..19  :   (bb (Var1
        10..=15 :  20..28  :     (x 22)))
        16..=18 :  29..34  :   (cc (
        19..=19 :  34..36  :     33
        20..=21 :  37..42  :     (Var2
        22..=26 :  43..51  :       (dd 44))
        27..=28 :  52..57  :     (Var3
        29..=36 :  58..68  :       (y 55)))))
        37..=41 :  69..80  :  ((xx false)
        42..=45 :  81..88  :   (yy ())
        46..=51 :  89..98  :   (zz "")))
        "#);

        fn raw_byte_index_to_visible_screen_line(doc: &SexpDocument, index: usize) -> String {
            let LogicalLine {
                start_index,
                end_index,
                ..
            } = doc
                .raw_byte_index_to_visible_screen_line(index)
                .logical_line;

            format!("{}..={}", start_index.0, end_index.0)
        }

        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 0), @"0..=5");
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 15), @"6..=9");

        // Byte index of the "55".
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 61), @"29..=36");

        // Collapse "(Var3 ....)"
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(27));
        assert_eq!(doc.closest_visible_cursor(&NodeIndex(29)), NodeIndex(27));
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 61), @"27..=28");

        // Collapse "(cc ....)"
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(16));
        assert_eq!(doc.closest_visible_ancestor(&NodeIndex(29)), NodeIndex(18));
        assert_eq!(doc.closest_visible_cursor(&NodeIndex(29)), NodeIndex(16));
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 61), @"16..=18");
    }

    #[test]
    fn test_raw_byte_indexes_to_screen_lines_with_line_wrapping() {
        let mut doc = new_doc(b"000001111122222333334444\n(0000111112222)");
        doc.resize(nz(5));
        assert_snapshot!(dump_with_byte_indexes(&doc), @r"
        0..=0  :   0..24  : 000001111122222333334444
        1..=3  :  25..40  : (0000111112222)
        ");

        fn raw_byte_index_to_visible_screen_line(doc: &SexpDocument, index: usize) -> String {
            let screen_line = doc.raw_byte_index_to_visible_screen_line(index);
            let LogicalLine {
                start_index,
                end_index,
                ..
            } = screen_line.logical_line;

            format!(
                "{}..={} [{}]",
                start_index.0, end_index.0, screen_line.index
            )
        }

        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 0),  @"0..=0 [0]");
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 4),  @"0..=0 [0]");
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 5),  @"0..=0 [1]");
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 23), @"0..=0 [4]");
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 25), @"1..=3 [0]");
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 33), @"1..=3 [1]");
        assert_snapshot!(raw_byte_index_to_visible_screen_line(&doc, 39), @"1..=3 [2]");
    }

    #[test]
    fn test_is_raw_byte_range_visible() {
        let mut doc = new_doc(b"((k1 a)(k2 (Var b c))(k3 d)(k4 (Var e f)))");
        assert_snapshot!(dump_with_byte_indexes(&doc), @r"
         0..=4  :   0..7   : ((k1 a)
         5..=8  :   8..16  :  (k2 (Var
         9..=9  :  17..18  :    b
        10..=12 :  19..22  :    c))
        13..=16 :  23..29  :  (k3 d)
        17..=20 :  30..38  :  (k4 (Var
        21..=21 :  39..40  :    e
        22..=25 :  41..45  :    f)))
        ");

        let var = 14..16;
        let hidden_var_value = 19..21;
        let hidden_into_next_line = 19..25;
        let hidden_into_next_line_into_hidden = 19..42;

        assert_snapshot!(doc.core.pretty_printed[var.clone()].as_bstr(), @"ar");
        assert_snapshot!(doc.core.pretty_printed[hidden_var_value.clone()].as_bstr(), @"c)");
        assert_snapshot!(doc.core.pretty_printed[hidden_into_next_line.clone()].as_bstr(), @"c)) (k");
        assert_snapshot!(doc.core.pretty_printed[hidden_into_next_line_into_hidden.clone()].as_bstr(), @"c)) (k3 d) (k4 (Var e f");

        assert_snapshot!(doc.is_raw_byte_range_visible(var.clone()), @"true");
        assert_snapshot!(doc.is_raw_byte_range_visible(hidden_var_value.clone()), @"true");
        assert_snapshot!(doc.is_raw_byte_range_visible(hidden_into_next_line.clone()), @"true");
        assert_snapshot!(doc.is_raw_byte_range_visible(hidden_into_next_line_into_hidden.clone()), @"true");

        // Collapse both variants
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(5));
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(16));

        assert_snapshot!(doc.is_raw_byte_range_visible(var), @"true");
        assert_snapshot!(doc.is_raw_byte_range_visible(hidden_var_value), @"false");
        // These two should arguably return true
        assert_snapshot!(doc.is_raw_byte_range_visible(hidden_into_next_line), @"false");
        assert_snapshot!(doc.is_raw_byte_range_visible(hidden_into_next_line_into_hidden), @"false");
    }
}
