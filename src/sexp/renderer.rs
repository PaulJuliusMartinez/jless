use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::rc::Rc;

use crate::rendering::{Attrs, Compositor, StyledSegment, Text};
use crate::search::SearchMatchHighlighter;
use crate::sexp::color_scheme::ColorScheme;
use crate::sexp::core::{
    invariants, DocCore, DocumentToken, EndOfListMetadata, ListKind, NodeIndex,
};
use crate::sexp::document::{TypesetLine, TypesetLines};
use crate::sexp::layout::LogicalLine;
use crate::sexp::state::{CollapseState, DocState};

#[derive(Copy, Clone, Debug)]
pub enum FragmentSource {
    Cursor,
    Whitespace,
    Node(NodeIndex),
    NodePreview(NodeIndex),
    ElidedPreviewNodes,
    SexpComment(NodeIndex),
}

impl FragmentSource {
    pub fn node_index(&self) -> Option<NodeIndex> {
        use FragmentSource::*;
        match self {
            Cursor | Whitespace | ElidedPreviewNodes => None,
            Node(node_index) | NodePreview(node_index) | SexpComment(node_index) => {
                Some(*node_index)
            }
        }
    }
}

struct Typesetter<'a> {
    logical_line: &'a LogicalLine,
    doc_content: &'a [u8],
    state: &'a DocState,
    compositor: Compositor<'a, FragmentSource>,
    include_cursor: bool,
}

pub fn typeset_logical_line(
    logical_line: &LogicalLine,
    doc_width: NonZeroUsize,
    state: &DocState,
    include_cursor: bool,
) -> TypesetLines {
    let doc_content = state.core.pretty_printed.data();
    let compositor = Compositor::new(doc_content, doc_width);

    let mut typesetter = Typesetter {
        logical_line,
        doc_content,
        state,
        compositor,
        include_cursor,
    };

    typesetter.typeset();

    TypesetLines(
        typesetter
            .compositor
            .finish()
            .into_iter()
            .map(TypesetLine)
            .collect(),
    )
}

const CURSOR_PLACEHOLDER: &str = "◆ ";

const NOT_FOCUSED_LINE: &str = "  ";
const FOCUSED_LINE: &str = "▶ ";
const FOCUSED_COLLAPSED_CONTAINER: &str = "▶ ";
const FOCUSED_EXPANDED_CONTAINER: &str = "▼ ";
const COLLAPSED_CONTAINER: &str = "▷ ";
const EXPANDED_CONTAINER: &str = "▽ ";

impl<'a> Typesetter<'a> {
    fn typeset(&mut self) {
        if self.logical_line.indentation > 0 {
            self.compositor
                .append_spaces(self.logical_line.indentation, FragmentSource::Whitespace);
        }

        if self.include_cursor {
            self.compositor
                .append_text(Text::Static(CURSOR_PLACEHOLDER), FragmentSource::Cursor);
        }

        let mut prev_node_end_index = None;
        let mut collapsed_start_and_end = None;

        for node_index in self.logical_line.node_indexes() {
            let node = self.state.core.node(node_index);

            if let Some(end_index) = prev_node_end_index {
                let whitespace_range = end_index..(node.data_range.start);
                if whitespace_range.len() > 0 {
                    self.compositor.append_text(
                        Text::SourceRange(whitespace_range),
                        FragmentSource::Whitespace,
                    );
                }
            }

            prev_node_end_index = Some(node.data_range.end);

            if let Some(sexp_comment_range) = node.sexp_comment_range() {
                let text = Text::SourceRange(sexp_comment_range);
                self.compositor
                    .append_text(text, FragmentSource::SexpComment(node_index));
            }

            let mut text = Text::SourceRange(node.data_range.clone());

            match &node.token {
                DocumentToken::StartOfList(list_metadata) => {
                    if let Some(CollapseState::Collapsed) =
                        self.state.collapsible_nodes.get(&node_index)
                    {
                        collapsed_start_and_end = Some((node_index, list_metadata.end_index()));
                        break;
                    }
                }
                DocumentToken::Error(error_metadata) => {
                    text = Text::String((
                        Rc::new(error_metadata.message.clone()),
                        0..error_metadata.message.len(),
                    ));
                }
                DocumentToken::EndOfList(_end_of_list_metadata) => (),
                DocumentToken::Atom(_atom_metadata) => (),
                DocumentToken::Unit {
                    sexp_commented_out: _handled_above,
                } => (),
                DocumentToken::LineComment | DocumentToken::BlockComment => (),
            }

            self.compositor
                .append_text(text, FragmentSource::Node(node_index));
        }

        if let Some((start_index, Some(end_index))) = collapsed_start_and_end {
            self.typeset_collapsed_preview(start_index, end_index);
        }
    }

