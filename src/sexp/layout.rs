use std::iter::DoubleEndedIterator;

use crate::sexp::core::{AtomKind, DocCore, DocumentToken, ListKind, ListMetadata, NodeIndex};

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct LogicalLine {
    pub start_index: NodeIndex,
    pub end_index: NodeIndex,
    pub indentation: usize,
}

impl LogicalLine {
    pub fn node_indexes(&self) -> impl DoubleEndedIterator<Item = NodeIndex> {
        ((self.start_index.0)..=(self.end_index.0)).map(NodeIndex)
    }

    pub fn contains_node_index(&self, node_index: NodeIndex) -> bool {
        self.start_index <= node_index && node_index <= self.end_index
    }
}

pub fn layout_fully_expanded_node(doc: &DocCore, node_index: NodeIndex) -> Vec<LogicalLine> {
    let mut layout_engine = LayoutEngine::new(doc, node_index);
    layout_engine.run();
    layout_engine.lines
}

struct LayoutEngine<'a> {
    doc: &'a DocCore,
    lines: Vec<LogicalLine>,
    list_contents_indentation: Vec<usize>,
    next_index: NodeIndex,
    end_index_incl: NodeIndex,
    layout_state: LayoutState,
    current_line_indentation: usize,
    current_line_start_index: NodeIndex,
}

#[derive(Copy, Clone)]
enum LayoutState {
    StartOfLine { leading_parens: usize },
    RecordFieldKey,
    RecordFieldValue,
    VariantConstructor { as_record_value: bool },
}

impl Default for LayoutState {
    fn default() -> Self {
        LayoutState::StartOfLine { leading_parens: 0 }
    }
}

/// Some token types are semantically treated the same when layout out a sexp document.
/// For example, unit ("()") and datetimes are treated just like normal atoms.
enum SemanticTokenKind {
    /// Actual atoms, unit, and datetimes.
    AtomLike,
    RecordField,
    Variant,
    Singleton,
    List,
    EndOfList,
    /// Line comments and block comments are treated identically.
    Comment,
    Error,
}

impl SemanticTokenKind {
    fn from_document_token(token: &DocumentToken) -> Self {
        match token {
            DocumentToken::Atom(_) | DocumentToken::Unit { .. } => SemanticTokenKind::AtomLike,
            DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => match list_kind {
                ListKind::DateTime => SemanticTokenKind::AtomLike,
                ListKind::RecordField => SemanticTokenKind::RecordField,
                ListKind::VariantRecord | ListKind::VariantTuple => SemanticTokenKind::Variant,
                ListKind::Singleton => SemanticTokenKind::Singleton,
                ListKind::Record | ListKind::Unit | ListKind::Plain => SemanticTokenKind::List,
            },
            DocumentToken::EndOfList(_) => SemanticTokenKind::EndOfList,
            DocumentToken::LineComment | DocumentToken::BlockComment => SemanticTokenKind::Comment,
            DocumentToken::Error(_) => SemanticTokenKind::Error,
        }
    }
}

