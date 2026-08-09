use std::cmp::Ordering;
use std::iter::DoubleEndedIterator;
use std::num::NonZeroUsize;
use std::ops::{Index, Range, RangeInclusive};
use std::rc::Rc;

use crate::dimensions;
use crate::document::{ContentRange, Document};
use crate::rendering::{Fragment, StyledSegment, Text};
use crate::search::{self, InvertedPairedDelimeters, SearchMatchHighlighter};
use crate::sexp::color_scheme::ColorScheme;
use crate::sexp::core::{
    invariants, AtomKind, AtomMetadata, DocumentToken, ListKind, ListMetadata, NodeIndex,
};
use crate::sexp::layout::LogicalLine;
use crate::sexp::renderer;
use crate::sexp::renderer::{style_typeset_line, FragmentSource, RenderContext};
use crate::sexp::state::{CollapseState, DocState};

use CollapseState::*;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum FocusTargetKind {
    Normal,
    ListValueOfRecordField,
    VariantSingletonValue,
}

pub struct SexpDocument {
    width: NonZeroUsize,
    pub state: DocState,
    // We should maybe keep track of whether this is some or none in document_viewer,
    // but I don't want to add another associated type to Document.
    adjacent_sibling_nav_depth: Option<usize>,
    // TODO: Probably put this in DocumentViewer; this only exists so I could set
    // it to false in tests and not update a bunch of them.
    include_cursor: bool,
}

#[derive(Clone, Debug)]
pub struct TypesetLine(pub Vec<Fragment<FragmentSource>>);

#[derive(Clone, Debug)]
pub struct TypesetLines(pub Vec<TypesetLine>);

impl TypesetLines {
    fn len(&self) -> usize {
        self.0.len()
    }