    fn typeset_collapsed_preview(&mut self, open_index: NodeIndex, close_index: NodeIndex) {
        let needed_space = 1 + self.count_trailing_paren(close_index + 1);

        if self.compositor.start_reserving_space(needed_space) {
            if !self.try_typeset_list_preview(open_index, close_index) {
                self.append_reserved_ellipsis();
            }

            self.add_trailing_paren_after_collapsed_preview(close_index + 1);
        } else {
            // If we can't reserve enough space for a preview, we'll just print an ellipsis and then
            // the closing parens and get on with it.

            self.compositor
                .append_text(Text::ellipsis(), FragmentSource::ElidedPreviewNodes);

            let mut closing_paren = close_index + 1;
            while closing_paren
                <= self
                    .state
                    .core
                    .last_node_index_of_part_of_completed_sexp
                    .unwrap()
            {
                if !matches!(
                    self.state.core.token(closing_paren),
                    DocumentToken::EndOfList(_)
                ) {
                    break;
                }

                self.compositor.append_text(
                    Text::SourceRange(self.state.core.node(closing_paren).data_range.clone()),
                    FragmentSource::Node(closing_paren),
                );

                closing_paren = closing_paren + 1;
            }
        }
    }

    fn append_reserved_ellipsis(&mut self) {
        self.compositor
            .append_reserved_text(Text::ellipsis(), FragmentSource::ElidedPreviewNodes);
    }

    fn count_trailing_paren(&self, mut closing_paren: NodeIndex) -> usize {
        let mut count = 0;

        while closing_paren
            <= self
                .state
                .core
                .last_node_index_of_part_of_completed_sexp
                .unwrap()
        {
            if !matches!(
                self.state.core.token(closing_paren),
                DocumentToken::EndOfList(_)
            ) {
                break;
            }

            count += 1;
            closing_paren = closing_paren + 1;
        }

        count
    }

    fn add_trailing_paren_after_collapsed_preview(&mut self, mut closing_paren: NodeIndex) {
        while closing_paren
            <= self
                .state
                .core
                .last_node_index_of_part_of_completed_sexp
                .unwrap()
        {
            if !matches!(
                self.state.core.token(closing_paren),
                DocumentToken::EndOfList(_)
            ) {
                break;
            }

            self.compositor.append_reserved_text(
                Text::SourceRange(self.state.core.node(closing_paren).data_range.clone()),
                FragmentSource::Node(closing_paren),
            );

            closing_paren = closing_paren + 1;
        }
    }

    fn try_typeset_list_preview(
        &mut self,
        list_index: NodeIndex,
        closing_paren: NodeIndex,
    ) -> bool {
        let list_metadata = self.state.core.token(list_index).list_metadata();
        let num_elems = list_metadata.data_length();

        // We've reserved 1; if there's no children it _should_ be an atom, but
        // if for some reason it's not, we just need to print the two parens, so
        // we need one more space; otherwise we need two more spaces for both parens.
        let extra_needed_space = if num_elems == 0 { 1 } else { 2 };
        if !self.compositor.reserve_more_space(extra_needed_space) {
            return false;
        }

        // Write opening paren
        self.append_reserved_node_as_preview(list_index);

        // For singletons, print the parens and recurse; we still want to keep
        // diving in and print the elems of the most deeply nested list.
        if num_elems == 1 {
            let inner_list_index = list_index + 1;
            if let DocumentToken::StartOfList(inner_list_metadata) =
                self.state.core.token(inner_list_index)
            {
                if let Some(inner_closing_paren) = inner_list_metadata.end_index() {
                    if !self.try_typeset_list_preview(inner_list_index, inner_closing_paren) {
                        self.append_reserved_ellipsis();
                    }

                    self.append_reserved_node_as_preview(closing_paren);
                    return true;
                }
            }
        }

        let mut num_elems_written = 0;
        let mut next_elem = Some(list_index + 1);

        while let Some(elem_index) = next_elem {
            let node = self.state.core.node(elem_index);

            // Skip line/block comments, errors and sexp comments.
            if !node.token.is_data() || node.token.is_sexp_commented_out() {
                next_elem = node.next_sibling();
                continue;
            }

            // … has been reserved. If we're not the last elem, we need to reserve
            // two spaces for the elem and a space separator "… "
            let is_last_elem = num_elems_written + 1 == num_elems;

            if !is_last_elem && !self.compositor.reserve_more_space(2) {
                break;
            }

            if !self.try_typeset_elem_preview(elem_index) {
                // We couldn't write anything; reclaim the two spaces if we weren't the last elem.
                if !is_last_elem {
                    self.compositor.give_back_reserved_space(2);
                }
                break;
            }

            if !is_last_elem {
                // Write the space between elems. We're not at the last elem, so
                // next_sibling must be Some.
                self.write_space_before_elem_or_static_space(node.next_sibling().unwrap());
            }

            num_elems_written += 1;
            next_elem = node.next_sibling();
        }

        if num_elems_written < num_elems {
            self.append_reserved_ellipsis();
        }

        // Write closing paren
        self.append_reserved_node_as_preview(closing_paren);

        true
    }

