use std::cmp::Ordering;
use std::collections::BTreeMap;
use std::num::NonZeroUsize;
use std::rc::Rc;

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::rendering::{PreHighlightingStyledSegment, Text, TokenColorScheme};
use crate::sexp::color_scheme::ColorScheme;
use crate::sexp::core::{
    invariants, AtomKind, DocCore, DocumentToken, EndOfListMetadata, ListKind, NodeIndex,
};
use crate::sexp::document::CollapseState;
use crate::sexp::layout::LogicalLine;

#[derive(Clone, Debug)]
pub struct ScreenLine {
    pub logical_line: LogicalLine,
    pub doc_width: NonZeroUsize,
    pub segments_per_screen_line: Rc<Vec<Vec<Segment>>>,
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

#[derive(Clone, Debug)]
pub struct Segment {
    pub node_index: Option<NodeIndex>,
    pub content: Text,
    kind: SegmentKind,
    terminal_width: usize,
}

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

struct WrappedSegmentsBuilder<'a> {
    pretty_printed: &'a [u8],
    doc_width: NonZeroUsize,
    wrapped_segments: Vec<Vec<Segment>>,
    remaining_width_on_current_line: usize,
}

impl<'a> WrappedSegmentsBuilder<'a> {
    fn new(pretty_printed: &'a [u8], doc_width: NonZeroUsize) -> Self {
        WrappedSegmentsBuilder {
            pretty_printed,
            wrapped_segments: vec![vec![]],
            doc_width,
            remaining_width_on_current_line: doc_width.get(),
        }
    }

    fn add_entire_segment_to_current_line(&mut self, segment: Segment) {
        debug_assert!(self.remaining_width_on_current_line >= segment.terminal_width);

        self.remaining_width_on_current_line -= segment.terminal_width;
        self.wrapped_segments.last_mut().unwrap().push(segment);
    }

    fn start_new_line(&mut self) {
        self.wrapped_segments.push(vec![]);
        self.remaining_width_on_current_line = self.doc_width.get();
    }

    fn maybe_start_new_line(&mut self) {
        if self.remaining_width_on_current_line == 0 {
            self.start_new_line();
        }
    }

    fn take_prefix_that_fits_in_available_space(
        s: &str,
        mut available_space: usize,
    ) -> (&str, usize) {
        let mut used_bytes = 0;
        let mut used_space = 0;

        for grapheme in s.graphemes(true) {
            let grapheme_width = UnicodeWidthStr::width(grapheme);
            if used_space + grapheme_width > available_space {
                break;
            }
            used_bytes += grapheme.len();
            used_space += grapheme_width;
        }

        (&s[0..used_bytes], used_space)
    }

    fn push_content(&mut self, node_index: Option<NodeIndex>, content: Text, kind: SegmentKind) {
        // Handle spaces separately, since it's simpler.
        if let Text::Spaces(n) = content {
            let mut content_remaining = n;

            while content_remaining > 0 {
                self.maybe_start_new_line();

                if content_remaining > self.remaining_width_on_current_line {
                    content_remaining -= self.remaining_width_on_current_line;

                    self.add_entire_segment_to_current_line(Segment {
                        node_index,
                        content: Text::Spaces(self.remaining_width_on_current_line),
                        kind,
                        terminal_width: self.remaining_width_on_current_line,
                    });
                } else {
                    self.add_entire_segment_to_current_line(Segment {
                        node_index,
                        content: Text::Spaces(content_remaining),
                        kind,
                        terminal_width: content_remaining,
                    });

                    break;
                }
            }

            return;
        };

        let mut processed_bytes = 0;

        while processed_bytes < content.len() {
            let remaining_s = match &content {
                Text::Spaces(_) => unreachable!(),
                Text::SourceRange(range) => {
                    str::from_utf8(&self.pretty_printed[(range.start + processed_bytes)..range.end])
                        .expect("pretty_printed should be valid utf8")
                }
                Text::String((s, range)) => &s[(range.start + processed_bytes)..range.end],
                Text::Static(s) => &s[processed_bytes..],
            };

            self.maybe_start_new_line();

            let (portion, used_width) = Self::take_prefix_that_fits_in_available_space(
                remaining_s,
                self.remaining_width_on_current_line,
            );

            if used_width == 0 {
                if self.remaining_width_on_current_line < self.doc_width.get() {
                    // Maybe if we start a new line we'll be able to fit in the next character.
                    self.start_new_line();
                    continue;
                } else {
                    // There's no way we'll be able to fit the next character in, so we'll
                    // put an ellipsis in instead.
                    self.add_entire_segment_to_current_line(Segment {
                        node_index,
                        content: Text::Static("…"),
                        kind,
                        terminal_width: 1,
                    });

                    let next_grapheme_len = remaining_s.graphemes(true).next().unwrap().len();
                    processed_bytes += next_grapheme_len;

                    continue;
                }
            }

            let content_portion = match &content {
                Text::Spaces(_) => unreachable!(),
                Text::SourceRange(range) => {
                    let portion_range_start = range.start + processed_bytes;
                    let portion_range_end = portion_range_start + portion.len();
                    Text::SourceRange(portion_range_start..portion_range_end)
                }
                Text::String((s, range)) => {
                    let portion_range_start = range.start + processed_bytes;
                    let portion_range_end = portion_range_start + portion.len();
                    Text::String((s.clone(), portion_range_start..portion_range_end))
                }
                Text::Static(s) => {
                    let portion_range_start = processed_bytes;
                    let portion_range_end = portion_range_start + portion.len();
                    Text::Static(&s[portion_range_start..portion_range_end])
                }
            };

            self.add_entire_segment_to_current_line(Segment {
                node_index,
                content: content_portion,
                kind,
                terminal_width: used_width,
            });

            processed_bytes += portion.len();
        }
    }
}