impl<'a> LayoutEngine<'a> {
    fn new(doc: &'a DocCore, node_index: NodeIndex) -> LayoutEngine<'a> {
        let end_index_incl = match doc.token(node_index) {
            DocumentToken::StartOfList(list_metadata) => list_metadata.end_index().unwrap(),
            _ => node_index,
        };

        LayoutEngine {
            doc,
            lines: vec![],
            list_contents_indentation: vec![],
            next_index: node_index,
            end_index_incl,
            layout_state: LayoutState::StartOfLine { leading_parens: 0 },
            current_line_indentation: 0,
            current_line_start_index: node_index,
        }
    }

    fn next_token(&self) -> &DocumentToken {
        self.doc.token(self.next_index)
    }

    fn put_next_token_on_current_line(&mut self) {
        self.next_index = self.next_index + 1;
    }

    fn put_comment_on_current_line(&mut self) {
        debug_assert!(matches!(
            self.next_token(),
            DocumentToken::LineComment | DocumentToken::BlockComment,
        ));
        self.put_next_token_on_current_line();
    }

    fn put_opening_paren_on_current_line(&mut self, elem_indentation: usize) {
        debug_assert!(matches!(self.next_token(), DocumentToken::StartOfList(_)));
        self.put_next_token_on_current_line();
        self.list_contents_indentation.push(elem_indentation);
    }

    fn put_atom_like_on_current_line(&mut self) {
        match self.next_token() {
            DocumentToken::Atom(_) | DocumentToken::Unit { .. } => {
                self.put_next_token_on_current_line();
            }
            DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => {
                debug_assert!(matches!(list_kind, ListKind::DateTime));
                self.put_next_token_on_current_line(); // StartOfList

                debug_assert!(matches!(
                    self.next_token().atom_kind(),
                    Some(AtomKind::Date)
                ));
                self.put_next_token_on_current_line();

                debug_assert!(matches!(
                    self.next_token().atom_kind(),
                    Some(AtomKind::Time)
                ));
                self.put_next_token_on_current_line();

                debug_assert!(matches!(self.next_token(), DocumentToken::EndOfList(_)));
                self.put_next_token_on_current_line();
            }
            _ => {
                panic!("self.next_index is not atom-like");
            }
        }
    }

    fn elem_indentation_for_current_list(&self) -> usize {
        self.list_contents_indentation
            .last()
            .map(|x| *x)
            .unwrap_or(0)
    }

    fn increase_current_indendation(&mut self, by: usize) {
        *self.list_contents_indentation.last_mut().unwrap() += by;
    }

    fn end_line_with_indentation_and_set_state(
        &mut self,
        indentation: usize,
        new_layout_state: LayoutState,
    ) {
        debug_assert!(
            self.current_line_start_index < self.next_index,
            "ended line with no tokens"
        );

        let end_index = self.next_index - 1;
        self.lines.push(LogicalLine {
            indentation,
            start_index: self.current_line_start_index,
            end_index,
        });

        self.layout_state = new_layout_state;
        self.current_line_start_index = self.next_index;
        self.current_line_indentation = self.elem_indentation_for_current_list();
    }

    fn end_line_and_set_state(&mut self, new_layout_state: LayoutState) {
        self.end_line_with_indentation_and_set_state(
            self.current_line_indentation,
            new_layout_state,
        );
    }

    fn end_line_and_reset_to_default_state(&mut self) {
        self.end_line_and_set_state(LayoutState::default());
    }

    fn consume_closing_parens(&mut self) {
        while self.next_index <= self.end_index_incl {
            if !matches!(self.next_token(), DocumentToken::EndOfList(_)) {
                break;
            }
            self.put_next_token_on_current_line();
            let popped_indentation = self.list_contents_indentation.pop();
            debug_assert!(popped_indentation.is_some());
        }
    }

    fn run(&mut self) {
        use SemanticTokenKind as STK;

        // TODO: COMMENTED OUT THINGS???

        while self.next_index <= self.end_index_incl {
            let next_token_kind = STK::from_document_token(self.next_token());

            match self.layout_state {
                LayoutState::StartOfLine { leading_parens } => {
                    match next_token_kind {
                        STK::AtomLike => {
                            self.put_atom_like_on_current_line();
                            self.consume_closing_parens();
                            self.end_line_and_reset_to_default_state();
                        }
                        STK::Comment => {
                            self.put_comment_on_current_line();
                            self.end_line_and_reset_to_default_state();
                        }
                        STK::Error => {
                            // We always want errors on their own line, so we'll only put the error
                            // on this line if haven't placed any parens yet.
                            if leading_parens == 0 {
                                self.put_next_token_on_current_line();
                            }
                            self.end_line_and_reset_to_default_state();
                        }
                        STK::RecordField => {
                            // This is okay:   But not this:   We want this:
                            // (((a 1)         ((((a 1)        ((
                            //   (b 2))           (b 2)          ((a 1)
                            //  ((c 3)           ((c 3)           (b 2))
                            //   (d 4)))          (d 4))))       ((c 3)
                            //                                    (d 4))))
                            if leading_parens > 2 {
                                self.end_line_and_reset_to_default_state();
                            } else {
                                self.put_opening_paren_on_current_line(
                                    self.current_line_indentation + leading_parens + 1,
                                );
                                self.layout_state = LayoutState::RecordFieldKey;
                            }
                        }
                        STK::Variant => {
                            if leading_parens > 2 {
                                self.end_line_and_reset_to_default_state();
                            } else {
                                self.put_opening_paren_on_current_line(
                                    self.current_line_indentation + leading_parens + 1,
                                );
                                self.layout_state = LayoutState::VariantConstructor {
                                    as_record_value: false,
                                };
                            }
                        }
                        STK::List => {
                            if leading_parens >= 2 {
                                self.end_line_and_reset_to_default_state();
                            } else {
                                self.put_opening_paren_on_current_line(
                                    self.current_line_indentation + leading_parens + 1,
                                );
                                self.layout_state = LayoutState::StartOfLine {
                                    leading_parens: leading_parens + 1,
                                };
                            }
                        }
                        STK::Singleton => {
                            self.put_opening_paren_on_current_line(
                                self.current_line_indentation + usize::min(leading_parens + 1, 2),
                            );
                            self.layout_state = LayoutState::StartOfLine {
                                leading_parens: leading_parens + 1,
                            };
                        }
                        STK::EndOfList => {
                            self.consume_closing_parens();
                            self.end_line_with_indentation_and_set_state(
                                self.elem_indentation_for_current_list(),
                                LayoutState::default(),
                            );
                        }
                    }
                }
                LayoutState::RecordFieldKey => {
                    match next_token_kind {
                        STK::AtomLike => {
                            self.put_atom_like_on_current_line();
                            self.layout_state = LayoutState::RecordFieldValue;
                        }
                        STK::Comment => {
                            self.put_comment_on_current_line();
                            // We stay in the same state.
                            self.end_line_and_set_state(LayoutState::RecordFieldKey);
                        }
                        STK::Error => {
                            // Errors always go on their own line.
                            if self.current_line_start_index != self.next_index {
                                self.end_line_and_set_state(LayoutState::RecordFieldKey);
                            }

                            self.put_next_token_on_current_line();

                            // We stay in the same state.
                            self.end_line_and_set_state(LayoutState::RecordFieldKey);
                        }
                        STK::RecordField
                        | STK::Variant
                        | STK::Singleton
                        | STK::List
                        | STK::EndOfList => {
                            panic!("Unexpected next token while formatting VariantConstructor")
                        }
                    }
                }
                LayoutState::RecordFieldValue => {
                    match next_token_kind {
                        STK::AtomLike => {
                            self.put_atom_like_on_current_line();
                            self.consume_closing_parens();
                            self.end_line_and_reset_to_default_state();
                        }
                        STK::Variant => {
                            // Under normal circumstances,
                            // ((a (Variant
                            //    varant-param-1
                            //    varant-param-2)))
                            self.put_opening_paren_on_current_line(
                                // We'll update the indentation after we see the Constructor.
                                self.elem_indentation_for_current_list(),
                            );
                            self.layout_state = LayoutState::VariantConstructor {
                                as_record_value: true,
                            };
                        }
                        STK::Singleton => {
                            // We'll nest a bunch of singletons together when they're the value
                            // of a key-value pair, so we just process the token, and pretend
                            // nothing happened.
                            self.put_opening_paren_on_current_line(
                                self.elem_indentation_for_current_list(),
                            );
                        }
                        // Treat RecordField like a normal list
                        STK::RecordField | STK::List => {
                            self.put_opening_paren_on_current_line(
                                self.elem_indentation_for_current_list() + 1,
                            );
                            self.end_line_and_reset_to_default_state();
                        }
                        STK::Comment | STK::Error => {
                            // Increase indentation by 2 (rather than 1) so it doesn't look like a
                            // normal nested value under the key; it's at the same level as the key.
                            //
                            // ((foo
                            //     ; comment
                            //     value)
                            //  (bar (
                            //    1
                            //    2)))
                            self.increase_current_indendation(2);
                            self.end_line_and_reset_to_default_state();
                        }
                        STK::EndOfList => {
                            panic!("Unexpected next token while formatting RecordFieldValue")
                        }
                    }
                }
                LayoutState::VariantConstructor { as_record_value } => match next_token_kind {
                    STK::Error => {
                        self.end_line_and_set_state(LayoutState::VariantConstructor {
                            as_record_value: false,
                        });
                    }
                    STK::Comment => {
                        if as_record_value {
                            self.increase_current_indendation(2);
                        } else {
                            self.put_comment_on_current_line();
                        }
                        self.end_line_and_set_state(LayoutState::VariantConstructor {
                            as_record_value: false,
                        });
                    }
                    STK::AtomLike => {
                        self.put_atom_like_on_current_line();
                        self.increase_current_indendation(1);
                        self.end_line_and_reset_to_default_state();
                    }
                    STK::RecordField
                    | STK::Variant
                    | STK::Singleton
                    | STK::List
                    | STK::EndOfList => {
                        panic!("Unexpected next token while formatting VariantConstructor")
                    }
                },
            }
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    use std::fmt::Write;

    use crate::sexp::core::{DocCore, NodeIndex};

    use bstr::ByteSlice;
    use insta::assert_snapshot;

    pub fn show_logical_lines(doc: &DocCore, lines: Vec<LogicalLine>) -> String {
        let mut output = String::new();

        for LogicalLine {
            indentation,
            start_index,
            end_index,
        } in lines.into_iter()
        {
            let start_range = doc.node(start_index).data_range.clone();
            let end_range = doc.node(end_index).data_range.clone();

            let _ = write!(output, "{:>2}..={:<2} : ", start_index.0, end_index.0,);

            let _ = write!(output, "{: <indentation$}", "");

            let _ = match (start_range, end_range) {
                (Some(start), Some(end)) => {
                    writeln!(
                        output,
                        "{}",
                        doc.pretty_printed[start.start..end.end].as_bstr()
                    )
                }
                _ => writeln!(output, "<no range>"),
            };
        }

        output
    }

    fn layout(bytes: &'static [u8]) -> String {
        let doc = DocCore::from_bytes(bytes);
        let logical_lines = layout_fully_expanded_node(&doc, NodeIndex(0));
        show_logical_lines(&doc, logical_lines)
    }

    #[test]
    fn layout_top_level_atoms_and_comments() {
        let output = layout(b"hello");
        assert_snapshot!(&output, @" 0..=0  : hello");

        let output = layout(b"; comment");
        assert_snapshot!(&output, @" 0..=0  : ; comment");

        let output = layout(b"#| 1 2 3 |#");
        assert_snapshot!(&output, @" 0..=0  : #| 1 2 3 |#");
    }

    #[test]
    fn layout_variants() {
        let output = layout(b"(Constructor 1 2 () (Nested a b c))");
        assert_snapshot!(&output, @r"
        0..=1  : (Constructor
        2..=2  :   1
        3..=3  :   2
        4..=4  :   ()
        5..=6  :   (Nested
        7..=7  :     a
        8..=8  :     b
        9..=11 :     c))
        ");

        let output = layout(
            br"(#| Why is there a comment here? |# Constructor 1 2 () (#| Nobody |# ; knows
               Nested a b c))",
        );
        assert_snapshot!(&output, @r"
         0..=1  : (#| Why is there a comment here? |#
         2..=2  :  Constructor
         3..=3  :   1
         4..=4  :   2
         5..=5  :   ()
         6..=7  :   (#| Nobody |#
         8..=8  :    ; knows
         9..=9  :    Nested
        10..=10 :     a
        11..=11 :     b
        12..=14 :     c))
        ");
    }

    #[test]
    fn layout_record_fields() {
        assert_snapshot!(layout(b"(key simple_value)"), @" 0..=3  : (key simple_value)");
        assert_snapshot!(layout(b"(key (singleton_value))"), @" 0..=5  : (key (singleton_value))");
        assert_snapshot!(layout(b"(key ((nested_singleton_value)))"), @" 0..=7  : (key ((nested_singleton_value)))");
        assert_snapshot!(layout(b"(key ((very_nested_singleton_value)))"), @" 0..=7  : (key ((very_nested_singleton_value)))");

        // DateTimes are treated like atoms
        assert_snapshot!(layout(b"(key (2025-01-13 11:28:37.000000000))"), @" 0..=6  : (key (2025-01-13 11:28:37.000000000))");

        assert_snapshot!(layout(b"(key (Variant value))"), @r"
        0..=3  : (key (Variant
        4..=6  :   value))
        ");
        assert_snapshot!(layout(b"(key ((Variant_in_singleton (a 1)(b 2))))"), @r"
        0..=4  : (key ((Variant_in_singleton
        5..=8  :   (a 1)
        9..=15 :   (b 2))))
        ");
        assert_snapshot!(layout(b"(; comment before key\nkey value)"), @r"
        0..=1  : (; comment before key
        2..=4  :  key value)
        ");
        assert_snapshot!(layout(b"(key ; comment before value\nvalue)"), @r"
        0..=1  : (key
        2..=2  :    ; comment before value
        3..=4  :    value)
        ");
        assert_snapshot!(layout(b"(key value ; comment after value\n)"), @r"
        0..=2  : (key value
        3..=3  :  ; comment after value
        4..=4  : )
        ");
        assert_snapshot!(layout(b"(key (Variant 1) ; comment after value\n)"), @r"
        0..=3  : (key (Variant
        4..=5  :   1)
        6..=6  :  ; comment after value
        7..=7  : )
        ");
        assert_snapshot!(layout(b"(key ; comment before variant value\n(Variant value))"), @r"
        0..=1  : (key
        2..=2  :    ; comment before variant value
        3..=4  :    (Variant
        5..=7  :      value))
        ");
        assert_snapshot!(layout(b"(key (; comment breaking up constructor\nConstructor (a 1) (b 2)))"), @r"
        0..=2  : (key (
        3..=3  :    ; comment breaking up constructor
        4..=4  :    Constructor
        5..=8  :     (a 1)
        9..=14 :     (b 2)))
        ");
        assert_snapshot!(layout(b"(key ; comment before\n(; and after paren\nConstructor (a 1) (b 2)))"), @r"
         0..=1  : (key
         2..=2  :    ; comment before
         3..=4  :    (; and after paren
         5..=5  :     Constructor
         6..=9  :      (a 1)
        10..=15 :      (b 2)))
        ");
    }

    #[test]
    fn layout_lists_and_records() {
        let output = layout(b"(1 2 3 4 5)");
        assert_snapshot!(&output, @r"
        0..=1  : (1
        2..=2  :  2
        3..=3  :  3
        4..=4  :  4
        5..=6  :  5)
        ");

        let output = layout(b"((apple 1)(banana 2)(cherry 3))");
        assert_snapshot!(&output, @r"
        0..=4  : ((apple 1)
        5..=8  :  (banana 2)
        9..=13 :  (cherry 3))
        ");

        let output = layout(b"(((apple 1)(banana 2)(cherry 3)))");
        assert_snapshot!(&output, @r"
         0..=5  : (((apple 1)
         6..=9  :   (banana 2)
        10..=15 :   (cherry 3)))
        ");

        let output = layout(b"((((apple 1)(banana 2)(cherry 3))))");
        assert_snapshot!(&output, @r"
         0..=1  : ((
         2..=6  :   ((apple 1)
         7..=10 :    (banana 2)
        11..=17 :    (cherry 3))))
        ");

        let output = layout(b"((apple (Aardvark 1 2))(banana 3))");
        assert_snapshot!(&output, @r"
        0..=4  : ((apple (Aardvark
        5..=5  :    1
        6..=8  :    2))
        9..=13 :  (banana 3))
        ");

        let output = layout(b"(((apple (Aardvark 1 2))(banana 3)))");
        assert_snapshot!(&output, @r"
         0..=5  : (((apple (Aardvark
         6..=6  :     1
         7..=9  :     2))
        10..=15 :   (banana 3)))
        ");

        let output = layout(b"(#; (apple 1)(banana 2)(cherry 3))");
        assert_snapshot!(&output, @r"
        0..=4  : (#; (apple 1)
        5..=8  :  (banana 2)
        9..=13 :  (cherry 3))
        ");
    }

    #[test]
    fn layout_singletons() {
        // Atom singletons
        let output = layout(b"(x)");
        assert_snapshot!(&output, @" 0..=2  : (x)");

        let output = layout(b"((x))");
        assert_snapshot!(&output, @" 0..=4  : ((x))");

        let output = layout(b"(((x)))");
        assert_snapshot!(&output, @" 0..=6  : (((x)))");

        let output = layout(b"((((x))))");
        assert_snapshot!(&output, @" 0..=8  : ((((x))))");

        let output = layout(b"(((((x)))))");
        assert_snapshot!(&output, @" 0..=10 : (((((x)))))");

        // Record singletons
        let output = layout(b"(((a 1)(b 2)))");
        assert_snapshot!(&output, @r"
        0..=5  : (((a 1)
        6..=11 :   (b 2)))
        ");

        let output = layout(b"((((a 1)(b 2))))");
        assert_snapshot!(&output, @r"
        0..=1  : ((
        2..=6  :   ((a 1)
        7..=13 :    (b 2))))
        ");

        let output = layout(b"(((((a 1)(b 2)))))");
        assert_snapshot!(&output, @r"
        0..=2  : (((
        3..=7  :   ((a 1)
        8..=15 :    (b 2)))))
        ");

        // Variant singletons
        let output = layout(b"((Variant 1 2))");
        assert_snapshot!(&output, @r"
        0..=2  : ((Variant
        3..=3  :    1
        4..=6  :    2))
        ");

        let output = layout(b"(((Variant 1 2)))");
        assert_snapshot!(&output, @r"
        0..=3  : (((Variant
        4..=4  :     1
        5..=8  :     2)))
        ");

        let output = layout(b"((((Variant 1 2))))");
        assert_snapshot!(&output, @r"
        0..=2  : (((
        3..=4  :   (Variant
        5..=5  :     1
        6..=10 :     2))))
        ");

        let output = layout(b"(((((Variant 1 2)))))");
        assert_snapshot!(&output, @r"
        0..=3  : ((((
        4..=5  :   (Variant
        6..=6  :     1
        7..=12 :     2)))))
        ");
    }

    #[test]
    fn layout_sexp_pp_config() {
        let sexp_pp_config = br"
            ((indent 2)
             (data_alignment (
               Data_aligned
               (parens_alignment    false)
               (atom_threshold      6)
               (character_threshold 50)
               (depth_threshold     3)))
             (color_scheme (Magenta Yellow Cyan White))
             (atom_coloring (Color_first 3))
             (atom_printing  Escaped)
             (paren_coloring true)
             (opening_parens Same_line)
             (closing_parens Same_line)
             (comments (Print (Indent_comment 3) (Green) Pretty_print))
             (singleton_limit (
               Singleton_limit
               (atom_threshold      3)
               (character_threshold 40)))
             (leading_threshold (
               (atom_threshold      3)
               (character_threshold 40)))
             (separator       Empty_line)
             (sticky_comments After))";

        let layout = layout(sexp_pp_config);
        assert_snapshot!(&layout, @r"
         0..=4  : ((indent 2)
         5..=8  :  (data_alignment (Data_aligned
         9..=12 :    (parens_alignment false)
        13..=16 :    (atom_threshold 6)
        17..=20 :    (character_threshold 50)
        21..=26 :    (depth_threshold 3)))
        27..=29 :  (color_scheme (
        30..=30 :    Magenta
        31..=31 :    Yellow
        32..=32 :    Cyan
        33..=35 :    White))
        36..=39 :  (atom_coloring (Color_first
        40..=42 :    3))
        43..=46 :  (atom_printing Escaped)
        47..=50 :  (paren_coloring true)
        51..=54 :  (opening_parens Same_line)
        55..=58 :  (closing_parens Same_line)
        59..=62 :  (comments (Print
        63..=64 :    (Indent_comment
        65..=66 :      3)
        67..=69 :    (Green)
        70..=72 :    Pretty_print))
        73..=76 :  (singleton_limit (Singleton_limit
        77..=80 :    (atom_threshold 3)
        81..=86 :    (character_threshold 40)))
        87..=89 :  (leading_threshold (
        90..=93 :    (atom_threshold 3)
        94..=99 :    (character_threshold 40)))
        100..=103 :  (separator Empty_line)
        104..=108 :  (sticky_comments After))
        ");
    }
}