    fn append_reserved_node_as_preview(&mut self, index: NodeIndex) {
        self.compositor.append_reserved_text(
            Text::SourceRange(self.state.core.node(index).data_range.clone()),
            FragmentSource::NodePreview(index),
        )
    }

    fn try_typeset_elem_preview(&mut self, index: NodeIndex) -> bool {
        let node = self.state.core.node(index);
        match &node.token {
            DocumentToken::Atom(_) => {
                let start = node.data_range.start;
                let content = Text::SourceRange(node.data_range.clone());

                let delimited = self.doc_content[start] == b'"';
                self.compositor.try_append_text(
                    content,
                    delimited,
                    FragmentSource::NodePreview(index),
                    1,
                )
            }
            DocumentToken::StartOfList(list_metadata) => {
                match list_metadata.list_kind {
                    ListKind::RecordField | ListKind::VariantRecord | ListKind::VariantTuple => {
                        self.try_typeset_record_field_or_variant_preview(index)
                    }
                    ListKind::Record | ListKind::Plain => {
                        let extra_to_reserve = if list_metadata.data_length() == 0 {
                            1
                        } else {
                            2
                        };

                        if !self.compositor.reserve_more_space(extra_to_reserve) {
                            return false;
                        }

                        self.append_reserved_node_as_preview(index);
                        if list_metadata.data_length() > 0 {
                            self.append_reserved_ellipsis();
                        }
                        self.append_reserved_node_as_preview(list_metadata.end_index().unwrap());

                        true
                    }
                    ListKind::Singleton => {
                        if !self.compositor.reserve_more_space(2) {
                            return false;
                        }

                        self.append_reserved_node_as_preview(index);

                        // Try showing what's inside.
                        if !self.try_typeset_elem_preview(index + 1) {
                            self.append_reserved_ellipsis();
                        }

                        self.append_reserved_node_as_preview(list_metadata.end_index().unwrap());

                        true
                    }
                    ListKind::DateTime => {
                        // Typeset the DateTime as a normal list so we'll put previews of both
                        // atoms.
                        self.try_typeset_list_preview(index, list_metadata.end_index().unwrap())
                    }
                }
            }
            DocumentToken::Unit {
                sexp_commented_out: _,
            } => {
                // TODO: Handle sexp_commented_out

                if !self.compositor.reserve_more_space(1) {
                    return false;
                }

                self.append_reserved_node_as_preview(index);

                true
            }
            DocumentToken::EndOfList(_)
            | DocumentToken::LineComment
            | DocumentToken::BlockComment
            | DocumentToken::Error(_) => {
                panic!(
                    "Shouldn't be trying to typeset preview of a end of list, comment, or error"
                );
            }
        }
    }

