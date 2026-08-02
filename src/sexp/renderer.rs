use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::ops::Range;
use std::rc::Rc;

use crate::rendering::{Attrs, Compositor, HighlightType, MatchHighlighter, StyledSegment, Text};
use crate::sexp::color_scheme::ColorScheme;
use crate::sexp::core::{
    invariants, DocCore, DocumentToken, EndOfListMetadata, ListKind, NodeIndex,
};
use crate::sexp::document::{CollapseState, TypesetLine, TypesetLines};
use crate::sexp::layout::LogicalLine;

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
    core: &'a DocCore,
    collapsible_nodes: &'a BTreeMap<NodeIndex, CollapseState>,
    compositor: Compositor<'a, FragmentSource>,
    include_cursor: bool,
}

pub fn typeset_logical_line(
    logical_line: &LogicalLine,
    doc_width: NonZeroUsize,
    core: &DocCore,
    collapsible_nodes: &BTreeMap<NodeIndex, CollapseState>,
    include_cursor: bool,
) -> TypesetLines {
    let doc_content = core.pretty_printed.data();
    let compositor = Compositor::new(doc_content, doc_width);

    let mut typesetter = Typesetter {
        logical_line,
        doc_content,
        core,
        collapsible_nodes,
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
            let node = self.core.node(node_index);

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
                    if let Some(CollapseState::Collapsed) = self.collapsible_nodes.get(&node_index)
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
            while closing_paren <= self.core.last_node_index_of_part_of_completed_sexp.unwrap() {
                if !matches!(self.core.token(closing_paren), DocumentToken::EndOfList(_)) {
                    break;
                }

                self.compositor.append_text(
                    Text::SourceRange(self.core.node(closing_paren).data_range.clone()),
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

        while closing_paren <= self.core.last_node_index_of_part_of_completed_sexp.unwrap() {
            if !matches!(self.core.token(closing_paren), DocumentToken::EndOfList(_)) {
                break;
            }

            count += 1;
            closing_paren = closing_paren + 1;
        }

        count
    }

    fn add_trailing_paren_after_collapsed_preview(&mut self, mut closing_paren: NodeIndex) {
        while closing_paren <= self.core.last_node_index_of_part_of_completed_sexp.unwrap() {
            if !matches!(self.core.token(closing_paren), DocumentToken::EndOfList(_)) {
                break;
            }

            self.compositor.append_reserved_text(
                Text::SourceRange(self.core.node(closing_paren).data_range.clone()),
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
        let list_metadata = self.core.token(list_index).list_metadata();
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
                self.core.token(inner_list_index)
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
            let node = self.core.node(elem_index);

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
            Text::SourceRange(self.core.node(index).data_range.clone()),
            FragmentSource::NodePreview(index),
        )
    }

    fn try_typeset_elem_preview(&mut self, index: NodeIndex) -> bool {
        let node = self.core.node(index);
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
            (self.core.node(index + 1), index + 2)
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
        while !self.core.token(next_elem).is_data() {
            next_elem = self
                .core
                .node(next_elem)
                .next_sibling()
                .expect("RecordField or Variant must have additional data");
        }

        self.write_space_before_elem_or_static_space(next_elem);

        if matches!(
            self.core.token(index).list_kind(),
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
        self.append_reserved_node_as_preview(self.core.token(index).list_end_index().unwrap());

        true
    }

    fn write_space_before_elem_or_static_space(&mut self, index: NodeIndex) {
        let node_start = self.core.node(index).data_range.start;
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
    collapsible_nodes: &'a BTreeMap<NodeIndex, CollapseState>,
    focused_node_indexes: Vec<NodeIndex>,
    focus: NodeIndex,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        color_scheme: &'a ColorScheme,
        core: &'a DocCore,
        collapsible_nodes: &'a BTreeMap<NodeIndex, CollapseState>,
        focus: NodeIndex,
    ) -> Self {
        let focused_node_indexes = match core.token(focus) {
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
            collapsible_nodes,
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

// 'l for lifetime of the line renderer, 's for the lifetime of rendering the whole screen
pub fn style_typeset_line<'l, 's>(
    core: &'l DocCore,
    context: &'l RenderContext<'s>,
    logical_line: &'l LogicalLine,
    fragments: &'l TypesetLine,
    search_matches: &[Range<usize>],
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

        let (attrs, search_match_attrs) = if focused {
            (
                token_color_scheme.focused,
                token_color_scheme.focused_search_match,
            )
        } else {
            (token_color_scheme.normal, token_color_scheme.search_match)
        };

        match &fragment.text {
            Text::String(_) | Text::Static(_) => {
                styled_segments.push(StyledSegment {
                    content: fragment.text.clone(),
                    attrs,
                });
            }
            Text::SourceRange(range) => {
                for (highlight_range, highlight_type) in
                    MatchHighlighter::new(range.clone(), search_matches)
                {
                    let attrs = match highlight_type {
                        HighlightType::NotAMatch => attrs,
                        HighlightType::Match => search_match_attrs,
                    };

                    styled_segments.push(StyledSegment {
                        content: Text::SourceRange(highlight_range),
                        attrs,
                    });
                }
            }
        }
    }

    styled_segments
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::collections::HashMap;

    use crate::document::Document;
    use crate::rendering::test_helpers::{build_style_map, dump_segments};
    use crate::rendering::{Attrs, Color, TokenColorScheme};
    use crate::sexp::color_scheme::ColorScheme;
    use crate::sexp::document::test_helpers::*;
    use crate::sexp::document::SexpDocument;

    use insta::assert_snapshot;

    const fn style(n: u8) -> TokenColorScheme {
        let normal = Attrs::from_fg(Color::Rgb256(2 * n));
        let focused = Attrs::from_fg(Color::Rgb256(2 * n + 1));

        TokenColorScheme {
            normal,
            focused,
            search_match: Attrs::const_default(),
            focused_search_match: Attrs::const_default(),
        }
    }

    const COLOR_SCHEME: ColorScheme = ColorScheme {
        whitespace: style(0),
        parens: style(1),
        plain_atom: style(2),
        atom_escape_sequence: style(3),
        atom_invalid_escape_sequence: style(4),
        record_key_atom: style(5),
        constructor_atom: style(6),
        number_atom: style(7),
        bool_atom: style(8),
        date_atom: style(9),
        time_atom: style(10),
        comment: style(11),
        error: style(12),
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

    fn render_doc_line(doc: &SexpDocument, line: usize, focus: NodeIndex) -> String {
        let render_context = doc.render_context_with_color_scheme(&COLOR_SCHEME, focus);
        let logical_lines = logical_lines(&doc);
        let typeset_lines = doc.typeset_logical_line(&logical_lines[line]);

        let mut s = String::new();
        for (i, typeset_line) in typeset_lines.0.iter().enumerate() {
            if i > 0 {
                s.push_str("\n\n");
            }

            s.push_str(
                dump_segments(
                    style_typeset_line(
                        &doc.core,
                        &render_context,
                        &logical_lines[line],
                        &typeset_line,
                        &[],
                    ),
                    doc.raw_bytes_for_searching(),
                    &style_map(),
                )
                .as_str(),
            );
        }

        s
    }

    #[test]
    fn test_basic_render() {
        let mut doc = new_doc(b"((num_field 123)(bool_field true))");
        doc.resize(nz(10));
        assert_snapshot!(dump_with_byte_indexes(&doc), @r"
        0..=4  :   0..16  : ((num_field 123)
        5..=9  :  17..35  :  (bool_field true))
        ");

        assert_snapshot!(render_doc_line(&doc, 0, NodeIndex(1)), @r"
        text: ((num_fiel
              012.......
        0: parens                    : range(0..1)
        1: parens (focused)          : range(1..2)
        2: record_key (focused)      : range(2..10)

        text: d 123)
              012..3
        0: record_key (focused)      : range(10..11)
        1: whitespace                : range(11..12)
        2: number                    : range(12..15)
        3: parens (focused)          : range(15..16)
        ");

        assert_snapshot!(render_doc_line(&doc, 1, NodeIndex(0)), @r"
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
        4: parens (focused)          : range(34..35)
        ");
    }
}
