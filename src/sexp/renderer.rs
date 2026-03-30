use std::collections::BTreeMap;
use std::rc;

use crate::rendering::{PreHighlightingStyledSegment, Text};
use crate::sexp::color_scheme::ColorScheme;
use crate::sexp::core::{DocCore, DocumentToken, EndOfListMetadata, ListKind, NodeIndex};
use crate::sexp::document::CollapseState;
use crate::sexp::layout::LogicalLine;

pub struct RenderContext<'a> {
    color_scheme: &'a ColorScheme,
    core: &'a DocCore,
    collapsible_nodes: &'a BTreeMap<NodeIndex, CollapseState>,
    focused_node_indexes: Vec<NodeIndex>,
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
                        indexes.push(focus + 1);
                    }
                    ListKind::DateTime => {
                        indexes.push(focus + 1);
                        indexes.push(focus + 2);
                    }
                    ListKind::Record | ListKind::Singleton | ListKind::Unit | ListKind::Plain => (),
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
            core,
            collapsible_nodes,
            focused_node_indexes,
        }
    }
}

// 'l for lifetime of the line renderer, 's for the lifetime of rendering the whole screen
struct LineRenderer<'l, 's> {
    context: &'l RenderContext<'s>,
    line: &'l LogicalLine,
}

pub fn render_line<'l, 's>(
    context: &'l RenderContext<'s>,
    line: &'l LogicalLine,
) -> Vec<PreHighlightingStyledSegment> {
    let mut line_renderer = LineRenderer { context, line };

    line_renderer.render()
}

impl<'l, 's> LineRenderer<'l, 's> {
    fn render(&mut self) -> Vec<PreHighlightingStyledSegment> {
        let mut segments = vec![];

        if self.line.indentation > 0 {
            let whitespace = self.context.color_scheme.whitespace;
            segments.push(PreHighlightingStyledSegment {
                attrs: whitespace.normal,
                search_match_attrs: whitespace.search_match,
                content: Text::Spaces(self.line.indentation),
            });
        }

        let mut prev_node_end_index = None;
        let mut paren_after_collapsed_section = None;
        let color_scheme = &self.context.color_scheme;

        for node_index in self.line.node_indexes() {
            let node = self.context.core.node(node_index);

            if let Some(end_index) = prev_node_end_index {
                let whitespace_range = end_index..(node.data_range.start);
                if whitespace_range.len() > 0 {
                    segments.push(PreHighlightingStyledSegment {
                        attrs: color_scheme.whitespace.normal,
                        search_match_attrs: color_scheme.whitespace.search_match,
                        content: Text::SourceRange(whitespace_range),
                    });
                }
            }

            prev_node_end_index = Some(node.data_range.end);

            let focused = self.context.focused_node_indexes.contains(&node_index);

            let token_color_scheme;
            let mut content = Text::SourceRange(node.data_range.clone());
            let mut remainder_is_collapsed = false;

            match &node.token {
                DocumentToken::StartOfList(list_metadata) => {
                    token_color_scheme = color_scheme.parens;
                    // Check if commented out
                    if let Some(CollapseState::Collapsed) =
                        self.context.collapsible_nodes.get(&node_index)
                    {
                        remainder_is_collapsed = true;
                        paren_after_collapsed_section = list_metadata.end_index();
                    }
                }
                DocumentToken::EndOfList(end_of_list_metadata) => {
                    token_color_scheme = color_scheme.parens;
                }
                DocumentToken::Atom(atom_metadata) => {
                    token_color_scheme = color_scheme.for_atom_kind(atom_metadata.atom_kind);
                    // Check if commented out
                }
                DocumentToken::Unit { commented_out: _ } => {
                    token_color_scheme = color_scheme.parens;
                    // Check if commented out
                }
                DocumentToken::LineComment | DocumentToken::BlockComment => {
                    token_color_scheme = color_scheme.comment;
                }
                DocumentToken::Error(error_metadata) => {
                    token_color_scheme = color_scheme.error;
                    content = Text::String(rc::Rc::new(error_metadata.message.clone()));
                }
            }

            let (attrs, search_match_attrs) = if focused {
                (
                    token_color_scheme.focused,
                    token_color_scheme.focused_search_match,
                )
            } else {
                (token_color_scheme.normal, token_color_scheme.search_match)
            };

            segments.push(PreHighlightingStyledSegment {
                attrs,
                search_match_attrs,
                content,
            });

            if remainder_is_collapsed {
                break;
            }
        }

        // If we had a collapsed section, we'll loop over all the coalesced closing parens.
        if let Some(mut closing_paren) = paren_after_collapsed_section {
            segments.push(PreHighlightingStyledSegment {
                attrs: color_scheme.comment.normal,
                search_match_attrs: color_scheme.comment.search_match,
                content: Text::String(rc::Rc::new("…".to_string())),
            });

            while closing_paren
                <= self
                    .context
                    .core
                    .last_node_index_of_part_of_completed_sexp
                    .unwrap()
            {
                if !matches!(
                    self.context.core.token(closing_paren),
                    DocumentToken::EndOfList(_)
                ) {
                    break;
                }

                let focused = self.context.focused_node_indexes.contains(&closing_paren);
                let (attrs, search_match_attrs) = if focused {
                    (
                        color_scheme.parens.focused,
                        color_scheme.parens.focused_search_match,
                    )
                } else {
                    (color_scheme.parens.normal, color_scheme.parens.search_match)
                };

                segments.push(PreHighlightingStyledSegment {
                    attrs,
                    search_match_attrs,
                    content: Text::SourceRange(
                        self.context.core.node(closing_paren).data_range.clone(),
                    ),
                });

                closing_paren = closing_paren + 1;
            }
        }

        segments
    }
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
        dump_segments(
            render_line(&render_context, &logical_lines[line]),
            doc.raw_bytes_for_searching(),
            &style_map(),
        )
    }

    #[test]
    fn test_basic_render() {
        let doc = new_doc(b"((num_field 123)(bool_field true))");
        assert_snapshot!(dump_with_byte_indexes(&doc), @r"
        0..=4  :   0..=16  : ((num_field 123)
        5..=9  :  17..=35  :  (bool_field true))
        ");

        assert_snapshot!(render_doc_line(&doc, 0, NodeIndex(1)), @r"
        text: ((num_field 123)
              012........34..5
        0: parens                    : range(0..1)
        1: parens (focused)          : range(1..2)
        2: record_key (focused)      : range(2..11)
        3: whitespace                : range(11..12)
        4: number                    : range(12..15)
        5: parens (focused)          : range(15..16)
        ");

        assert_snapshot!(render_doc_line(&doc, 1, NodeIndex(0)), @r"
        text:  (bool_field true))
              012.........34...56
        0: whitespace                : spaces
        1: parens                    : range(17..18)
        2: record_key                : range(18..28)
        3: whitespace                : range(28..29)
        4: bool                      : range(29..33)
        5: parens                    : range(33..34)
        6: parens (focused)          : range(34..35)
        ");
    }
}