    // Record fields and variants are handled similarly in that we want to make sure we can
    // show _something_ from the first atom (field name or constructor), and otherwise we
    // won't show anything at all.
    fn try_typeset_record_field_or_variant_preview(&mut self, index: NodeIndex) -> bool {
        let (atom_node, second_elem_index) = {
            invariants::record_keys_are_the_first_child_of_record_fields();
            invariants::constructors_are_the_first_child_of_variants();
            (self.state.core.node(index + 1), index + 2)
        };

        let atom_content = Text::SourceRange(atom_node.data_range.clone());
        let is_delimited = self.doc_content[atom_node.data_range.start] == b'"';

        let space_needed_for_first_atom = self
            .compositor
            .min_space_needed_to_show_actual_content(&atom_content, is_delimited);

        // We have space reserved for "…", but we only want to proceed if we can show "(X _)",
        // where X is whatever's needed by the first atom.
        if !self
            .compositor
            .reserve_more_space(space_needed_for_first_atom + 3)
        {
            return false;
        }

        // Opening paren
        self.append_reserved_node_as_preview(index);

        let successfully_wrote_first_atom = self.compositor.try_append_text(
            atom_content,
            is_delimited,
            FragmentSource::NodePreview(index),
            space_needed_for_first_atom,
        );

        if !successfully_wrote_first_atom {
            panic!("failed to write atom after making sure we have enough space");
        }

        let mut next_elem = second_elem_index;
        while !self.state.core.token(next_elem).is_data() {
            next_elem = self
                .state
                .core
                .node(next_elem)
                .next_sibling()
                .expect("RecordField or Variant must have additional data");
        }

        self.write_space_before_elem_or_static_space(next_elem);

        if matches!(
            self.state.core.token(index).list_kind(),
            Some(ListKind::RecordField)
        ) {
            if !self.try_typeset_elem_preview(next_elem) {
                self.append_reserved_ellipsis();
            }
        } else {
            // Don't show nested variant contents; just show an ellipsis.
            self.append_reserved_ellipsis();
        }

        // Closing paren
        self.append_reserved_node_as_preview(
            self.state.core.token(index).list_end_index().unwrap(),
        );

        true
    }

    fn write_space_before_elem_or_static_space(&mut self, index: NodeIndex) {
        let node_start = self.state.core.node(index).data_range.start;
        let space_before = node_start - 1;

        let space_content = if self.doc_content[space_before] == b' ' {
            Text::SourceRange(space_before..node_start)
        } else {
            Text::Static(" ")
        };

        self.compositor
            .append_reserved_text(space_content, FragmentSource::Whitespace);
    }
}

pub struct RenderContext<'a> {
    color_scheme: &'a ColorScheme,
    state: &'a DocState,
    focused_node_indexes: Vec<NodeIndex>,
    focus: NodeIndex,
}

impl<'a> RenderContext<'a> {
    pub fn new(color_scheme: &'a ColorScheme, state: &'a DocState, focus: NodeIndex) -> Self {
        let focused_node_indexes = match state.core.token(focus) {
            DocumentToken::StartOfList(list_metadata) => {
                let mut indexes = vec![focus];

                match list_metadata.list_kind {
                    ListKind::RecordField | ListKind::VariantRecord | ListKind::VariantTuple => {
                        invariants::record_keys_are_the_first_child_of_record_fields();
                        invariants::constructors_are_the_first_child_of_variants();
                        indexes.push(focus + 1);
                    }
                    ListKind::DateTime => {
                        invariants::date_times_have_no_comments_errors_or_commented_out_sexps();
                        indexes.push(focus + 1);
                        indexes.push(focus + 2);
                    }
                    ListKind::Record | ListKind::Singleton | ListKind::Plain => (),
                }

                if let Some(end_index) = list_metadata.end_index() {
                    indexes.push(end_index);
                }

                indexes
            }
            DocumentToken::EndOfList(EndOfListMetadata {
                list_start_index, ..
            }) => {
                vec![*list_start_index, focus]
            }
            _ => vec![focus],
        };

        RenderContext {
            color_scheme,
            state,
            focused_node_indexes,
            focus,
        }
    }

    fn cursor_content(&self, logical_line: &LogicalLine) -> &'static str {
        // If not focused on line, show first collapsible state on line
        // If focused on line, check collapse state of the actual cursor,
        // otherwise have it be the first collapsible state on the line.

        let line_contains_cursor = logical_line.contains_node_index(self.focus);

        let collapsible_nodes_in_line = self
            .state
            .collapsible_nodes
            .range(logical_line.start_index..=logical_line.end_index);

        let mut first_collapse_state = None;
        let mut first_collapse_state_at_or_after_cursor = None;

