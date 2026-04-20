use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::rc::Rc;

use crate::rendering::{Compositor, PreHighlightingStyledSegment, Segment, Text, TokenColorScheme};
use crate::sexp::color_scheme::ColorScheme;
use crate::sexp::core::{
    invariants, AtomKind, DocCore, DocumentToken, EndOfListMetadata, ListKind, NodeIndex,
};
use crate::sexp::document::CollapseState;
use crate::sexp::layout::LogicalLine;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SegmentKind {
    Whitespace,
    Parens,
    PlainAtom,
    AtomEscapeSequence,
    AtomInvalidEscapeSequence,
    RecordKeyAtom,
    ConstructorAtom,
    NumberAtom,
    BoolAtom,
    DateAtom,
    TimeAtom,
    Comment,
    Error,
    Preview,
}

impl SegmentKind {
    pub fn for_atom_kind(atom_kind: AtomKind) -> Self {
        match atom_kind {
            AtomKind::Constructor => SegmentKind::ConstructorAtom,
            AtomKind::RecordKey => SegmentKind::RecordKeyAtom,
            AtomKind::Number => SegmentKind::NumberAtom,
            AtomKind::Bool => SegmentKind::BoolAtom,
            AtomKind::Date => SegmentKind::DateAtom,
            AtomKind::Time => SegmentKind::TimeAtom,
            AtomKind::StringifiedList | AtomKind::Plain => SegmentKind::PlainAtom,
        }
    }

    pub fn color_scheme(self, color_scheme: &ColorScheme) -> TokenColorScheme {
        match self {
            SegmentKind::Whitespace => color_scheme.whitespace,
            SegmentKind::Parens => color_scheme.parens,
            SegmentKind::PlainAtom => color_scheme.plain_atom,
            SegmentKind::AtomEscapeSequence => color_scheme.atom_escape_sequence,
            SegmentKind::AtomInvalidEscapeSequence => color_scheme.atom_invalid_escape_sequence,
            SegmentKind::RecordKeyAtom => color_scheme.record_key_atom,
            SegmentKind::ConstructorAtom => color_scheme.constructor_atom,
            SegmentKind::NumberAtom => color_scheme.number_atom,
            SegmentKind::BoolAtom => color_scheme.bool_atom,
            SegmentKind::DateAtom => color_scheme.date_atom,
            SegmentKind::TimeAtom => color_scheme.time_atom,
            SegmentKind::Comment => color_scheme.comment,
            SegmentKind::Error => color_scheme.error,
            SegmentKind::Preview => color_scheme.comment,
        }
    }
}

struct Typesetter<'a> {
    logical_line: &'a LogicalLine,
    doc_content: &'a [u8],
    core: &'a DocCore,
    collapsible_nodes: &'a BTreeMap<NodeIndex, CollapseState>,
    compositor: Compositor<'a, NodeIndex, SegmentKind>,
}

pub fn typeset_logical_line(
    logical_line: &LogicalLine,
    doc_width: NonZeroUsize,
    core: &DocCore,
    collapsible_nodes: &BTreeMap<NodeIndex, CollapseState>,
) -> Vec<Vec<Segment<NodeIndex, SegmentKind>>> {
    let doc_content = core.pretty_printed.data();
    let compositor = Compositor::new(doc_content, doc_width);

    let mut typesetter = Typesetter {
        logical_line,
        doc_content,
        core,
        collapsible_nodes,
        compositor,
    };

    typesetter.typeset();

    typesetter.compositor.finish()
}