pub fn convert_logical_line_to_screen_line(
    logical_line: &LogicalLine,
    doc_width: NonZeroUsize,
    core: &DocCore,
    collapsible_nodes: &BTreeMap<NodeIndex, CollapseState>,
) -> ScreenLine {
    let mut wrapped_segments_builder =
        WrappedSegmentsBuilder::new(core.pretty_printed.data(), doc_width);

    if logical_line.indentation > 0 {
        wrapped_segments_builder.push_content(
            None,
            Text::Spaces(logical_line.indentation),
            SegmentKind::Whitespace,
        );
    }

    let mut prev_node_end_index = None;
    let mut paren_after_collapsed_section = None;

    for node_index in logical_line.node_indexes() {
        let node = core.node(node_index);

        if let Some(end_index) = prev_node_end_index {
            let whitespace_range = end_index..(node.data_range.start);
            if whitespace_range.len() > 0 {
                wrapped_segments_builder.push_content(
                    None,
                    Text::SourceRange(whitespace_range),
                    SegmentKind::Whitespace,
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

        wrapped_segments_builder.push_content(Some(node_index), content, segment_kind);

        if remainder_is_collapsed {
            break;
        }
    }

    // If we had a collapsed section, we'll loop over all the coalesced closing parens.
    if let Some(mut closing_paren) = paren_after_collapsed_section {
        wrapped_segments_builder.push_content(None, Text::Static("…"), SegmentKind::Comment);

        while closing_paren <= core.last_node_index_of_part_of_completed_sexp.unwrap() {
            if !matches!(core.token(closing_paren), DocumentToken::EndOfList(_)) {
                break;
            }

            wrapped_segments_builder.push_content(
                Some(closing_paren),
                Text::SourceRange(core.node(closing_paren).data_range.clone()),
                SegmentKind::Parens,
            );

            closing_paren = closing_paren + 1;
        }
    }

    ScreenLine {
        logical_line: logical_line.clone(),
        doc_width,
        segments_per_screen_line: Rc::new(wrapped_segments_builder.wrapped_segments),
        index: 0,
    }
}

pub struct RenderContext<'a> {
    color_scheme: &'a ColorScheme,
    core: &'a DocCore,
    collapsible_nodes: &'a BTreeMap<NodeIndex, CollapseState>,
    focused_node_indexes: Vec<NodeIndex>,
    doc_width: NonZeroUsize,
}

impl<'a> RenderContext<'a> {
    pub fn new(
        color_scheme: &'a ColorScheme,
        core: &'a DocCore,
        collapsible_nodes: &'a BTreeMap<NodeIndex, CollapseState>,
        focus: NodeIndex,
        doc_width: usize,
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
            core,
            collapsible_nodes,
            focused_node_indexes,
            doc_width: NonZeroUsize::new(doc_width).expect("doc width to be non-zero"),
        }
    }
}

// 'l for lifetime of the line renderer, 's for the lifetime of rendering the whole screen
struct ScreenLineRenderer<'l, 's> {
    context: &'l RenderContext<'s>,
    screen_line: &'l ScreenLine,
}

pub fn render_line<'l, 's>(
    context: &'l RenderContext<'s>,
    screen_line: &'l ScreenLine,
) -> Vec<PreHighlightingStyledSegment> {
    let mut line_renderer = ScreenLineRenderer {
        context,
        screen_line,
    };

    line_renderer.render()
}

impl<'l, 's> ScreenLineRenderer<'l, 's> {
    fn render(&mut self) -> Vec<PreHighlightingStyledSegment> {
        self.screen_line.segments_per_screen_line[self.screen_line.index]
            .iter()
            .map(|segment| {
                let focused = if let Some(node_index) = segment.node_index {
                    self.context.focused_node_indexes.contains(&node_index)
                } else {
                    false
                };

                let token_color_scheme = segment.kind.color_scheme(&self.context.color_scheme);

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
        let mut screen_line =
            doc.create_first_screen_line_from_logical_line(logical_lines[line].clone());

        let mut s = String::new();
        for index in 0..(screen_line.segments_per_screen_line.len()) {
            if index > 0 {
                s.push_str("\n\n");
            }

            screen_line.index = index;

            s.push_str(
                dump_segments(
                    render_line(&render_context, &screen_line),
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