    fn index_of_closest_line_to_byte_index(&self, byte_index: usize) -> usize {
        for (i, typeset_line) in self.0.iter().enumerate() {
            for fragment in typeset_line.0.iter() {
                if let Text::SourceRange(range) = &fragment.text {
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

        match self
            .logical_line
            .start_index()
            .cmp(&other.logical_line.start_index())
        {
            Ordering::Less => Ordering::Less,
            Ordering::Greater => Ordering::Greater,
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
    fn maybe_logical_line_of_node_index(&self, node_index: NodeIndex) -> Option<LogicalLine> {
        self.state.maybe_logical_line_of_node_index(node_index)
    }

    fn logical_line_of_node_index(&self, node_index: NodeIndex) -> LogicalLine {
        self.state.logical_line_of_node_index(node_index)
    }

    fn collapsible_nodes_in_line<'a>(
        &'a self,
        logical_line: &LogicalLine,
    ) -> impl DoubleEndedIterator<Item = (&'a NodeIndex, &'a CollapseState)> {
        self.state
            .collapsible_nodes
            .range(logical_line.start_index()..=logical_line.end_index())
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
        let mut next_token_is_variant_value = false;
        let mut last_token_was_variant = false;

        for (i, node_index) in logical_line.node_indexes().enumerate() {
            let mut is_this_token_record_field = false;
            let mut is_this_token_record_key = false;
            let mut is_this_token_variant = false;

            let this_token_is_record_value = next_token_is_record_value;
            let this_token_is_variant_value = next_token_is_variant_value;
            next_token_is_record_value = false;
            next_token_is_variant_value = false;

            match self.state.core.token(node_index) {
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

                    next_token_is_variant_value =
                        last_token_was_variant && matches!(atom_kind, AtomKind::Constructor);
                }
                DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => {
                    let should_skip = last_token_was_record_key && !list_kind.is_variant();

                    let focus_target_kind = if this_token_is_record_value {
                        // We "coalesce" singletons when they are values of record fields,
                        // so we still will consider the next token as the value of a record field.
                        next_token_is_record_value = matches!(list_kind, ListKind::Singleton);
                        ListValueOfRecordField
                    } else if this_token_is_variant_value {
                        // Same deal as above with coalescing singletons
                        next_token_is_variant_value = matches!(list_kind, ListKind::Singleton);
                        VariantSingletonValue
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

            // The start of the line should _always_ be focusable. We'll add a debug
            // assert to catch this in tests, but in release we can just make sure to
            // add it and there's no problem.
            if i == 0 {
                let contains_first_node = focusable_nodes.len() == 1;
                debug_assert!(contains_first_node);
                if !contains_first_node {
                    focusable_nodes.push((node_index, Normal));
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
            .next_back()
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

    fn current_adjacent_sibling_nav_depth_or_calculate_new_value(
        &mut self,
        node_index: NodeIndex,
    ) -> usize {
        if self.adjacent_sibling_nav_depth.is_some() {
            return self.adjacent_sibling_nav_depth.unwrap();
        }

        let reference_node = self.first_normal_focusable_node_to_left_of_node_or_node(node_index);
        let depth = self.state.core.depth(reference_node);
        self.adjacent_sibling_nav_depth = Some(depth);
        depth
    }

    // Returns true if a node is the first child of its parent, unless the parent is a variant,
    // in which case it only returns true if it is the second child of its parent.
    fn is_logically_the_first_elem_in_list(&self, node_index: NodeIndex) -> bool {
        let node = self.state.core.node(node_index);

        let Some(parent_index) = node.parent_index() else {
            // If node doesn't have a parent, then just check if it is actually the first
            // element.
            return node.prev_sibling().is_none();
        };

        match self.state.core.token(parent_index).list_kind() {
            Some(ListKind::VariantRecord | ListKind::VariantTuple) => {
                // If parent is a variant, check if the previous sibling is the constructor
                // (i.e., the first element).
                match node.prev_sibling() {
                    None => false,
                    Some(prev_sibling) => {
                        invariants::constructors_are_the_first_child_of_variants();
                        self.state.core.node(prev_sibling).prev_sibling().is_none()
                    }
                }
            }
            _ => {
                // If parent is not a variant, just check if it is actually the first
                // element.
                node.prev_sibling().is_none()
            }
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

        match self.state.core.token(node_index) {
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
                    if matches!(
                        self.state.core.token(atom_node_index),
                        DocumentToken::Atom(_)
                    ) && logical_line.contains_node_index(atom_node_index)
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
                    let list_end_index =
                        self.state.core.token(*node_index).list_end_index().unwrap();
                    let end_line = self.logical_line_of_node_index(list_end_index);
                    return self.maybe_logical_line_of_node_index(end_line.end_index() + 1);
                }
            }
        }

        // If nothing is collapsed, then it's just the next logical line.
        self.maybe_logical_line_of_node_index(logical_line.end_index() + 1)
    }

    fn closest_visible_ancestor(&self, cursor: &NodeIndex) -> NodeIndex {
        let mut closest_visible = *cursor;
        let mut curr = *cursor;

        while let Some(parent_index) = self.state.core.parent_index(curr) {
            match self.state.collapsible_nodes.get(&parent_index) {
                Some(Collapsed) => closest_visible = parent_index,
                None | Some(Expanded) => (),
            }
            curr = parent_index;
        }

        closest_visible
    }

    fn first_visible_line_at_or_above(&self, logical_line: &LogicalLine) -> LogicalLine {
        let visible_ancestor = self.closest_visible_ancestor(&logical_line.start_index());
        self.logical_line_of_node_index(visible_ancestor)
    }

    fn prev_visible_logical_line(&self, logical_line: &LogicalLine) -> Option<LogicalLine> {
        if logical_line.start_index() == NodeIndex(0) {
            return None;
        }

        let previous_logical_line = self.logical_line_of_node_index(logical_line.start_index() - 1);

        Some(self.first_visible_line_at_or_above(&previous_logical_line))
    }

    // Very annoying that this can't be in the `impl Document` block...
    fn move_cursor_up_one_line(&mut self, cursor: NodeIndex) -> Option<NodeIndex> {
        let logical_line = self.logical_line_of_node_index(cursor);
        let Some(prev_logical_line) = self.prev_visible_logical_line(&logical_line) else {
            // If there's no previous line, then we'll focus the first focusable node in
            // that line. (This should only ever happen if it's the first line of the document.)
            assert_eq!(logical_line.start_index().0, 0);
            if cursor.0 != 0 {
                return Some(NodeIndex(0));
            } else {
                return None;
            }
        };

        let node = self.state.core.node(cursor);
        let parent_index = node.parent_index();
        let prev_sibling = node.prev_sibling();

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
            relative_node = prev_sibling;
        } else if parent_index.is_some()
            && prev_logical_line.contains_node_index(parent_index.unwrap())
        {
            relative_node = parent_index;
        }

        if let Some(relative_node) = relative_node {
            focusable_nodes.retain(|(node_index, _)| *node_index <= relative_node);

            match Self::last_normal_focusable_node_or_last_node(focusable_nodes) {
                Some(node_index) => Some(node_index),
                None => {
                    // Probably shouldn't ever happen; just return the start of the line.
                    Some(prev_logical_line.start_index())
                }
            }
        } else {
            // Focus first thing in the previous line.
            Some(focusable_nodes.first().unwrap().0)
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
                        FocusTargetKind::ListValueOfRecordField
                        | FocusTargetKind::VariantSingletonValue => {
                            // Pretty sure this couldn't actually ever be none.
                            prev_focusable_node_in_line.is_none()
                        }
                    }
                }
            };

            if should_collapse && should_actually_collapse {
                let _prev_state = self
                    .state
                    .collapsible_nodes
                    .insert(node_to_collapse, Collapsed);
                // Cursor doesn't move.
                return Some(*cursor);
            }
        }

        // If we didn't collapse, try to move left:
        if let Some(prev_focusable_node_in_line) = prev_focusable_node_in_line {
            return Some(prev_focusable_node_in_line);
        }

        // Can't move further left on our line, so we'll try to move to our parent.
        if let Some(parent_index) = self.state.core.parent_index(*cursor) {
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

    pub fn render_context_with_color_scheme<'a>(
        &'a self,
        color_scheme: &'a ColorScheme,
        focus: NodeIndex,
    ) -> RenderContext<'a> {
        RenderContext::new(color_scheme, &self.state, focus)
    }

    pub fn typeset_logical_line(&self, logical_line: &LogicalLine) -> TypesetLines {
        renderer::typeset_logical_line(logical_line, self.width, &self.state, self.include_cursor)
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
            state: DocState::new(),
            adjacent_sibling_nav_depth: None,
            include_cursor: !cfg!(test),
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
        self.state.append(data);
    }

    fn eof(&mut self) {
        self.state.eof();
    }

    fn top_screen_line_and_cursor(&self) -> Option<(ScreenLine, Self::Cursor)> {
        self.state
            .logical_lines_by_start_index
            .first_key_value()
            .map(|(start_index, logical_line)| {
                (
                    self.first_typeset_screen_line_for_logical_line(logical_line.clone()),
                    *start_index,
                )
            })
    }

    fn bottom_screen_line_and_cursor(&self) -> Option<(ScreenLine, Self::Cursor)> {
        match self.state.logical_lines_by_start_index.last_key_value() {
            None => None,
            Some((_start_index, last_logical_line)) => {
                let last_visible_logical_line =
                    self.first_visible_line_at_or_above(last_logical_line);

                let cursor = last_visible_logical_line.start_index();
                let last_screen_line =
                    self.last_typeset_screen_line_for_logical_line(last_visible_logical_line);

                Some((last_screen_line, cursor))
            }
        }
    }

    fn first_visible_cursor_at_or_before_line_index(&self, index: usize) -> Option<Self::Cursor> {
        let (_start_index, line_at_index) =
            match self.state.logical_lines_by_start_index.get_by_rank(index) {
                None => self.state.logical_lines_by_start_index.last_key_value()?,
                Some(x) => x,
            };

        let visible_line_at_or_before_index = self.first_visible_line_at_or_above(line_at_index);

        Some(visible_line_at_or_before_index.start_index())
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
            .state
            .logical_lines_by_start_index
            .rank_of(&screen_line.logical_line.start_index())
            .expect("to find logical line start in `logical_lines_by_start_index`")
    }

    fn num_lines(&self) -> usize {
        self.state.logical_lines_by_start_index.len()
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
        let fallback = screen_line.logical_line.start_index();
        screen_line
            .typeset_line()
            .0
            .iter()
            .find_map(|fragment| fragment.source.node_index())
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
            Some(logical_line.start_index())
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
        // - Otherwise, if the cursor is a list that continues on another line, move down.
        //
        // It's tricky knowing when it's okay to move down. Consider three cases:
        //
        // ((a 1)        When cursor is at "(a", right shouldn't do anything
        //  (b 2))
        //
        // ((a (         When cursor is at "(a", right should go to the next line because
        //    (b 2))))   the last ( on the first line is collapsible and after the cursor
        //
        // ((a ((        When cursor is at "(a", right should go to the last paren,
        //    (b 2)))))  on that line, then right again show go down because it continus on
        //               another line (even though it's not collapsible)

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

        let node_to_expand = 'find_node_to_expand: {
            let collapsible_nodes_in_line = self.collapsible_nodes_in_line(&current_line);

            let end_of_range_to_check_for_collapsed_nodes =
                next_focusable_node_in_line.unwrap_or(current_line.end_index());

            'checking_collapsible_nodes: for (node_index, collapse_state) in
                collapsible_nodes_in_line
            {
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
            let _prev_state = self
                .state
                .collapsible_nodes
                .insert(node_to_expand, Expanded);
            // Cursor doesn't move.
            return Some(*cursor);
        }

        // We didn't find something to collapse, so we'll try to move
        // to the next focusable node in the line.
        if let Some(next_focusable_node_in_line) = next_focusable_node_in_line {
            return Some(next_focusable_node_in_line);
        }

        let should_move_down = match self.state.core.token(*cursor).list_end_index() {
            None => false,
            Some(end_index) => current_line.end_index() < end_index,
        };

        if should_move_down {
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
        let Some(parent_index) = self.state.core.parent_index(*cursor) else {
            // If we're focused on a top level sexp, we'll move to the first one.
            return Some(NodeIndex(0));
        };

        let DocumentToken::StartOfList(parent_list_metadata) = self.state.core.token(parent_index)
        else {
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
                self.state.core.node(first_child).next_sibling()
            }
        }
    }

    fn move_cursor_to_last_sibling(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let Some(parent_index) = self.state.core.parent_index(*cursor) else {
            // If we're focused on a top level sexp, we'll move to the last one.
            return self.state.core.node_index_of_last_completed_top_level_sexp;
        };

        let DocumentToken::StartOfList(list_metadata) = self.state.core.token(parent_index) else {
            panic!("parent_index didn't point to StartOfList");
        };

        list_metadata.last_child_index()
    }

    fn move_cursor_to_next_sibling_or_down(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let desired_depth = self.current_adjacent_sibling_nav_depth_or_calculate_new_value(*cursor);
        let curr_depth = self.state.core.depth(*cursor);

        // If currently at the correct depth, try to move to the next sibling. When
        // moving backwards, there are concerns that we wouldn't want to focus the
        // previous sibling (e.g., going from a record field value to the key of the
        // record field, or from the first value of a variant to the constructor),
        // but that isn't a concern when moving forward in the document.
        if curr_depth == desired_depth {
            if let Some(next_sibling) = self.state.core.node(*cursor).next_sibling() {
                return Some(next_sibling);
            }
        }

        // Ok, so we can't move to the next sibling. We'll try just going to the next
        // line, seeing if there's anything at the right depth there, and then skipping
        // ahead until we find something.
        let starting_logical_line = self.logical_line_of_node_index(*cursor);
        let mut candidate_line = self.next_visible_logical_line(&starting_logical_line)?;

        loop {
            let leading_depth = self.state.core.depth(candidate_line.start_index());

            match leading_depth.cmp(&desired_depth) {
                Ordering::Equal => {
                    // The first node in any line is always focusable, so if we're at the right
                    // depth, great! We'll stop there.
                    return Some(candidate_line.start_index());
                }
                Ordering::Less => {
                    // If the line is starts at a higher level, pick the deepest normal focusable
                    // node (that's not too deep).
                    let mut best_choice = candidate_line.start_index();
                    for (node_index, focus_target_kind) in
                        self.focusable_nodes_in_line(&candidate_line).into_iter()
                    {
                        // Stop as soon as we see something more deeply nested. (This might
                        // help do the right thing when we put more stuff on one line.)
                        if self.state.core.depth(node_index) > desired_depth {
                            break;
                        }

                        match focus_target_kind {
                            FocusTargetKind::Normal => {
                                best_choice = node_index;
                            }
                            FocusTargetKind::ListValueOfRecordField
                            | FocusTargetKind::VariantSingletonValue => {
                                // Don't move focus to these
                            }
                        }
                    }

                    return Some(best_choice);
                }
                Ordering::Greater => {
                    // We've ended up at more deeply nested line than we want. We'll go to
                    // the parent, zoom to its ending, then go to the next line after that.
                    let parent_index = self
                        .state
                        .core
                        .parent_index(candidate_line.start_index())
                        .unwrap();
                    let closing_paren = self
                        .state
                        .core
                        .token(parent_index)
                        .list_end_index()
                        .unwrap();
                    let closing_paren_line = self.logical_line_of_node_index(closing_paren);
                    candidate_line = self.next_visible_logical_line(&closing_paren_line)?;
                }
            }
        }
    }

    fn move_cursor_to_prev_sibling_or_up(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let desired_depth = self.current_adjacent_sibling_nav_depth_or_calculate_new_value(*cursor);
        let curr_depth = self.state.core.depth(*cursor);

        // If currently at the correct depth, try to move to the previous sibling, but
        // make sure that's actually a focusable node! We don't want to move from the
        // first argument in a variant to the constructor, or from the value of a record
        // field to the key.
        if curr_depth == desired_depth {
            if let Some(prev_sibling) = self.state.core.node(*cursor).prev_sibling() {
                return Some(
                    self.first_normal_focusable_node_to_left_of_node_or_node(prev_sibling),
                );
            }
        }

        // When not going to a previous sibling, it should work just like hitting 'k'
        // (possibly multiple times).
        let mut candidate_cursor = self.move_cursor_up_one_line(*cursor)?;

        loop {
            let leading_depth = self.state.core.depth(candidate_cursor);

            match leading_depth.cmp(&desired_depth) {
                Ordering::Equal => {
                    // Great! We're at the right depth.
                    return Some(candidate_cursor);
                }
                Ordering::Less => {
                    // Technically there might be something at the right depth to the
                    // right of the candidate cursor, but `move_cursor_up_one_line` will
                    // take us to the rightmost "regularly" focusable line, which is more
                    // what we want.
                    return Some(candidate_cursor);
                }
                Ordering::Greater => {
                    // We're too deep. We'll just keep moving left to a parent until we find
                    // something at the right depth.
                    candidate_cursor =
                        self.move_cursor_left_or_up_without_collapsing(&candidate_cursor)?;
                }
            }
        }
    }

    fn clear_adjacent_sibling_nav_state(&mut self) {
        self.adjacent_sibling_nav_depth = None;
    }

    fn move_cursor_to_next_indentation_change(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let mut curr_logical_line = self.logical_line_of_node_index(*cursor);
        let mut starting_indentation = curr_logical_line.indentation();
        let mut is_first = true;

        loop {
            let Some(next_logical_line) = self.next_visible_logical_line(&curr_logical_line) else {
                // If we hit the bottom of the document, just return the start of that line.
                if *cursor < curr_logical_line.start_index() {
                    return Some(curr_logical_line.start_index());
                } else {
                    return None;
                }
            };

            if next_logical_line.indentation() < starting_indentation {
                // This is this case:
                //
                // start:      (y 1)
                //             (z 2)))))
                // end:   (next thing)
                //
                // We want to go to (next thing)
                return Some(next_logical_line.start_index());
            } else if next_logical_line.indentation() > starting_indentation {
                // We've run up to a nested thing. If this is not the immediate
                // next line, we'll stop at the parent:
                //
                // start:  (a 1)
                //         (b 2)
                // end:    (c (
                //           ...
                if !is_first {
                    return Some(curr_logical_line.start_index());
                }

                // So we started on one line, and the next line is immediately more indented.
                // This can be tricky:
                //
                // start: (((a 1)     Here, it's a little awkward if we move to "(b 2)", because
                //          (b 2)     it's not the start of the record. And moving to "(c 3)" or
                //          (c 3))    the next collapsed elem would feel weird. We want to move
                //         (...))     to "(a 1)".
                //
                // start: ((Var       But here, moving to 1 feels more natural, since it's the
                //           1        first payload in the variant.
                //           2)
                //         (Bar ...))
                //
                // The heuristic we'll use is to check if the next line is the logically the
                // "first" elem in a list. If it's not, we'll try to focus the first element
                // of that list instead.
                if self.is_logically_the_first_elem_in_list(next_logical_line.start_index()) {
                    return Some(next_logical_line.start_index());
                }

                let target_index = self
                    .state
                    .core
                    .node(next_logical_line.start_index())
                    .parent_index()
                    .unwrap()
                    + 1;

                // We have to make sure we're not _starting_ at (or after) the first child,
                // otherwise we won't go anywhere. We want to make sure we actually move
                // forward.
                if *cursor < target_index {
                    return Some(target_index);
                }

                // And now we have to update our starting indentation to the next line as if
                // that's the indentation level where we started.
                starting_indentation = next_logical_line.indentation();
            }

            is_first = false;
            curr_logical_line = next_logical_line;
        }
    }

    fn move_cursor_to_prev_indentation_change(&mut self, cursor: &NodeIndex) -> Option<NodeIndex> {
        let mut curr_logical_line = self.logical_line_of_node_index(*cursor);

        // First just move the cursor to the start of the line if it's not there already.
        if let Some((start_of_line, _)) = self.focusable_nodes_in_line(&curr_logical_line).first() {
            if start_of_line != cursor {
                return Some(*start_of_line);
            }
        }

        let mut starting_indentation = curr_logical_line.indentation();
        let mut is_first = true;
        loop {
            let Some(prev_logical_line) = self.prev_visible_logical_line(&curr_logical_line) else {
                // If we're at the top of the document just go to the start of that line.
                if curr_logical_line.start_index() < *cursor {
                    return Some(curr_logical_line.start_index());
                } else {
                    return None;
                }
            };

            if prev_logical_line.indentation() > starting_indentation {
                // This is this case:
                //
                //             ((a 1)
                //              (b 2))
                // end:        (y 1)
                //             (z 2)))))
                // start: (next thing)
                //
                // If this is the immediately preceding line, we'll update our starting indentation
                // and go on from there so we keep going past "(z 2)". But otherwise we stop so we
                // end up at "(y 1)" and not "(b 2)".
                if is_first {
                    starting_indentation = prev_logical_line.indentation();
                } else {
                    return Some(curr_logical_line.start_index());
                }
            } else if prev_logical_line.indentation() < starting_indentation {
                // We've bumped into a parent. If the current node isn't logically the first child,
                // we'll move to its sibling.
                if !self.is_logically_the_first_elem_in_list(curr_logical_line.start_index()) {
                    let first_elem = self
                        .state
                        .core
                        .node(curr_logical_line.start_index())
                        .parent_index()
                        .unwrap()
                        + 1;
                    return Some(first_elem);
                }

                // Otherwise, if this the immediately preceding line, we'll move to it, but if
                // we've been moving for a while, we'll stop before it.
                if is_first {
                    return Some(prev_logical_line.start_index());
                } else {
                    return Some(curr_logical_line.start_index());
                }
            }

            is_first = false;
            curr_logical_line = prev_logical_line;
        }
    }

    fn collapse_node_and_siblings(
        &mut self,
        cursor: &NodeIndex,
        depth: Option<usize>,
    ) -> Option<Self::Cursor> {
        Some(
            self.state
                .set_collapse_state_on_node_and_siblings(*cursor, depth, Collapsed),
        )
    }

    fn expand_node_and_siblings(
        &mut self,
        cursor: &NodeIndex,
        depth: Option<usize>,
    ) -> Option<Self::Cursor> {
        Some(
            self.state
                .set_collapse_state_on_node_and_siblings(*cursor, depth, Expanded),
        )
    }

    fn path_to_cursor(&self, cursor: &NodeIndex) -> Option<String> {
        self.state.core.sexp_get_style_path_to_node(*cursor)
    }

    fn debug_text_content(&self, screen_line: &ScreenLine, cursor: &NodeIndex) -> Vec<u8> {
        use unicode_segmentation::UnicodeSegmentation;

        let mut output = String::new();

        let mut highlighted_cursor = false;

        for fragment in screen_line.typeset_line().0.iter() {
            match &fragment.text {
                Text::SourceRange(range) => {
                    let content =
                        std::str::from_utf8(&self.state.core.pretty_printed[range.clone()])
                            .unwrap_or("INVALID UTF8");

                    let source_node_index = fragment.source.node_index();

                    if !highlighted_cursor && source_node_index == Some(*cursor) {
                        if let Some(after_paren) = content.strip_prefix("(") {
                            output.push('[');
                            output.push_str(after_paren);
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
        match_highlighter: &mut SearchMatchHighlighter<'_>,
    ) -> Option<Vec<StyledSegment>> {
        let color_scheme = ColorScheme::default();
        let render_context = self.render_context_with_color_scheme(&color_scheme, *cursor);
        Some(style_typeset_line(
            &self.state.core,
            &render_context,
            &screen_line.logical_line,
            screen_line.typeset_line(),
            match_highlighter,
        ))
    }

    fn get_search_input_under_cursor(&self, cursor: &NodeIndex) -> Option<String> {
        let node = self.state.core.node(*cursor);
        let data_range = match &node.token {
            DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => match list_kind {
                ListKind::RecordField | ListKind::VariantRecord | ListKind::VariantTuple => {
                    invariants::record_keys_are_the_first_child_of_record_fields();
                    invariants::constructors_are_the_first_child_of_variants();
                    let atom_range_end = self.state.core.node(*cursor + 1).data_range.end;
                    Some(node.data_range.start..atom_range_end)
                }
                ListKind::Plain | ListKind::Record | ListKind::Singleton | ListKind::DateTime => {
                    None
                }
            },
            DocumentToken::Atom(_) | DocumentToken::Unit { .. } => Some(node.data_range.clone()),
            DocumentToken::EndOfList(_)
            | DocumentToken::LineComment
            | DocumentToken::BlockComment
            | DocumentToken::Error(_) => None,
        }?;

        // Right now the only non-UTF-8 content can appear in comments, which we
        // don't try to convert to search terms.
        let raw_text = &self.state.core.pretty_printed[data_range];
        let text = match std::str::from_utf8(raw_text) {
            Ok(s) => s,
            Err(_) => return None,
        };

        Some(search::escape_literal_and_maybe_add_word_boundaries(
            text,
            Self::inverted_paired_delimiters_for_search_input(),
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
        self.state.core.raw_bytes_of_complete_content()
    }

    fn raw_byte_range_of_cursor(&self, cursor: &NodeIndex) -> Range<usize> {
        let nodes = self.nodes_considered_as_part_of_cursor(*cursor);
        let start = self.state.core.node(*nodes.start()).data_range.start;
        let end = self.state.core.node(*nodes.end()).data_range.end;
        start..end
    }

    fn raw_byte_index_to_cursor(&self, byte_index: usize) -> NodeIndex {
        let closest_node_to_byte_index = self.state.core.closest_node_to_byte_index(byte_index);
        self.first_normal_focusable_node_to_left_of_node_or_node(closest_node_to_byte_index)
    }

    fn raw_byte_index_to_visible_screen_line(&self, byte_index: usize) -> ScreenLine {
        let closest_node_to_byte_index = self.state.core.closest_node_to_byte_index(byte_index);
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
        doc.state.all_logical_lines()
    }

    pub fn visible_logical_lines(doc: &SexpDocument) -> Vec<LogicalLine> {
        let mut curr_logical_line = doc
            .state
            .logical_lines_by_start_index
            .first_key_value()
            .unwrap()
            .1
            .clone();

        let mut visible_logical_lines = vec![curr_logical_line.clone()];

        while let Some(next_logical_line) = doc.next_visible_logical_line(&curr_logical_line) {
            visible_logical_lines.push(next_logical_line.clone());
            curr_logical_line = next_logical_line;
        }

        visible_logical_lines
    }

    pub fn dump(doc: &SexpDocument) -> String {
        let logical_lines = logical_lines(doc);
        crate::sexp::layout::tests::show_logical_lines(&doc.state.core, logical_lines)
    }

    pub fn dump_visible(doc: &SexpDocument) -> String {
        let visible_logical_lines = visible_logical_lines(doc);
        crate::sexp::layout::tests::show_logical_lines(&doc.state.core, visible_logical_lines)
    }

    pub fn dump_with_byte_indexes(doc: &SexpDocument) -> String {
        let logical_lines = logical_lines(doc);
        crate::sexp::layout::tests::show_logical_lines_with_byte_indexes(
            &doc.state.core,
            logical_lines,
        )
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
    use insta::{assert_debug_snapshot, assert_snapshot};

    #[derive(Copy, Clone, Debug)]
    enum Action {
        Down(usize),
        Up(usize),
        Right,
        Left,
        LeftNoCollapse,
        FirstSibling,
        LastSibling,
        NextSibling,
        PrevSibling,
        NextIndentationChange,
        PrevIndentationChange,
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
                LeftNoCollapse => self.move_cursor_left_or_up_without_collapsing(&current_cursor),
                FirstSibling => self.move_cursor_to_first_sibling(&current_cursor),
                LastSibling => self.move_cursor_to_last_sibling(&current_cursor),
                NextSibling => self.move_cursor_to_next_sibling_or_down(&current_cursor),
                PrevSibling => self.move_cursor_to_prev_sibling_or_up(&current_cursor),
                NextIndentationChange => {
                    self.move_cursor_to_next_indentation_change(&current_cursor)
                }
                PrevIndentationChange => {
                    self.move_cursor_to_prev_indentation_change(&current_cursor)
                }
                FocusBottom => self
                    .bottom_screen_line_and_cursor()
                    .map(|(_, cursor)| cursor),
            }
        }
    }

    fn perform_action_and_compute_collapse_state_changes(
        doc: &mut SexpDocument,
        current_cursor: NodeIndex,
        action: Action,
    ) -> (Option<NodeIndex>, String) {
        let prev_collapsed_states = doc.state.collapsible_nodes.clone();
        let new_cursor = doc.perform_action(current_cursor, action);
        let new_collapsed_states = doc.state.collapsible_nodes.clone();

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
    fn test_moving_around_variants() {
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
        assert_snapshot!(movements, @"Up(1) => NodeIndex(5)");
    }

    #[test]
    fn test_moving_around_singleton_variants() {
        // Basic singleton variant
        let mut doc = new_doc(b"(Variant (1 2))");
        assert_snapshot!(dump(&doc), @r"
        0..=2  : (Variant (
        3..=3  :   1
        4..=6  :   2))
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(3), vec![Left]);
        assert_snapshot!(movements, @"Left => NodeIndex(0)");

        // Singleton variant as first elem in list
        let mut doc = new_doc(b"((Variant (1 2)) 3 4)");

        assert_snapshot!(dump(&doc), @r"
        0..=3  : ((Variant (
        4..=4  :    1
        5..=7  :    2))
        8..=8  :  3
        9..=10 :  4)
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(5), vec![Left, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        Left =>           NodeIndex(1)
        LeftNoCollapse => NodeIndex(0)
        ");

        // Singleton variants as first and regular elem in record
        let mut doc = new_doc(b"((a (Variant (1 2)))(b (Other (((3 4))))))");

        assert_snapshot!(dump(&doc), @r"
         0..=5  : ((a (Variant (
         6..=6  :    1
         7..=10 :    2)))
        11..=17 :  (b (Other (((
        18..=18 :    3
        19..=25 :    4))))))
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(6), vec![Left, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        Left =>           NodeIndex(1)
        LeftNoCollapse => NodeIndex(0)
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(18), vec![Left, LeftNoCollapse]);
        assert_snapshot!(movements, @r"
        Left =>           NodeIndex(11)
        LeftNoCollapse => NodeIndex(0)
        ");
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
    fn test_tricky_cases_for_moving_right() {
        let mut doc = new_doc(b"((a 1)(b 2))");
        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((a 1)
        5..=9  :  (b 2))
        ");
        let movements = show_cursor_movements(&mut doc, NodeIndex(0), vec![Right, Right]);
        assert_snapshot!(movements, @r"
        Right => NodeIndex(1)
        Right => -
        ");

        let mut doc = new_doc(b"((a ((b 2))))");
        assert_snapshot!(dump(&doc), @r"
        0..=3  : ((a (
        4..=10 :    (b 2))))
        ");
        let movements = show_cursor_movements(&mut doc, NodeIndex(0), vec![Right, Right, Right]);
        assert_snapshot!(movements, @r"
        Right => NodeIndex(1)
        Right => NodeIndex(4)
        Right => -
        ");

        let mut doc = new_doc(b"((a (((b 2)))))");
        assert_snapshot!(dump(&doc), @r"
        0..=4  : ((a ((
        5..=12 :    (b 2)))))
        ");
        let movements =
            show_cursor_movements(&mut doc, NodeIndex(0), vec![Right, Right, Right, Right]);
        assert_snapshot!(movements, @r"
        Right => NodeIndex(1)
        Right => NodeIndex(4)
        Right => NodeIndex(5)
        Right => -
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
    fn test_moving_to_next_and_prev_sibling_basic() {
        let mut doc =
            new_doc(b"(((a 1) (b 2)) ((a 3) (b 4))) (((a 5) (b 6))) (((a 7) (b 8)) ((a 9) (b 0)))");
        assert_snapshot!(dump(&doc), @r"
         0..=5  : (((a 1)
         6..=10 :   (b 2))
        11..=15 :  ((a 3)
        16..=21 :   (b 4)))
        22..=27 : (((a 5)
        28..=33 :   (b 6)))
        34..=39 : (((a 7)
        40..=44 :   (b 8))
        45..=49 :  ((a 9)
        50..=55 :   (b 0)))
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(1),
            vec![
                NextSibling,
                NextSibling,
                NextSibling,
                NextSibling,
                NextSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
            ],
        );
        assert_snapshot!(movements, @r"
        NextSibling => NodeIndex(11)
        NextSibling => NodeIndex(23)
        NextSibling => NodeIndex(35)
        NextSibling => NodeIndex(45)
        NextSibling => -
        PrevSibling => NodeIndex(35)
        PrevSibling => NodeIndex(23)
        PrevSibling => NodeIndex(11)
        PrevSibling => NodeIndex(1)
        PrevSibling => NodeIndex(0)
        PrevSibling => -
        ");

        let mut doc = new_doc(
            b"((((a 1) (b 2)) ((a 3) (b 4)))) ((((a 5) (b 6)))) ((((a 7) (b 8)) ((a 9) (b 0))))",
        );
        assert_snapshot!(dump(&doc), @r"
         0..=1  : ((
         2..=6  :   ((a 1)
         7..=11 :    (b 2))
        12..=16 :   ((a 3)
        17..=23 :    (b 4))))
        24..=25 : ((
        26..=30 :   ((a 5)
        31..=37 :    (b 6))))
        38..=39 : ((
        40..=44 :   ((a 7)
        45..=49 :    (b 8))
        50..=54 :   ((a 9)
        55..=61 :    (b 0))))
        ");

        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(2),
            vec![
                NextSibling,
                NextSibling,
                NextSibling,
                NextSibling,
                NextSibling,
                NextSibling,
                NextSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
                PrevSibling,
            ],
        );
        assert_snapshot!(movements, @r"
        NextSibling => NodeIndex(12)
        NextSibling => NodeIndex(25)
        NextSibling => NodeIndex(26)
        NextSibling => NodeIndex(39)
        NextSibling => NodeIndex(40)
        NextSibling => NodeIndex(50)
        NextSibling => -
        PrevSibling => NodeIndex(40)
        PrevSibling => NodeIndex(39)
        PrevSibling => NodeIndex(26)
        PrevSibling => NodeIndex(25)
        PrevSibling => NodeIndex(12)
        PrevSibling => NodeIndex(2)
        PrevSibling => NodeIndex(1)
        PrevSibling => NodeIndex(0)
        ");
    }

    #[test]
    fn test_moving_to_next_and_prev_sibling_edge_cases() {
        // Don't move to the constructor of a variant
        let mut doc = new_doc(b"(Variant (field 1) (field 2))");
        assert_snapshot!(dump(&doc), @r"
        0..=1  : (Variant
        2..=5  :   (field 1)
        6..=10 :   (field 2))
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(6), vec![PrevSibling, PrevSibling]);
        assert_snapshot!(movements, @r"
        PrevSibling => NodeIndex(2)
        PrevSibling => NodeIndex(0)
        ");

        // Check for weird things with singleton variants
        let mut doc = new_doc(b"((a (X ((b (Y ((c Z))))))))");
        assert_snapshot!(dump(&doc), @r"
         0..=5  : ((a (X (
         6..=10 :    (b (Y (
        11..=21 :      (c Z))))))))
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(11), vec![PrevSibling, PrevSibling]);
        assert_snapshot!(movements, @r"
        PrevSibling => NodeIndex(6)
        PrevSibling => NodeIndex(1)
        ");

        // More weird things with singleton variants
        let mut doc = new_doc(b"((One uno)(Two dos deux)(One uno))");
        assert_snapshot!(dump(&doc), @r"
         0..=4  : ((One uno)
         5..=6  :  (Two
         7..=7  :    dos
         8..=9  :    deux)
        10..=14 :  (One uno))
        ");

        let movements =
            show_cursor_movements(&mut doc, NodeIndex(7), vec![PrevSibling, PrevSibling]);
        assert_snapshot!(movements, @r"
        PrevSibling => NodeIndex(5)
        PrevSibling => NodeIndex(1)
        ");

        // Not ideal; we don't want to move to a node that's not focusable.
        let movements = show_cursor_movements(&mut doc, NodeIndex(8), vec![NextSibling]);
        assert_snapshot!(movements, @"NextSibling => NodeIndex(12)");
    }

    #[test]
    fn test_moving_to_next_and_prev_sibling_nodes_at_same_depth_in_different_structures() {
        let mut doc = new_doc(b"(1 (2 3)) ((a 4) (b 5))");
        assert_snapshot!(dump(&doc), @r"
         0..=1  : (1
         2..=3  :  (2
         4..=6  :   3))
         7..=11 : ((a 4)
        12..=16 :  (b 5))
        ");

        // The "a" is at the same depth as the starting "2", but not focusable.
        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(3),
            vec![NextSibling, NextSibling, NextSibling],
        );
        assert_snapshot!(movements, @r"
        NextSibling => NodeIndex(4)
        NextSibling => NodeIndex(8)
        NextSibling => NodeIndex(12)
        ");

        let mut doc = new_doc(b"((a ((1 2)(3 4)))(b (true)))");
        assert_snapshot!(dump(&doc), @r"
         0..=3  : ((a (
         4..=5  :    (1
         6..=7  :     2)
         8..=9  :    (3
        10..=13 :     4)))
        14..=20 :  (b (true)))
        ");

        // The 17 is qualitatively different; should the focus move there? In this case
        // it seems like no, but if it were [5] instead of [true], maybe?
        let movements =
            show_cursor_movements(&mut doc, NodeIndex(4), vec![NextSibling, NextSibling]);
        assert_snapshot!(movements, @r"
        NextSibling => NodeIndex(8)
        NextSibling => NodeIndex(17)
        ");
    }

    #[test]
    fn test_moving_to_next_and_prev_sibling_nodes_different_depths_due_to_singletons() {
        // The values of the record are lists of records. If one of the lists is a singleton,
        // it gets coalesced; should that be focused?
        let mut doc = new_doc(b"((x (((a 1) (b 2)) ((c 3) (d 4)))) (y (((e 5) (f 6)))))");
        assert_snapshot!(dump(&doc), @r"
         0..=3  : ((x (
         4..=8  :    ((a 1)
         9..=13 :     (b 2))
        14..=18 :    ((c 3)
        19..=25 :     (d 4))))
        26..=29 :  (y ((
        30..=33 :    (e 5)
        34..=41 :    (f 6)))))
        ");

        // It wouldn't be unreasonable to go to 29 instead of 26.
        let movements =
            show_cursor_movements(&mut doc, NodeIndex(4), vec![NextSibling, NextSibling]);
        assert_snapshot!(movements, @r"
        NextSibling => NodeIndex(14)
        NextSibling => NodeIndex(26)
        ");

        doc.clear_adjacent_sibling_nav_state();

        // 19 is technically at the same depth in the document, but indented greater.
        // Seems reasonable to still move there.
        let movements = show_cursor_movements(
            &mut doc,
            NodeIndex(34),
            vec![PrevSibling, PrevSibling, PrevSibling],
        );
        assert_snapshot!(movements, @r"
        PrevSibling => NodeIndex(30)
        PrevSibling => NodeIndex(26)
        PrevSibling => NodeIndex(19)
        ");
    }

    #[test]
    fn test_moving_to_next_and_prev_indentation_change() {
        let mut doc = new_doc(
            b"(((a (1))(b 2)(c 3))((d 4)(e 6)(f ((g 7)(h ((i 9)(j 10)))(k 11)(l 12)))))((Var 1 2 3)(Bar 4 5 (6)))",
        );
        assert_snapshot!(dump(&doc), @r"
         0..=7  : (((a (1))
         8..=11 :   (b 2)
        12..=16 :   (c 3))
        17..=21 :  ((d 4)
        22..=25 :   (e 6)
        26..=28 :   (f (
        29..=32 :     (g 7)
        33..=35 :     (h (
        36..=39 :       (i 9)
        40..=45 :       (j 10)))
        46..=49 :     (k 11)
        50..=57 :     (l 12)))))
        58..=60 : ((Var
        61..=61 :    1
        62..=62 :    2
        63..=64 :    3)
        65..=66 :  (Bar
        67..=67 :    4
        68..=68 :    5
        69..=73 :    (6)))
        ");

        fn show_all_indentation_changes(doc: &mut SexpDocument) -> String {
            let mut s = String::new();
            let mut curr = NodeIndex(0);

            while let Some(next) = doc.move_cursor_to_next_indentation_change(&curr) {
                let _ = writeln!(s, "{next:?}");
                curr = next;
            }

            let _ = writeln!(s, "<end>");

            while let Some(prev) = doc.move_cursor_to_prev_indentation_change(&curr) {
                let _ = writeln!(s, "{prev:?}");
                curr = prev;
            }

            s
        }

        assert_snapshot!(doc.is_logically_the_first_elem_in_list(NodeIndex(67)), @"true");

        assert_snapshot!(show_all_indentation_changes(&mut doc), @r"
        NodeIndex(2)
        NodeIndex(17)
        NodeIndex(18)
        NodeIndex(26)
        NodeIndex(29)
        NodeIndex(33)
        NodeIndex(36)
        NodeIndex(46)
        NodeIndex(58)
        NodeIndex(61)
        NodeIndex(65)
        NodeIndex(67)
        NodeIndex(69)
        <end>
        NodeIndex(67)
        NodeIndex(65)
        NodeIndex(61)
        NodeIndex(58)
        NodeIndex(46)
        NodeIndex(36)
        NodeIndex(33)
        NodeIndex(29)
        NodeIndex(26)
        NodeIndex(18)
        NodeIndex(17)
        NodeIndex(2)
        NodeIndex(0)
        ");

        let movements = show_cursor_movements(&mut doc, NodeIndex(5), vec![NextIndentationChange]);
        // Starting at the (1) doesn't take us backwards.
        assert_snapshot!(movements, @"NextIndentationChange => NodeIndex(17)");

        // Check movements past collapsed nodes
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(1));
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(26));
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(59));
        doc.collapse_or_move_cursor_left_or_up(&NodeIndex(65));

        assert_snapshot!(dump_visible(&doc), @r"
         0..=7  : (((a (1))
        17..=21 :  ((d 4)
        22..=25 :   (e 6)
        26..=28 :   (f (
        58..=60 : ((Var
        65..=66 :  (Bar
        ");

        assert_snapshot!(show_all_indentation_changes(&mut doc), @r"
        NodeIndex(1)
        NodeIndex(17)
        NodeIndex(18)
        NodeIndex(58)
        NodeIndex(59)
        NodeIndex(65)
        <end>
        NodeIndex(59)
        NodeIndex(58)
        NodeIndex(18)
        NodeIndex(17)
        NodeIndex(1)
        NodeIndex(0)
        ");
    }

    #[test]
    fn test_get_search_input_under_cursor() {
        let doc = new_doc(b"(1 () ; line\n ((a 1) #| block |# (b 2)) (Var (c 3))) (");
        assert_snapshot!(dump(&doc), @r"
         0..=1  : (1
         2..=2  :  ()
         3..=3  :  ; line
         4..=8  :  ((a 1)
         9..=9  :   #| block |#
        10..=14 :   (b 2))
        15..=16 :  (Var
        17..=22 :    (c 3)))
        23..=23 : (
        24..=24 :  ERR: Unexpected EOF while parsing list
        25..=25 :
        ");

        let f = |i| {
            doc.get_search_input_under_cursor(&NodeIndex(i))
                .unwrap_or("<none>".to_string())
        };

        // Record fields and variants
        assert_snapshot!(f(5), @r"(a\>");
        assert_snapshot!(f(15), @r"(Var\>");

        // Atoms and unit
        assert_snapshot!(f(1), @r"\<1\>");
        assert_snapshot!(f(2), @"()");

        // Lists and records
        assert_snapshot!(f(0), @"<none>");
        assert_snapshot!(f(4), @"<none>");

        // Comments and errors
        assert_snapshot!(f(3), @"<none>");
        assert_snapshot!(f(9), @"<none>");
        assert_snapshot!(f(24), @"<none>");
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
            let logical_line = doc
                .raw_byte_index_to_visible_screen_line(index)
                .logical_line;

            format!(
                "{}..={}",
                logical_line.start_index().0,
                logical_line.end_index().0
            )
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
            let start_index = screen_line.logical_line.start_index().0;
            let end_index = screen_line.logical_line.end_index().0;

            format!("{start_index}..={end_index} [{}]", screen_line.index)
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

        assert_snapshot!(doc.state.core.pretty_printed[var.clone()].as_bstr(), @"ar");
        assert_snapshot!(doc.state.core.pretty_printed[hidden_var_value.clone()].as_bstr(), @"c)");
        assert_snapshot!(doc.state.core.pretty_printed[hidden_into_next_line.clone()].as_bstr(), @"c)) (k");
        assert_snapshot!(doc.state.core.pretty_printed[hidden_into_next_line_into_hidden.clone()].as_bstr(), @"c)) (k3 d) (k4 (Var e f");

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
