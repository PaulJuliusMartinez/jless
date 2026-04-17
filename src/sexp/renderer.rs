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
        }
    }
}

pub fn typeset_logical_line(
    logical_line: &LogicalLine,
    doc_width: NonZeroUsize,
    core: &DocCore,
    collapsible_nodes: &BTreeMap<NodeIndex, CollapseState>,
) -> Vec<Vec<Segment<NodeIndex, SegmentKind>>> {
    let mut lines = Compositor::new(core.pretty_printed.data(), doc_width);

    if logical_line.indentation > 0 {
        lines.append_spaces(logical_line.indentation, SegmentKind::Whitespace, None);
    }

    let mut prev_node_end_index = None;
    let mut paren_after_collapsed_section = None;

    for node_index in logical_line.node_indexes() {
        let node = core.node(node_index);

        if let Some(end_index) = prev_node_end_index {
            let whitespace_range = end_index..(node.data_range.start);
            if whitespace_range.len() > 0 {
                lines.append_content(
                    Text::SourceRange(whitespace_range),
                    SegmentKind::Whitespace,
                    None,
                );
            }
        }

        prev_node_end_index = Some(node.data_range.end);

        let segment_kind;
        let mut content = Text::SourceRange(node.data_range.clone());
        let mut remainder_is_collapsed = false;

        match &node.token {
            DocumentToken::StartOfList(list_metadata) => {
                segment_kind = SegmentKind::Parens;
                // Check if commented out
                if let Some(CollapseState::Collapsed) = collapsible_nodes.get(&node_index) {
                    remainder_is_collapsed = true;
                    paren_after_collapsed_section = list_metadata.end_index();
                }
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

        lines.append_content(content, segment_kind, Some(node_index));

        if remainder_is_collapsed {
            break;
        }
    }

    // If we had a collapsed section, we'll loop over all the coalesced closing parens.
    if let Some(mut closing_paren) = paren_after_collapsed_section {
        lines.append_content(Text::Static("…"), SegmentKind::Comment, None);

        while closing_paren <= core.last_node_index_of_part_of_completed_sexp.unwrap() {
            if !matches!(core.token(closing_paren), DocumentToken::EndOfList(_)) {
                break;
            }

            lines.append_content(
                Text::SourceRange(core.node(closing_paren).data_range.clone()),
                SegmentKind::Parens,
                Some(closing_paren),
            );

            closing_paren = closing_paren + 1;
        }
    }

    lines.finish()
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
        0: whitespace                : spaces
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