impl<'a> Typesetter<'a> {
    fn typeset(&mut self) {
        if self.logical_line.indentation > 0 {
            self.compositor.append_spaces(
                self.logical_line.indentation,
                SegmentKind::Whitespace,
                None,
            );
        }

        let mut prev_node_end_index = None;
        let mut collapsed_start_and_end = None;

        for node_index in self.logical_line.node_indexes() {
            let node = self.core.node(node_index);

            if let Some(end_index) = prev_node_end_index {
                let whitespace_range = end_index..(node.data_range.start);
                if whitespace_range.len() > 0 {
                    self.compositor.append_content(
                        Text::SourceRange(whitespace_range),
                        SegmentKind::Whitespace,
                        None,
                    );
                }
            }

            prev_node_end_index = Some(node.data_range.end);

            let segment_kind;
            let mut content = Text::SourceRange(node.data_range.clone());

            match &node.token {
                DocumentToken::StartOfList(list_metadata) => {
                    // Check if commented out
                    if let Some(CollapseState::Collapsed) = self.collapsible_nodes.get(&node_index)
                    {
                        collapsed_start_and_end = Some((node_index, list_metadata.end_index()));
                        break;
                    }

                    segment_kind = SegmentKind::Parens;
                }
                DocumentToken::EndOfList(end_of_list_metadata) => {
                    segment_kind = SegmentKind::Parens;
                }
                DocumentToken::Atom(atom_metadata) => {
                    segment_kind = SegmentKind::for_atom_kind(atom_metadata.atom_kind);
                    // Check if commented out
                }
                DocumentToken::Unit { commented_out: _ } => {
                    segment_kind = SegmentKind::Parens;
                    // Check if commented out
                }
                DocumentToken::LineComment | DocumentToken::BlockComment => {
                    segment_kind = SegmentKind::Comment;
                }
                DocumentToken::Error(error_metadata) => {
                    segment_kind = SegmentKind::Error;
                    content = Text::String((
                        Rc::new(error_metadata.message.clone()),
                        0..error_metadata.message.len(),
                    ));
                }
            }

            self.compositor
                .append_content(content, segment_kind, Some(node_index));
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
                .append_content(Text::ellipsis(), SegmentKind::Preview, None);

            let mut closing_paren = close_index + 1;
            while closing_paren <= self.core.last_node_index_of_part_of_completed_sexp.unwrap() {
                if !matches!(self.core.token(closing_paren), DocumentToken::EndOfList(_)) {
                    break;
                }

                self.compositor.append_content(
                    Text::SourceRange(self.core.node(closing_paren).data_range.clone()),
                    SegmentKind::Parens,
                    Some(closing_paren),
                );

                closing_paren = closing_paren + 1;
            }
        }
    }