        for (node_index, collapse_state) in collapsible_nodes_in_line {
            first_collapse_state = first_collapse_state.or(Some(*collapse_state));

            if self.focus <= *node_index {
                first_collapse_state_at_or_after_cursor =
                    first_collapse_state_at_or_after_cursor.or(Some(*collapse_state));
            }
        }

        let Some(first_collapse_state) = first_collapse_state else {
            // No collapsible nodes in the line; only show a cursor if we're focused on it.
            if line_contains_cursor {
                return FOCUSED_LINE;
            } else {
                return NOT_FOCUSED_LINE;
            }
        };

        if line_contains_cursor {
            // The cursor just reflects the cursor or whatever is to the right of it.
            match first_collapse_state_at_or_after_cursor {
                // If the cursor is _after_ all collapsible nodes, they must all be expanded.
                None | Some(CollapseState::Expanded) => FOCUSED_EXPANDED_CONTAINER,
                Some(CollapseState::Collapsed) => FOCUSED_COLLAPSED_CONTAINER,
            }
        } else {
            // If the line doesn't contain the cursor, just reflect the outermost state.
            match first_collapse_state {
                CollapseState::Collapsed => COLLAPSED_CONTAINER,
                CollapseState::Expanded => EXPANDED_CONTAINER,
            }
        }
    }
}