    fn append_reserved_ellipsis(&mut self) {
        self.compositor
            .append_reserved_content(Text::ellipsis(), SegmentKind::Preview, None);
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

            self.compositor.append_reserved_content(
                Text::SourceRange(self.core.node(closing_paren).data_range.clone()),
                SegmentKind::Parens,
                Some(closing_paren),
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

        let extra_needed_space = if num_elems == 0 { 1 } else { 2 };
        if !self.compositor.reserve_more_space(extra_needed_space) {
            return false;
        }

        // Write opening paren
        self.append_reserved_node_as_preview(list_index);

        let mut num_elems_written = 0;
        let mut next_elem = Some(list_index + 1);

        while let Some(elem_index) = next_elem {
            let node = self.core.node(elem_index);

            // Skip comments and errors.
            if !node.token.is_data() {
                next_elem = node.next_sibling;
                continue;
            }

            // … has been reserved. If we're not the last elem, we need to reserve
            // two spaces for the elem and a space separator "… "
            let is_last_elem = num_elems_written + 1 == num_elems;

            if !is_last_elem && !self.compositor.reserve_more_space(2) {
                break;
            }

            if !self.try_typeset_elem_preview(elem_index) {
                // We couldn't write anything; reclaim the two spaces.
                self.compositor.give_back_reserved_space(2);
                break;
            }

            if !is_last_elem {
                // Write the space between elems. We're not at the last elem, so
                // next_sibling must be Some.
                self.write_space_before_elem_or_static_space(node.next_sibling.unwrap());
            }

            num_elems_written += 1;
            next_elem = node.next_sibling;
        }

        if num_elems_written < num_elems {
            self.append_reserved_ellipsis();
        }

        // Write closing paren
        self.append_reserved_node_as_preview(closing_paren);

        true
    }

    fn append_reserved_node_as_preview(&mut self, index: NodeIndex) {
        self.compositor.append_reserved_content(
            Text::SourceRange(self.core.node(index).data_range.clone()),
            SegmentKind::Preview,
            Some(index),
        )
    }

    fn try_typeset_elem_preview(&mut self, index: NodeIndex) -> bool {
        let node = self.core.node(index);
        match &node.token {
            DocumentToken::Atom(_) => {
                let start = node.data_range.start;
                let content = Text::SourceRange(node.data_range.clone());

                if self.doc_content[start] == b'"' {
                    self.compositor.try_append_delimited_content(
                        content,
                        SegmentKind::Preview,
                        Some(index),
                        1,
                    )
                } else {
                    self.compositor.try_append_content(
                        content,
                        SegmentKind::Preview,
                        Some(index),
                        1,
                    )
                }
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
            DocumentToken::Unit { commented_out: _ } => {
                // TODO: Handle commented_out

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

        let space_needed_for_first_atom = if is_delimited {
            self.compositor
                .min_space_needed_to_show_actual_delimited_content(&atom_content)
        } else {
            self.compositor
                .min_space_needed_to_show_actual_content(&atom_content)
        };

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

        let successfully_wrote_first_atom = if is_delimited {
            self.compositor.try_append_delimited_content(
                atom_content,
                SegmentKind::Preview,
                Some(index),
                space_needed_for_first_atom,
            )
        } else {
            self.compositor.try_append_content(
                atom_content,
                SegmentKind::Preview,
                Some(index),
                space_needed_for_first_atom,
            )
        };

        // Very unlikely, but could happen if there are two columns left at the end of
        // the line, and all the remaining space is reserved, and the atom starts with
        // a wide character. The opening paren will take the first column, but then the
        // atom can't get split across the lines.
        if !successfully_wrote_first_atom {
            self.compositor
                .give_back_reserved_space(space_needed_for_first_atom.saturating_sub(1));
            self.append_reserved_ellipsis();
        }

        let mut next_elem = second_elem_index;
        while !self.core.token(next_elem).is_data() {
            next_elem = self
                .core
                .node(next_elem)
                .next_sibling
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
            .append_reserved_content(space_content, SegmentKind::Preview, None);
    }
}

pub struct RenderContext<'a> {
    color_scheme: &'a ColorScheme,
    focused_node_indexes: Vec<NodeIndex>,
}

impl<'a> RenderContext<'a> {
    pub fn new(color_scheme: &'a ColorScheme, core: &'a DocCore, focus: NodeIndex) -> Self {
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
            focused_node_indexes,
        }
    }
}

// 'l for lifetime of the line renderer, 's for the lifetime of rendering the whole screen
pub fn style_typeset_line<'l, 's>(
    context: &'l RenderContext<'s>,
    segments: &'l Vec<Segment<NodeIndex, SegmentKind>>,
) -> Vec<PreHighlightingStyledSegment> {
    segments
        .iter()
        .map(|segment| {
            let focused = if let Some(node_index) = segment.doc_ref {
                context.focused_node_indexes.contains(&node_index)
            } else {
                false
            };

            let token_color_scheme = segment.kind.color_scheme(&context.color_scheme);

            let (attrs, search_match_attrs) = if focused {
                (
                    token_color_scheme.focused,
                    token_color_scheme.focused_search_match,
                )
            } else {
                (token_color_scheme.normal, token_color_scheme.search_match)
            };

            PreHighlightingStyledSegment {
                attrs,
                search_match_attrs,
                content: segment.content.clone(),
            }
        })
        .collect::<Vec<_>>()
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
    use crate::sexp::document::{CollapseState, SexpDocument};

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
        for (i, typeset_line) in typeset_lines.iter().enumerate() {
            if i > 0 {
                s.push_str("\n\n");
            }

            s.push_str(
                dump_segments(
                    style_typeset_line(&render_context, &typeset_line),
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
        doc.resize(10);
        assert_snapshot!(dump_with_byte_indexes(&doc), @r"
        0..=4  :   0..=16  : ((num_field 123)
        5..=9  :  17..=35  :  (bool_field true))
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