// 'l for lifetime of the line renderer
// 's for the lifetime of rendering the whole screen
// 'h for the lifetime of the highlighter's search match ranges
pub fn style_typeset_line<'l, 's, 'h>(
    core: &'l DocCore,
    context: &'l RenderContext<'s>,
    logical_line: &'l LogicalLine,
    fragments: &'l TypesetLine,
    match_highlighter: &'l mut SearchMatchHighlighter<'h>,
) -> Vec<StyledSegment> {
    let mut styled_segments = vec![];

    for fragment in &fragments.0 {
        let (focused, token_color_scheme) = match fragment.source {
            FragmentSource::Cursor => {
                let cursor = context.cursor_content(logical_line);

                styled_segments.push(StyledSegment {
                    attrs: Attrs::default(),
                    content: Text::Static(cursor),
                });

                continue;
            }
            FragmentSource::Whitespace => (false, context.color_scheme.whitespace),
            FragmentSource::ElidedPreviewNodes | FragmentSource::NodePreview(_) => {
                (false, context.color_scheme.comment)
            }
            FragmentSource::SexpComment(node_index) => {
                let focused = context.focused_node_indexes.contains(&node_index);
                (focused, context.color_scheme.comment)
            }
            FragmentSource::Node(node_index) => {
                let focused = context.focused_node_indexes.contains(&node_index);
                let token_color_scheme = match core.token(node_index) {
                    DocumentToken::StartOfList(_)
                    | DocumentToken::EndOfList(_)
                    | DocumentToken::Unit { .. } => context.color_scheme.parens,
                    DocumentToken::LineComment | DocumentToken::BlockComment => {
                        context.color_scheme.comment
                    }
                    DocumentToken::Error(_) => context.color_scheme.error,
                    DocumentToken::Atom(atom_metadata) => {
                        context.color_scheme.for_atom_kind(atom_metadata.atom_kind)
                    }
                };
                (focused, token_color_scheme)
            }
        };

        let search_attrs = if focused {
            token_color_scheme.focused
        } else {
            token_color_scheme.normal
        };

        match &fragment.text {
            Text::String(_) | Text::Static(_) => {
                styled_segments.push(StyledSegment {
                    content: fragment.text.clone(),
                    attrs: search_attrs.not_a_match,
                });
            }
            Text::SourceRange(range) => {
                styled_segments.extend(match_highlighter.highlight(range.clone(), search_attrs));
            }
        }
    }

    styled_segments
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;
    use std::fmt::Write;
    use std::ops::Range;

    use crate::document::Document;
    use crate::rendering::test_helpers::{
        build_style_map, create_distinct_token_color_scheme, dump_segments_content,
        dump_segments_with_styles,
    };
    use crate::rendering::Attrs;
    use crate::sexp::color_scheme::ColorScheme;
    use crate::sexp::document::test_helpers::*;
    use crate::sexp::document::SexpDocument;

    use insta::assert_snapshot;
    use unicode_width::UnicodeWidthStr;

    const COLOR_SCHEME: ColorScheme = ColorScheme {
        default: Attrs::const_default(),
        whitespace: create_distinct_token_color_scheme(1),
        parens: create_distinct_token_color_scheme(2),
        plain_atom: create_distinct_token_color_scheme(3),
        atom_escape_sequence: create_distinct_token_color_scheme(4),
        atom_invalid_escape_sequence: create_distinct_token_color_scheme(5),
        record_key_atom: create_distinct_token_color_scheme(6),
        constructor_atom: create_distinct_token_color_scheme(7),
        number_atom: create_distinct_token_color_scheme(8),
        bool_atom: create_distinct_token_color_scheme(9),
        date_atom: create_distinct_token_color_scheme(10),
        time_atom: create_distinct_token_color_scheme(11),
        comment: create_distinct_token_color_scheme(12),
        error: create_distinct_token_color_scheme(13),
    };

    fn style_map() -> HashMap<Attrs, String> {
        build_style_map(vec![
            (COLOR_SCHEME.whitespace, "whitespace"),
            (COLOR_SCHEME.parens, "parens"),
            (COLOR_SCHEME.plain_atom, "plain_atom"),
            (COLOR_SCHEME.atom_escape_sequence, "atom_escape_sequence"),
            (
                COLOR_SCHEME.atom_invalid_escape_sequence,
                "atom_invalid_escape_sequence",
            ),
            (COLOR_SCHEME.record_key_atom, "record_key"),
            (COLOR_SCHEME.constructor_atom, "constructor"),
            (COLOR_SCHEME.number_atom, "number"),
            (COLOR_SCHEME.bool_atom, "bool"),
            (COLOR_SCHEME.date_atom, "date"),
            (COLOR_SCHEME.time_atom, "time"),
            (COLOR_SCHEME.comment, "comment"),
            (COLOR_SCHEME.error, "error"),
        ])
    }

    fn render_doc_contents(doc: &SexpDocument, focus: NodeIndex) -> String {
        let render_context = doc.render_context_with_color_scheme(&COLOR_SCHEME, focus);
        let render_context_ref = &render_context;
        let logical_lines = logical_lines(&doc);
        logical_lines
            .into_iter()
            .flat_map(move |logical_line| {
                doc.typeset_logical_line(&logical_line)
                    .0
                    .into_iter()
                    .map(move |typeset_line| {
                        dump_segments_content(
                            style_typeset_line(
                                &doc.state.core,
                                render_context_ref,
                                &logical_line,
                                &typeset_line,
                                &mut SearchMatchHighlighter::empty(),
                            ),
                            doc.raw_bytes_for_searching(),
                        )
                    })
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_doc_line_contents(doc: &SexpDocument, line: usize, focus: NodeIndex) -> String {
        let render_context = doc.render_context_with_color_scheme(&COLOR_SCHEME, focus);
        let logical_lines = logical_lines(&doc);

        doc.typeset_logical_line(&logical_lines[line])
            .0
            .into_iter()
            .map(|typeset_line| {
                dump_segments_content(
                    style_typeset_line(
                        &doc.state.core,
                        &render_context,
                        &logical_lines[line],
                        &typeset_line,
                        &mut SearchMatchHighlighter::empty(),
                    ),
                    doc.raw_bytes_for_searching(),
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_doc_line_with_search_matches(
        doc: &SexpDocument,
        line: usize,
        focus: NodeIndex,
        matches: &[Range<usize>],
        current_match: Option<usize>,
    ) -> String {
        let render_context = doc.render_context_with_color_scheme(&COLOR_SCHEME, focus);
        let logical_lines = logical_lines(&doc);
        let typeset_lines = doc.typeset_logical_line(&logical_lines[line]);

        let mut s = String::new();
        let mut highlighter = SearchMatchHighlighter::new(matches, current_match);
        for (i, typeset_line) in typeset_lines.0.iter().enumerate() {
            if i > 0 {
                s.push_str("\n\n");
            }

            s.push_str(
                dump_segments_with_styles(
                    style_typeset_line(
                        &doc.state.core,
                        &render_context,
                        &logical_lines[line],
                        &typeset_line,
                        &mut highlighter,
                    ),
                    doc.raw_bytes_for_searching(),
                    &style_map(),
                )
                .as_str(),
            );
        }

        s
    }

    fn render_doc_line_with_styles(doc: &SexpDocument, line: usize, focus: NodeIndex) -> String {
        render_doc_line_with_search_matches(doc, line, focus, &[], None)
    }

    #[test]
    fn test_basic_render_styling() {
        let mut doc = new_doc(b"((num_field 123)(bool_field true))");
        assert_snapshot!(render_doc_contents(&doc, NodeIndex(0)), @r"
        ((num_field 123)
         (bool_field true))
        ");

        doc.resize(nz(10));
        assert_snapshot!(render_doc_contents(&doc, NodeIndex(0)), @r"
        ((num_fiel
        d 123)
         (bool_fie
        ld true))
        ");

        assert_snapshot!(render_doc_line_with_styles(&doc, 0, NodeIndex(1)), @r"
        text: ((num_fiel
              012.......
        0: parens                    : range(0..1)
        1: parens!                   : range(1..2)
        2: record_key!               : range(2..10)

        text: d 123)
              012..3
        0: record_key!               : range(10..11)
        1: whitespace                : range(11..12)
        2: number                    : range(12..15)
        3: parens!                   : range(15..16)
        ");

        assert_snapshot!(render_doc_line_with_styles(&doc, 1, NodeIndex(0)), @r"
        text:  (bool_fie
              012.......
        0: whitespace                : static
        1: parens                    : range(17..18)
        2: record_key                : range(18..26)

        text: ld true))
              0.12...34
        0: record_key                : range(26..28)
        1: whitespace                : range(28..29)
        2: bool                      : range(29..33)
        3: parens                    : range(33..34)
        4: parens!                   : range(34..35)
        ");

        let _new_cursor = doc.collapse_or_move_cursor_left_or_up(&NodeIndex(0));
        doc.resize(nz(50));
        assert_snapshot!(render_doc_line_with_styles(&doc, 0, NodeIndex(0)), @r"
        text: ((num_field 123) (bool_field true))
              012........34..5678.........9a...bc
        0: comment                   : range(0..1)
        1: comment                   : range(1..2)
        2: comment                   : range(2..11)
        3: whitespace                : range(11..12)
        4: comment                   : range(12..15)
        5: comment                   : range(15..16)
        6: whitespace                : range(16..17)
        7: comment                   : range(17..18)
        8: comment                   : range(18..28)
        9: whitespace                : range(28..29)
        a: comment                   : range(29..33)
        b: comment                   : range(33..34)
        c: comment                   : range(34..35)
        ");

        doc.resize(nz(30));
        assert_snapshot!(render_doc_line_with_styles(&doc, 0, NodeIndex(0)), @r"
        text: ((num_field 123) (bool_fi… …))
              012........34..5678......9abcd
        0: comment                   : range(0..1)
        1: comment                   : range(1..2)
        2: comment                   : range(2..11)
        3: whitespace                : range(11..12)
        4: comment                   : range(12..15)
        5: comment                   : range(15..16)
        6: whitespace                : range(16..17)
        7: comment                   : range(17..18)
        8: comment                   : range(18..25)
        9: comment                   : static
        a: whitespace                : range(28..29)
        b: comment                   : static
        c: comment                   : range(33..34)
        d: comment                   : range(34..35)
        ");
    }

    #[test]
    fn test_highlight_search_matches() {
        let matches = [3..5, 8..12];
        let mut doc = new_doc(b"(12345678\nabcdefghi)");
        assert_snapshot!(render_doc_contents(&doc, NodeIndex(0)), @r"
        (12345678
         abcdefghi)
        ");

        assert_snapshot!(render_doc_line_with_search_matches(&doc, 0, NodeIndex(0), &matches, Some(1)), @r"
        text: (12345678
              01.2.3..4
        0: parens!                   : range(0..1)
        1: number                    : range(1..3)
        2: number (match)            : range(3..5)
        3: number                    : range(5..8)
        4: number (curr match)       : range(8..9)
        ");

        assert_snapshot!(render_doc_line_with_search_matches(&doc, 1, NodeIndex(0), &matches, Some(1)), @r"
        text:  abcdefghi)
              01.2......3
        0: whitespace                : static
        1: plain_atom (curr match)   : range(10..12)
        2: plain_atom                : range(12..19)
        3: parens!                   : range(19..20)
        ");

        let _new_cursor = doc.collapse_or_move_cursor_left_or_up(&NodeIndex(0));
        assert_snapshot!(render_doc_line_with_search_matches(&doc, 0, NodeIndex(0), &matches, Some(1)), @r"
        text: (12345678 abcdefghi)
              01.2.3..456.7......8
        0: comment                   : range(0..1)
        1: comment                   : range(1..3)
        2: comment (match)           : range(3..5)
        3: comment                   : range(5..8)
        4: comment (curr match)      : range(8..9)
        5: whitespace (curr match)   : range(9..10)
        6: comment (curr match)      : range(10..12)
        7: comment                   : range(12..19)
        8: comment                   : range(19..20)
        ");
    }

    fn render_line_preview_at_different_widths(
        doc: &mut SexpDocument,
        line: usize,
        widths: impl IntoIterator<Item = usize>,
    ) -> String {
        widths
            .into_iter()
            .map(|width| {
                doc.resize(nz(width));
                let mut formatted = format!(
                    "{width:<2}: {}",
                    render_doc_line_contents(&doc, line, NodeIndex(0))
                );
                let actual_width = UnicodeWidthStr::width(formatted.as_str()) - 4;
                if actual_width != width {
                    let _ = write!(formatted, "   (only used {actual_width})");
                }
                formatted
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn render_top_level_preview_at_different_widths(
        input: &'static [u8],
        widths: impl IntoIterator<Item = usize>,
    ) -> String {
        let mut doc = new_doc(input);
        let _new_cursor = doc.collapse_or_move_cursor_left_or_up(&NodeIndex(0));
        render_line_preview_at_different_widths(&mut doc, 0, widths)
    }

    fn render_second_record_value_preview_at_different_widths(
        input: &'static [u8],
        widths: impl IntoIterator<Item = usize>,
    ) -> String {
        let mut doc = new_doc(input);
        // ((k v) (
        // 012 34 5
        let _new_cursor = doc.collapse_or_move_cursor_left_or_up(&NodeIndex(5));
        render_line_preview_at_different_widths(&mut doc, 1, widths)
    }

    #[test]
    fn test_previews() {
        use render_top_level_preview_at_different_widths as f;

        // Simple record
        assert_snapshot!(f(b"((abc 123) (def 456))", vec![21, 20, 19, 18, 17, 13, 12, 11, 10, 9, 3, 2]), @r"
        21: ((abc 123) (def 456))
        20: ((abc 123) (def 4…))
        19: ((abc 123) (def …))
        18: ((abc 123) (d… …))
        17: ((abc 123) …)   (only used 13)
        13: ((abc 123) …)
        12: ((abc 1…) …)
        11: ((abc …) …)
        10: ((a… …) …)
        9 : (…)   (only used 3)
        3 : (…)
        2 : …   (only used 1)
        ");

        // Variant tuple
        assert_snapshot!(f(b"(Var 1 two)", vec![11, 10, 9, 8, 7, 6, 5, 2]), @r"
        11: (Var 1 two)
        10: (Var 1 t…)
        9 : (Var 1 …)
        8 : (Var …)   (only used 7)
        7 : (Var …)
        6 : (V… …)
        5 : (…)   (only used 3)
        2 : …   (only used 1)
        ");

        // Variant record
        assert_snapshot!(f(b"(Var (abc 123) (def 456))", vec![25, 24, 23, 22, 21]), @r"
        25: (Var (abc 123) (def 456))
        24: (Var (abc 123) (def 4…))
        23: (Var (abc 123) (def …))
        22: (Var (abc 123) (d… …))
        21: (Var (abc 123) …)   (only used 17)
        ");

        // Record with single value variant key
        // Someday: This shouldn't truncate the 222.
        assert_snapshot!(f(b"((a 1) (b (Var 222)))", vec![30, 18]), @r"
        30: ((a 1) (b (Var …)))   (only used 19)
        18: ((a 1) (b (V… …)))
        ");

        use render_second_record_value_preview_at_different_widths as g;
        // Someday: This should be: (bee (Var ((x 2) (y 3)))))
        assert_snapshot!(g(b"((a 1) (bee (Var ((x 2) (y 3)))))", vec![30, 16]), @r"
        30:  (bee (Var (…))))   (only used 17)
        16:  (bee (Var …)))   (only used 15)
        ");
    }
}
