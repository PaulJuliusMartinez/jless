use std::ops::Range;

use regex::bytes::Regex;
#[cfg(test)]
use serde::Serialize;

use ocaml_sexplib::atom::Atom;
use ocaml_sexplib::input::InputRef;
use ocaml_sexplib::tokenizer::{RawBytes, RawToken, UnescapedBytes};

use crate::sexp::pretty::PrettyPrinted;

#[cfg_attr(test, derive(Serialize))]
#[derive(Copy, Clone, Debug)]
struct NodeIndex(usize);

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
struct SexpDocument {
    // Pretty-printed data
    pretty_printed: PrettyPrinted,
    data_len_of_completed_sexps: usize,

    // Structural data about the document
    all_nodes: Vec<DocumentNode>,
    last_top_level_node_index: Option<NodeIndex>,
    num_top_level_data_nodes: usize,
    error_indexes: Vec<NodeIndex>,

    // Parsing state
    starts_of_unterminated_lists: Vec<NodeIndex>,
    num_pending_sexp_comments: usize,
    scratch_buffer_for_unescaping_atoms: Vec<u8>,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
struct DocumentNode {
    parent_index: Option<NodeIndex>,
    prev_sibling: Option<NodeIndex>,
    next_sibling: Option<NodeIndex>,
    /// Only set for atoms and the start of lists. Indicates the index of the node
    /// in the parent (or amongst all top-level nodes) if comments are ignored.
    data_index_in_parent: Option<usize>,
    data_range: Option<Range<usize>>,
    token: DocumentToken,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
enum DocumentToken {
    StartOfList(ListMetadata),
    EndOfList(EndOfListMetadata),
    Atom(AtomMetadata),
    Unit { commented_out: bool },
    LineComment,
    BlockComment,
    Error(ErrorMetadata),
}

impl DocumentToken {
    fn is_data_node(&self) -> bool {
        match self {
            DocumentToken::StartOfList(_) | DocumentToken::Atom(_) | DocumentToken::Unit { .. } => {
                true
            }
            _ => false,
        }
    }

    fn is_commented_out(&self) -> bool {
        match self {
            DocumentToken::StartOfList(ListMetadata { commented_out, .. })
            | DocumentToken::Atom(AtomMetadata { commented_out, .. })
            | DocumentToken::Unit { commented_out } => *commented_out,
            _ => false,
        }
    }

    fn list_metadata(&self) -> &ListMetadata {
        match self {
            DocumentToken::StartOfList(list_metadata) => list_metadata,
            _ => panic!(
                "Expected StartOfList token when calling `list_metadata`, got {:?}",
                self,
            ),
        }
    }

    fn list_metadata_mut(&mut self) -> &mut ListMetadata {
        match self {
            DocumentToken::StartOfList(list_metadata) => list_metadata,
            _ => panic!(
                "Expected StartOfList token when calling `list_metadata_mut`, got {:?}",
                self,
            ),
        }
    }

    fn list_kind(&self) -> Option<ListKind> {
        match self {
            DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => Some(*list_kind),
            DocumentToken::Unit { .. } => Some(ListKind::Unit),
            _ => None,
        }
    }

    fn atom_kind(&self) -> Option<AtomKind> {
        match self {
            DocumentToken::Atom(AtomMetadata { atom_kind, .. }) => Some(*atom_kind),
            _ => None,
        }
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
struct ListMetadata {
    list_kind: ListKind,
    // first_child_index is just our own index + 1.
    last_child_index: Option<NodeIndex>,
    list_end_index: Option<NodeIndex>,
    commented_out: bool,
    data_length: usize,
    contains_non_data: bool,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
struct AtomMetadata {
    atom_kind: AtomKind,
    commented_out: bool,
    quoted: bool,
    valid: bool,
    // printable_ascii: bool,
    // has_escapes: bool,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
struct EndOfListMetadata {
    list_start_index: NodeIndex,
    // We display closing parens on their own line if:
    // 1) the last element of the list is a line comment (so we're forced to), or
    // 2) the last element of the list is an error
    should_display_on_own_line: bool,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
struct ErrorMetadata {
    message: String,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Copy, Clone, Debug)]
enum AtomKind {
    /// An upper snake case value
    Constructor,
    /// A lower snake case value
    RecordKey,
    /// A number, possibly with underscores, or in scientific notation
    Number,
    /// "true" or "false"
    Bool,
    /// YYYY-MM-DD
    Date,
    /// HH:MM:SS...
    Time,
    /// A stringified non-empty list, e.g. "((a 1) (b 2))"
    StringifiedList,
    /// Anything else
    Plain,
}

const ATOM_DATE_LENGTH: usize = 10;

lazy_static::lazy_static! {
    static ref ATOM_CONSTRUCTOR_RE: Regex = Regex::new("^[A-Z][_a-zA-Z0-9']*$").unwrap();
    static ref ATOM_RECORD_KEY_RE: Regex = Regex::new("^[a-z][_a-z0-9']*$").unwrap();
    static ref ATOM_BOOL_RE: Regex = Regex::new("^(true|false)$").unwrap();

    static ref ATOM_DATE_RE: Regex = Regex::new(r"(?x)
      ^
      [0-9]{4}        # Year
      -
      (0[1-9]|1[0-2]) # Month, only allowing 01-12
      -
      [0-3][0-9]      # Day, only allowing 00-39
      $
    ").unwrap();

    static ref ATOM_TIME_RE: Regex = Regex::new(r"(?x)
      ^
      [0-9]{2} # Hour
      :
      [0-9]{2} # Minute
      :
      [0-9]{2} # Second
      \.       # Decimal
      [0-9]{9} # ns
      # Optional timezone
      ( Z                      # UTC
      | (\+|-)[0-9]{2}:[0-9]{2} # Offset
      )?
      $
    ").unwrap();

    // https://ocaml.org/manual/5.4/lex.html#sss:integer-literals
    static ref ATOM_INTEGER_RE: Regex = Regex::new(r"(?x)
      ^
      -?                              # Optional negative sign, then
      ( [0-9][_0-9]*                  # Regular integer, or
      | 0[xX][0-9A-Fa-f][0-9A-Fa-f_]* # Hex integer, or
      | 0[oO][0-7][0-7_]*             # Octal integer, or
      | 0[bB][0-1][0-1_]*             # Binary integer
      )
      $
    ").unwrap();

    // https://ocaml.org/manual/5.4/lex.html#sss:floating-point-literals
    static ref ATOM_FLOAT_RE: Regex = Regex::new(r"(?x)
      ^
      -?        # Optional negative sign, then
      ( # Regular floating point
        [0-9][0-9_]*              # Leading digits
        (.[0-9_]*)?               # Optional decimal
        ([eE](\+|-)?[0-9][0-9_]*)? # Optional exponent
      | # Hex floating point
        0[xX]                     # Leading 0x
        [0-9A-Fa-f][0-9A-Fa-f_]*  # Leading digits
        (.[0-9A-Fa-f_]*)?         # Optional decimal
        ([pP](\+|-)?[0-9][0-9_]*)  # Optional exponent (in decimal)
      )
      $
    ").unwrap();
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Copy, Clone, Debug)]
enum ListKind {
    /// A list of length where every element is a list of kind `RecordField`.
    Record,
    /// A list of length two, where the first element is an atom with kind `Record_key`.
    RecordField,
    /// A list of length at least two, where first element is an atom with kind `Constructor`,
    /// and every subsequent element is a list of kind `RecordField`.
    VariantRecord,
    /// A list of length at least two, where first element is an atom with kind `Constructor`.
    VariantTuple,
    /// A list of length two where the first element is an atom of kind `Date` and the
    /// second is an atom of kind `Time`.
    DateTime,
    /// A list of exactly length 1, with no comments.
    Singleton,
    /// Unit; a list with data length 0
    Unit,
    /// Anything else.
    Plain,
}

impl SexpDocument {
    pub fn new() -> SexpDocument {
        SexpDocument {
            pretty_printed: PrettyPrinted::new(),
            data_len_of_completed_sexps: 0,
            all_nodes: vec![],
            last_top_level_node_index: None,
            num_top_level_data_nodes: 0,
            error_indexes: vec![],
            starts_of_unterminated_lists: vec![],
            num_pending_sexp_comments: 0,
            scratch_buffer_for_unescaping_atoms: vec![],
        }
    }
}

// Implementation of parsing logic
impl SexpDocument {
    // Creates a new `DocumentNode` for the given token, and updates all the bookkeeping
    // appropriately:
    // - On parent node (or document itself if top-level node):
    //   - last child index
    //   - # of data nodes
    // - On previous sibling:
    //   - next sibling
    // - On new node
    //   - parent index
    //   - data index in parent
    //   - prev sibling
    //
    // This should _not_ be called when adding a new `EndOfList` token.
    fn push_new_document_node(
        &mut self,
        token: DocumentToken,
        data_range: Option<Range<usize>>,
    ) -> NodeIndex {
        assert!(!matches!(token, DocumentToken::EndOfList(_)));

        let new_node_index = NodeIndex(self.all_nodes.len());

        let parent_index;
        let prev_sibling;
        let data_index_in_parent;

        match self.starts_of_unterminated_lists.last() {
            None => {
                // We're adding a top-level node with no parent; we'll update fields
                // in `self` directly.
                parent_index = None;

                prev_sibling = self.last_top_level_node_index;
                self.last_top_level_node_index = Some(new_node_index);

                data_index_in_parent = if token.is_data_node() && !token.is_commented_out() {
                    let index = self.num_top_level_data_nodes;
                    self.num_top_level_data_nodes += 1;
                    Some(index)
                } else {
                    None
                };
            }
            Some(list_start_index) => {
                // We're adding a node inside a list; update the list metadata.
                let parent_metadata = self.all_nodes[list_start_index.0].token.list_metadata_mut();

                parent_index = Some(*list_start_index);

                prev_sibling = parent_metadata.last_child_index;
                parent_metadata.last_child_index = Some(new_node_index);

                data_index_in_parent = if token.is_data_node() && !token.is_commented_out() {
                    let index = parent_metadata.data_length;
                    parent_metadata.data_length += 1;
                    Some(index)
                } else {
                    parent_metadata.contains_non_data = true;
                    None
                };
            }
        }

        if let Some(sibling_index) = prev_sibling {
            self.all_nodes[sibling_index.0].next_sibling = Some(new_node_index);
        }

        if matches!(token, DocumentToken::Error(_)) {
            self.error_indexes.push(new_node_index);
        }

        let document_node = DocumentNode {
            parent_index,
            prev_sibling,
            next_sibling: None,
            data_index_in_parent,
            data_range,
            token,
        };

        self.all_nodes.push(document_node);

        new_node_index
    }

    fn push_new_error_node(&mut self, error_metadata: ErrorMetadata) {
        let _error_node_index =
            self.push_new_document_node(DocumentToken::Error(error_metadata), None);
    }

    pub fn append_raw_token(&mut self, raw_token: RawToken<'_, '_>) {
        match raw_token {
            RawToken::LeftParen => self.start_new_list(),
            RawToken::RightParen => self.complete_list(),
            RawToken::Atom(atom_bytes) => self.add_atom(atom_bytes),
            RawToken::LineComment(line_comment) => {
                let data_range = self
                    .pretty_printed
                    .write_line_comment(line_comment.raw_bytes());
                let _ = self.push_new_document_node(DocumentToken::LineComment, Some(data_range));
            }
            RawToken::BlockComment(block_comment) => {
                // TODO: I need to validate these block comment bytes.
                let data_range = self
                    .pretty_printed
                    .write_block_comment(block_comment.raw_bytes());
                let _ = self.push_new_document_node(DocumentToken::BlockComment, Some(data_range));
            }
            RawToken::SexpComment => self.add_sexp_comment(),
        }

        if self.starts_of_unterminated_lists.is_empty() && self.num_pending_sexp_comments == 0 {
            self.pretty_printed.complete_top_level_node();
            self.data_len_of_completed_sexps = self.pretty_printed.len();
        }
    }

    pub fn append_eof(&mut self) {
        // We just have to check for errors here, pending sexp comments and unterminated lists.
        if self.num_pending_sexp_comments > 0 {
            self.num_pending_sexp_comments = 0;

            self.push_new_error_node(ErrorMetadata {
                message: "Unexpected EOF after sexp comment \"#;\"".to_string(),
            });
        }

        if self.starts_of_unterminated_lists.is_empty() {
            return;
        }

        self.push_new_error_node(ErrorMetadata {
            message: "Unexpected EOF while parsing list".to_string(),
        });

        // We'll put the first closing ')' on its own line, and then the rest the same line.
        let mut is_first_unterminated_list = true;

        while let Some(list_start_index) = self.starts_of_unterminated_lists.pop() {
            let should_display_on_own_line = is_first_unterminated_list;
            is_first_unterminated_list = false;

            // We won't actually add the trailing ')' to the internal doc.
            let should_append_to_pretty_printed = false;

            self.complete_single_list(
                list_start_index,
                should_display_on_own_line,
                should_append_to_pretty_printed,
            );
        }
    }

    fn start_new_list(&mut self) {
        let list_metadata = ListMetadata {
            list_kind: ListKind::Plain,
            last_child_index: None,
            list_end_index: None,
            commented_out: self.consume_pending_sexp_comment(),
            data_length: 0,
            contains_non_data: false,
        };

        let data_range = self.pretty_printed.start_list();
        let new_node_index = self
            .push_new_document_node(DocumentToken::StartOfList(list_metadata), Some(data_range));

        self.starts_of_unterminated_lists.push(new_node_index);
    }

    fn complete_list(&mut self) {
        let Some(list_start_index) = self.starts_of_unterminated_lists.pop() else {
            // Saw a ')' while not in a list!
            self.push_new_error_node(ErrorMetadata {
                message: "Saw unexpected ')' while parsing top-level sexp".to_string(),
            });

            // Don't add the ')' to `pretty_printed`; it's invalid
            return;
        };

        if self.num_pending_sexp_comments > 0 {
            // We didn't see a another list or an atom after a sexp-comment. We'll
            // clear it back to 0 to prevent further problems.
            self.num_pending_sexp_comments = 0;

            self.push_new_error_node(ErrorMetadata {
                message: "Saw unexpected ')' after sexp comment \"#;\"".to_string(),
            });
        }

        let list_metadata = self.all_nodes[list_start_index.0].token.list_metadata();

        let Some(last_child_index) = list_metadata.last_child_index else {
            // If no data in previous list, replace it with `Unit`, instead of an actual list.
            let commented_out = list_metadata.commented_out;
            let curr_node = &mut self.all_nodes[list_start_index.0];
            curr_node.token = DocumentToken::Unit { commented_out };

            let unit_start = self.pretty_printed.len() - 1;
            let _end_list_range = self.pretty_printed.end_list();
            curr_node.data_range = Some(unit_start..(unit_start + 2));

            return;
        };

        let should_display_on_own_line = match &self.all_nodes[last_child_index.0].token {
            DocumentToken::LineComment | DocumentToken::Error(_) => true,
            _ => false,
        };

        let should_append_to_pretty_printed = true;
        self.complete_single_list(
            list_start_index,
            should_display_on_own_line,
            should_append_to_pretty_printed,
        );
    }

    fn complete_single_list(
        &mut self,
        list_start_index: NodeIndex,
        should_display_on_own_line: bool,
        should_append_to_pretty_printed: bool,
    ) {
        let list_end_index = NodeIndex(self.all_nodes.len());
        let list_kind = self.analyze_list(list_start_index);

        let data_range = if should_append_to_pretty_printed {
            self.pretty_printed.end_list()
        } else {
            let len = self.pretty_printed.len();
            len..len
        };

        let list_start_node = &mut self.all_nodes[list_start_index.0];
        let list_metadata = list_start_node.token.list_metadata_mut();
        list_metadata.list_end_index = Some(list_end_index);
        list_metadata.list_kind = list_kind;

        let end_of_list_document_node = {
            let end_of_list_metadata = {
                EndOfListMetadata {
                    list_start_index,
                    should_display_on_own_line,
                }
            };

            DocumentNode {
                parent_index: list_start_node.parent_index,
                prev_sibling: list_start_node.prev_sibling,
                next_sibling: list_start_node.next_sibling,
                data_index_in_parent: None,
                data_range: Some(data_range),
                token: DocumentToken::EndOfList(end_of_list_metadata),
            }
        };

        self.all_nodes.push(end_of_list_document_node);
    }

    fn analyze_list(&self, list_start_index: NodeIndex) -> ListKind {
        let mut first_elem_atom_kind = None;
        let mut first_elem_list_kind = None;
        let mut second_elem_atom_kind = None;
        let mut all_elems_after_first_are_record_fields = true;
        let mut list_contains_non_data = false;

        let mut next_child_index = Some(NodeIndex(list_start_index.0 + 1));
        let mut list_length = 0;
        let mut uncommented_list_length = 0;

        while let Some(node_index) = next_child_index {
            let node = &self.all_nodes[node_index.0];
            next_child_index = node.next_sibling;

            if !node.token.is_data_node() {
                list_contains_non_data = true;
                continue;
            }

            // Don't consider commented out sexps for classification, so we don't say
            // something like "( #; Variant_record (x 1) (x 3))" is a variant record.
            if !node.token.is_commented_out() {
                if list_length == 0 {
                    first_elem_atom_kind = node.token.atom_kind();
                    first_elem_list_kind = node.token.list_kind();
                }

                if list_length == 1 {
                    second_elem_atom_kind = node.token.atom_kind();
                }

                uncommented_list_length += 1;
            }

            if list_length > 0 {
                // We don't ignore commented out tokens here under the assumption that
                // users will only comment out valid parts of data structures.
                all_elems_after_first_are_record_fields = all_elems_after_first_are_record_fields
                    && matches!(node.token.list_kind(), Some(ListKind::RecordField));
            }

            list_length += 1;
        }

        let first_elem_is_constructor = matches!(first_elem_atom_kind, Some(AtomKind::Constructor));
        let first_elem_is_record_key = matches!(first_elem_atom_kind, Some(AtomKind::RecordKey));
        let first_elem_is_record_field =
            matches!(first_elem_list_kind, Some(ListKind::RecordField));

        let first_two_elems_are_date_time = matches!(
            (first_elem_atom_kind, second_elem_atom_kind),
            (Some(AtomKind::Date), Some(AtomKind::Time)),
        );

        if first_elem_is_record_field && all_elems_after_first_are_record_fields {
            ListKind::Record
        } else if first_elem_is_constructor && all_elems_after_first_are_record_fields {
            ListKind::VariantRecord
        } else if first_elem_is_constructor && uncommented_list_length > 1 {
            ListKind::VariantTuple
        } else if first_two_elems_are_date_time && list_length == 2 && !list_contains_non_data {
            ListKind::DateTime
        } else if first_elem_is_record_key && list_length == 2 && uncommented_list_length == 2 {
            ListKind::RecordField
        } else if list_length == 1 && !list_contains_non_data {
            ListKind::Singleton
        } else if list_length == 0 {
            ListKind::Unit
        } else {
            ListKind::Plain
        }
    }

    fn add_atom(&mut self, atom_bytes: InputRef<'_, '_, RawBytes>) {
        let (atom_kind, unescaped_bytes) =
            match atom_bytes.unescape_atom(&mut self.scratch_buffer_for_unescaping_atoms) {
                Ok(unescape_result) => {
                    let unescaped_bytes = unescape_result.unescaped_bytes();
                    let atom_kind = Self::classify_atom_kind(unescaped_bytes);

                    (atom_kind, Some(unescaped_bytes))
                }
                Err(err) => {
                    self.push_new_error_node(ErrorMetadata {
                        message: format!("Unable to unescape atom: {:?}", err),
                    });

                    let atom_kind = AtomKind::Plain;

                    (atom_kind, None)
                }
            };

        let data_range = if let Some(unescaped_bytes) = unescaped_bytes {
            let atom = Atom::new(unescaped_bytes);
            self.pretty_printed.write_atom(atom)
        } else {
            self.pretty_printed
                .write_malformed_atom(atom_bytes.raw_bytes())
        };

        let quoted = matches!(&self.pretty_printed.data()[data_range.start], &b'"');
        let valid = unescaped_bytes.is_some();

        let atom_metadata = AtomMetadata {
            atom_kind,
            commented_out: self.consume_pending_sexp_comment(),
            quoted,
            valid,
        };

        let _ = self.push_new_document_node(DocumentToken::Atom(atom_metadata), Some(data_range));
    }

    fn classify_atom_kind(unescaped_bytes: &UnescapedBytes) -> AtomKind {
        if ATOM_BOOL_RE.is_match(unescaped_bytes) {
            AtomKind::Bool
        } else if ATOM_RECORD_KEY_RE.is_match(unescaped_bytes) {
            AtomKind::RecordKey
        } else if ATOM_CONSTRUCTOR_RE.is_match(unescaped_bytes) {
            AtomKind::Constructor
        } else if ATOM_INTEGER_RE.is_match(unescaped_bytes)
            || ATOM_FLOAT_RE.is_match(unescaped_bytes)
        {
            AtomKind::Number
        } else if unescaped_bytes.len() == ATOM_DATE_LENGTH
            && ATOM_DATE_RE.is_match(unescaped_bytes)
        {
            AtomKind::Date
        } else if ATOM_TIME_RE.is_match(unescaped_bytes) {
            AtomKind::Time
        } else {
            AtomKind::Plain
        }
    }

    fn add_sexp_comment(&mut self) {
        self.num_pending_sexp_comments += 1;
        self.pretty_printed.write_sexp_comment();
    }

    // Returns true if it did consume a pending sexp comment.
    fn consume_pending_sexp_comment(&mut self) -> bool {
        if self.num_pending_sexp_comments == 0 {
            false
        } else {
            self.num_pending_sexp_comments -= 1;
            true
        }
    }
}

struct Formatter<'a> {
    doc: &'a SexpDocument,
    lines: Vec<(NodeIndex, NodeIndex, usize)>,
    list_contents_indentation: Vec<usize>,
    current_index: NodeIndex,
    end_index_incl: NodeIndex,
}

impl<'a> Formatter<'a> {
    fn new(doc: &'a SexpDocument, node_index: NodeIndex) -> Formatter {
        let end_index_incl = match &doc.all_nodes[node_index.0].token {
            DocumentToken::StartOfList(ListMetadata { list_end_index, .. }) => {
                list_end_index.unwrap()
            }
            _ => node_index,
        };

        Formatter {
            doc,
            lines: vec![],
            list_contents_indentation: vec![],
            current_index: node_index,
            end_index_incl,
        }
    }

    fn current_indentation(&self) -> usize {
        self.list_contents_indentation
            .last()
            .map(|x| *x)
            .unwrap_or(0)
    }

    fn increase_current_indendation(&mut self, by: usize) {
        *self.list_contents_indentation.last_mut().unwrap() += by;
    }

    fn incr_current_index(&mut self) {
        self.current_index = NodeIndex(self.current_index.0 + 1);
    }

    fn consume_closing_parens(&mut self) {
        while self.current_index.0 <= self.end_index_incl.0 {
            if !matches!(
                &self.doc.all_nodes[self.current_index.0].token,
                DocumentToken::EndOfList(_),
            ) {
                break;
            }
            let popped_indentation = self.list_contents_indentation.pop();
            assert!(popped_indentation.is_some());
            self.incr_current_index();
        }
    }

    fn current_token_is_data(&self) -> bool {
        self.doc.all_nodes[self.current_index.0]
            .token
            .is_data_node()
    }

    fn current_token_is_atom(&self) -> bool {
        match &self.doc.all_nodes[self.current_index.0].token {
            DocumentToken::Atom(_) => true,
            _ => false,
        }
    }

    fn current_token_is_start_of_singleton(&self) -> bool {
        match &self.doc.all_nodes[self.current_index.0].token {
            DocumentToken::StartOfList(ListMetadata {
                list_kind: ListKind::Singleton,
                ..
            }) => true,
            _ => false,
        }
    }

    fn push_line(&mut self, start_index: NodeIndex, indentation: usize) {
        let end_index = NodeIndex(self.current_index.0 - 1);
        self.lines.push((start_index, end_index, indentation));
    }

    fn layout_list(&mut self, list_metadata: &ListMetadata) {
        let ListMetadata {
            list_kind,
            commented_out,
            ..
        } = list_metadata;

        let line_start_index = self.current_index;
        self.incr_current_index(); // StartOfList

        let list_indentation = self.current_indentation();
        let elem_indentation = if *commented_out {
            list_indentation + 4
        } else {
            list_indentation + 1
        };
        self.list_contents_indentation.push(elem_indentation);

        match list_kind {
            ListKind::Plain | ListKind::Unit => {}
            ListKind::Singleton => {}
            ListKind::Unit => {}
            ListKind::Record => {}
            ListKind::RecordField => {
                // We know the key will be the first non-comment token (and there won't be any
                // commented out sexps before it). If there are comments before the key, we'll
                // put those at no indentation, then we'll put key at no indentation. If there's
                // nothing in between the key and the value, we'll maybe put just the "start" of
                // the value on the same line as the key, if they are on separate lines, we'll
                // layout the value like normal.
                let mut key_line_start_index = line_start_index;
                let mut key_line_indentation = list_indentation;

                if !self.current_token_is_atom() {
                    // Push first non-key at the list indentation level.
                    self.incr_current_index(); // First comment
                    self.push_line(line_start_index, list_indentation);

                    // The key will be indented like a regular element.
                    key_line_indentation = elem_indentation;

                    // Layout additional non-keys at the elem level, one at a time.
                    while !self.current_token_is_atom() {
                        let comment_line_index = self.current_index;
                        self.incr_current_index(); // Subsequent comment
                        self.push_line(comment_line_index, elem_indentation);
                    }

                    key_line_start_index = self.current_index;
                }

                self.incr_current_index(); // key

                if !self.current_token_is_data() {
                    // The key is on its own line, because we have comments afterwards.
                    // We'll lay those out separately, indented two spaces relative to the key.
                    // (Two spaces, rather than the normal 1, to emphasize that it doesn't
                    // represent a normal container.)
                    self.push_line(key_line_start_index, key_line_indentation);
                    self.increase_current_indendation(2);
                } else {
                    // The value is on the same line as the key. We'll just push the "start" of it.

                    // Singletons coalesce
                    while self.current_token_is_start_of_singleton() {
                        self.incr_current_index(); // StartOfList

                        // TODO: Does it matter what I put here? I think only if it goes:
                        // <comment or error> EndOfList+ CorrespondingEndOfList <comment or error>
                        // We'll just do this for now.
                        self.list_contents_indentation.push(elem_indentation);
                    }

                    match &self.doc.all_nodes[self.current_index.0].token {
                        DocumentToken::Atom(_) | DocumentToken::Unit { .. } => {
                            self.incr_current_index(); // Atom or unit
                            self.consume_closing_parens();
                            self.push_line(key_line_start_index, key_line_indentation);
                        }
                        DocumentToken::StartOfList(list_metadata) => {
                            match list_metadata.list_kind {
                                // For regular lists and records, we'll just put the opening paren
                                // on this line, and then indent the elements appropriately.
                                // Technically a RecordField that's not inside a Record is meaningless,
                                // but we won't try to do something like: ((key ((key2 nested_value))).
                                // We'll just do:
                                // ((key (
                                //    key2
                                //    nested_value)))
                                ListKind::Plain
                                | ListKind::Record
                                | ListKind::Unit
                                | ListKind::RecordField => {
                                    self.incr_current_index(); // StartOfList

                                    // TODO: What to put here???
                                    self.list_contents_indentation.push(elem_indentation);

                                    self.push_line(key_line_start_index, key_line_indentation);
                                    // Indent elems by 1.
                                    self.increase_current_indendation(1);
                                }
                                ListKind::VariantRecord | ListKind::VariantTuple => {
                                    // Variants are tricky. By default, we just want to put the
                                    // start of list and the Constructor on the same line. If
                                    // there are comments between them we want to do...
                                    //
                                    // (foo (Variant
                                    //   (a 1)
                                    //   (b 2)))
                                    //
                                    // (foo (
                                    //    ; why
                                    //    Variant
                                    //     (a 1)
                                    self.incr_current_index(); // StartOfList

                                    if self.current_token_is_atom() {
                                        // Indent elems by 1 more than the key.
                                        self.list_contents_indentation.push(elem_indentation + 1);

                                        self.incr_current_index(); // Constructor
                                        self.push_line(key_line_start_index, key_line_indentation);
                                    } else {
                                        // Line for key and opening paren
                                        self.push_line(key_line_start_index, key_line_indentation);

                                        while !self.current_token_is_atom() {
                                            let comment_index = self.current_index;
                                            self.incr_current_index();
                                            self.push_line(comment_index, elem_indentation + 2)
                                        }

                                        // Push the constructor line
                                        let constructor_index = self.current_index;
                                        self.incr_current_index(); // Constructor
                                        self.push_line(constructor_index, elem_indentation + 2);
                                        self.list_contents_indentation.push(elem_indentation + 3);
                                    }
                                }
                                // We treat this like an atom, and put it on the same line
                                ListKind::DateTime => {
                                    self.incr_current_index(); // StartOfList

                                    // Value doesn't matter; we're about to pop it.
                                    self.list_contents_indentation.push(0);

                                    self.incr_current_index(); // Date
                                    self.incr_current_index(); // Time
                                    self.consume_closing_parens();
                                    self.push_line(key_line_start_index, key_line_indentation);
                                }
                                ListKind::Singleton => {
                                    panic!("We should have just skipped over all the singletons")
                                }
                            }
                        }
                        DocumentToken::EndOfList(_)
                        | DocumentToken::BlockComment
                        | DocumentToken::LineComment
                        | DocumentToken::Error(_) => panic!("current token must be a data node"),
                    }
                }
            }
            ListKind::VariantRecord | ListKind::VariantTuple => {
                // We know the constuctor will be the first non-comment token (and there won't be
                // any commented out sexps before it). If there are any comments, before the
                // constructor, we'll put them at no indentation, then everything after the
                // constructor will have an extra level of indentation.
                //
                // (Variant
                //   (key value)
                //   elem)
                //
                // (; questionably located comment
                //  #| mystery block comment |#
                //  Constructor
                //   value
                let mut constructor_line_start_index = line_start_index;
                let mut constructor_line_indentation = list_indentation;

                if !self.current_token_is_atom() {
                    // Push first non-constructor at the list indentation level.
                    self.incr_current_index(); // First comment
                    self.push_line(line_start_index, list_indentation);

                    // The constructor will be indented like a regular element.
                    constructor_line_indentation = elem_indentation;

                    // Layout additional non-constructors at the elem level, one at
                    // a time.
                    while !self.current_token_is_atom() {
                        let comment_line_index = self.current_index;
                        self.incr_current_index(); // Subsequent comments
                        self.push_line(comment_line_index, elem_indentation);
                    }

                    constructor_line_start_index = self.current_index;
                }

                // Finally layout the constructor:
                self.incr_current_index(); // Constructor
                self.push_line(constructor_line_start_index, constructor_line_indentation);

                // And indent variant elements an extra space
                self.increase_current_indendation(1);
            }
            ListKind::DateTime => {
                // DateTimes can't have any non-data in them, so we just advance past everything
                // and put it on the same line.
                self.incr_current_index(); // Date
                self.incr_current_index(); // Time
                self.consume_closing_parens();
                self.push_line(line_start_index, list_indentation);
            }
        }
    }

    fn layout(&mut self) {
        while self.current_index.0 <= self.end_index_incl.0 {
            let line_start_index = self.current_index;
            let starting_indentation = self.current_indentation();

            match &self.doc.all_nodes[self.current_index.0].token {
                DocumentToken::EndOfList(_) => {
                    self.consume_closing_parens();
                    self.push_line(line_start_index, self.current_indentation());
                }
                DocumentToken::Atom(_)
                | DocumentToken::Unit { .. }
                | DocumentToken::BlockComment => {
                    self.incr_current_index();
                    self.consume_closing_parens();
                    self.push_line(line_start_index, starting_indentation);
                }
                DocumentToken::LineComment | DocumentToken::Error(_) => {
                    self.incr_current_index();
                    self.push_line(line_start_index, starting_indentation);
                }
                DocumentToken::StartOfList(list_metadata) => self.layout_list(list_metadata),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bstr::ByteSlice;

    use std::fmt::Write;

    use insta::assert_snapshot;
    use ocaml_sexplib::input::{Input, SliceInput};
    use ocaml_sexplib::tokenizer::RawTokenizer;

    fn tokenizer(data: &'static [u8]) -> RawTokenizer<SliceInput<'static>> {
        let input = SliceInput::new(data);
        RawTokenizer::new(input)
    }

    const STOP_ATOM: &[u8] = b"STOP";

    fn feed_until_stop<'de, I: Input<'de>>(
        sexp_doc: &mut SexpDocument,
        tokenizer: &mut RawTokenizer<I>,
    ) {
        loop {
            match tokenizer.next_raw_token().unwrap() {
                Some(RawToken::Atom(atom_bytes)) if atom_bytes.raw_bytes() == STOP_ATOM => {
                    break;
                }
                Some(token) => {
                    sexp_doc.append_raw_token(token);
                }
                None => {
                    sexp_doc.append_eof();
                    break;
                }
            }
        }
    }

    fn dump_doc(doc: &SexpDocument) -> String {
        let mut output = String::new();

        let _ = writeln!(output, "Raw document:");
        let _ = writeln!(output, "{}", doc.pretty_printed.data().as_bstr());

        let _ = writeln!(output, "");

        for (i, node) in doc.all_nodes.iter().enumerate() {
            let _ = write!(output, "{:<4}", i);

            if let Some(data_range) = &node.data_range {
                let _ = write!(output, "{:<9}", format!("{:?}", data_range));
            } else {
                let _ = write!(output, "{:<9}", "");
            }

            let DocumentNode {
                parent_index,
                prev_sibling,
                next_sibling,
                data_range,
                data_index_in_parent,
                token,
            } = &node;

            fn fmt_i(node_index: Option<usize>) -> String {
                node_index
                    .map(|i| i.to_string())
                    .unwrap_or("--".to_string())
            }

            let _ = write!(output, "<{:<2} ", fmt_i(prev_sibling.map(|i| i.0)),);
            let _ = write!(
                output,
                "^{:>2}[{:<2}] ",
                fmt_i(parent_index.map(|i| i.0)),
                if token.is_commented_out() {
                    assert!(data_index_in_parent.is_none());
                    "#;".to_string()
                } else {
                    fmt_i(*data_index_in_parent)
                },
            );
            let _ = write!(output, "{:>2}> ", fmt_i(next_sibling.map(|i| i.0)),);

            let token = match token {
                DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => {
                    format!("StartOfList({:?})", list_kind)
                }
                DocumentToken::EndOfList(_) => {
                    format!("EndOfList")
                }
                DocumentToken::Atom(AtomMetadata { atom_kind, .. }) => {
                    format!("Atom({:?})", atom_kind)
                }
                DocumentToken::Unit { .. } => format!("Unit"),
                DocumentToken::LineComment => format!("LineComment"),
                DocumentToken::BlockComment => format!("BlockComment"),
                DocumentToken::Error(ErrorMetadata { message }) => {
                    let _ = writeln!(output, "Error: {}", message);
                    continue;
                }
            };

            let _ = write!(output, "{:<25}", token);
            if let Some(data_range) = data_range {
                let data = &doc.pretty_printed[data_range.clone()];
                let _ = write!(output, ": {:?}", data.as_bstr());
            }
            let _ = writeln!(output, "");
        }

        output
    }

    fn dump(bytes: &'static [u8]) -> String {
        let mut doc = SexpDocument::new();
        let mut tokenizer = tokenizer(bytes);
        feed_until_stop(&mut doc, &mut tokenizer);
        doc.append_eof();
        dump_doc(&doc)
    }

    #[test]
    fn test_basic_atom_classification() {
        let doc = dump(br#"("Atom Kinds:" Constructor record_key 123_456 7.89e10 true false 2021-07-20 22:42:32.000000000)"#);

        assert_snapshot!(&doc, @r#"
        Raw document:
        ("Atom Kinds:" Constructor record_key 123_456 7.89e10 true false 2021-07-20 22:42:32.000000000)

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..14    <-- ^ 0[0 ]  2> Atom(Plain)              : "\"Atom Kinds:\""
        2   15..26   <1  ^ 0[1 ]  3> Atom(Constructor)        : "Constructor"
        3   27..37   <2  ^ 0[2 ]  4> Atom(RecordKey)          : "record_key"
        4   38..45   <3  ^ 0[3 ]  5> Atom(Number)             : "123_456"
        5   46..53   <4  ^ 0[4 ]  6> Atom(Number)             : "7.89e10"
        6   54..58   <5  ^ 0[5 ]  7> Atom(Bool)               : "true"
        7   59..64   <6  ^ 0[6 ]  8> Atom(Bool)               : "false"
        8   65..75   <7  ^ 0[7 ]  9> Atom(Date)               : "2021-07-20"
        9   76..94   <8  ^ 0[8 ] --> Atom(Time)               : "22:42:32.000000000"
        10  94..95   <-- ^--[--] --> EndOfList                : ")"
        "#);
    }

    #[test]
    fn test_basic_list_classification() {
        let record = dump(b"((key1 a) (key2 b))");
        assert_snapshot!(&record, @r#"
        Raw document:
        ((key1 a) (key2 b))

        0   0..1     <-- ^--[0 ] --> StartOfList(Record)      : "("
        1   1..2     <-- ^ 0[0 ]  5> StartOfList(RecordField) : "("
        2   2..6     <-- ^ 1[0 ]  3> Atom(RecordKey)          : "key1"
        3   7..8     <2  ^ 1[1 ] --> Atom(RecordKey)          : "a"
        4   8..9     <-- ^ 0[--] --> EndOfList                : ")"
        5   10..11   <1  ^ 0[1 ] --> StartOfList(RecordField) : "("
        6   11..15   <-- ^ 5[0 ]  7> Atom(RecordKey)          : "key2"
        7   16..17   <6  ^ 5[1 ] --> Atom(RecordKey)          : "b"
        8   17..18   <1  ^ 0[--] --> EndOfList                : ")"
        9   18..19   <-- ^--[--] --> EndOfList                : ")"
        "#);

        let variant_record = dump(b"(Constructor (key1 a) (key2 b))");
        assert_snapshot!(&variant_record, @r#"
        Raw document:
        (Constructor (key1 a) (key2 b))

        0   0..1     <-- ^--[0 ] --> StartOfList(VariantRecord): "("
        1   1..12    <-- ^ 0[0 ]  2> Atom(Constructor)        : "Constructor"
        2   13..14   <1  ^ 0[1 ]  6> StartOfList(RecordField) : "("
        3   14..18   <-- ^ 2[0 ]  4> Atom(RecordKey)          : "key1"
        4   19..20   <3  ^ 2[1 ] --> Atom(RecordKey)          : "a"
        5   20..21   <1  ^ 0[--] --> EndOfList                : ")"
        6   22..23   <2  ^ 0[2 ] --> StartOfList(RecordField) : "("
        7   23..27   <-- ^ 6[0 ]  8> Atom(RecordKey)          : "key2"
        8   28..29   <7  ^ 6[1 ] --> Atom(RecordKey)          : "b"
        9   29..30   <2  ^ 0[--] --> EndOfList                : ")"
        10  30..31   <-- ^--[--] --> EndOfList                : ")"
        "#);

        let variant_tuple = dump(b"(Constructor () 1 2 3)");
        assert_snapshot!(&variant_tuple, @r#"
        Raw document:
        (Constructor () 1 2 3)

        0   0..1     <-- ^--[0 ] --> StartOfList(VariantTuple): "("
        1   1..12    <-- ^ 0[0 ]  2> Atom(Constructor)        : "Constructor"
        2   13..15   <1  ^ 0[1 ]  3> Unit                     : "()"
        3   16..17   <2  ^ 0[2 ]  4> Atom(Number)             : "1"
        4   18..19   <3  ^ 0[3 ]  5> Atom(Number)             : "2"
        5   20..21   <4  ^ 0[4 ] --> Atom(Number)             : "3"
        6   21..22   <-- ^--[--] --> EndOfList                : ")"
        "#);

        let date_time = dump(b"(2025-07-20 22:42:32.000000000)");
        assert_snapshot!(&date_time, @r#"
        Raw document:
        (2025-07-20 22:42:32.000000000)

        0   0..1     <-- ^--[0 ] --> StartOfList(DateTime)    : "("
        1   1..11    <-- ^ 0[0 ]  2> Atom(Date)               : "2025-07-20"
        2   12..30   <1  ^ 0[1 ] --> Atom(Time)               : "22:42:32.000000000"
        3   30..31   <-- ^--[--] --> EndOfList                : ")"
        "#);
    }

    #[test]
    fn test_basic_errors() {
        let unmatched_closing_paren = dump(b"one )");
        assert_snapshot!(&unmatched_closing_paren, @r#"
        Raw document:
        one

        0   0..3     <-- ^--[0 ]  1> Atom(RecordKey)          : "one"
        1            <0  ^--[--] --> Error: Saw unexpected ')' while parsing top-level sexp
        "#);

        let pending_sexp_comment_at_end_of_list = dump(b"(1 #;)");
        assert_snapshot!(&pending_sexp_comment_at_end_of_list, @r##"
        Raw document:
        (1 #;)

        0   0..1     <-- ^--[0 ]  2> StartOfList(Singleton)   : "("
        1   1..2     <-- ^ 0[0 ] --> Atom(Number)             : "1"
        2            <0  ^--[--] --> Error: Saw unexpected ')' after sexp comment "#;"
        3   5..6     <-- ^--[--]  2> EndOfList                : ")"
        "##);

        let invalid_atom_escape = dump(br#""\xGG""#);
        assert_snapshot!(&invalid_atom_escape, @r#"
        Raw document:
        "\xGG"

        0            <-- ^--[--]  1> Error: Unable to unescape atom: InvalidHexadecimalEscape
        1   0..6     <0  ^--[0 ] --> Atom(Plain)              : "\"\\xGG\""
        "#);

        let eof_before_list_end = dump(b"((a z");
        assert_snapshot!(&eof_before_list_end, @r#"
        Raw document:
        ((a z

        0   0..1     <-- ^--[0 ] --> StartOfList(Record)      : "("
        1   1..2     <-- ^ 0[0 ] --> StartOfList(RecordField) : "("
        2   2..3     <-- ^ 1[0 ]  3> Atom(RecordKey)          : "a"
        3   4..5     <2  ^ 1[1 ]  4> Atom(RecordKey)          : "z"
        4            <3  ^ 1[--] --> Error: Unexpected EOF while parsing list
        5   5..5     <-- ^ 0[--] --> EndOfList                : ""
        6   5..5     <-- ^--[--] --> EndOfList                : ""
        "#);
    }

    fn layout(bytes: &'static [u8]) -> String {
        let mut doc = SexpDocument::new();
        let mut tokenizer = tokenizer(bytes);
        feed_until_stop(&mut doc, &mut tokenizer);
        doc.append_eof();

        let mut formatter = Formatter::new(&mut doc, NodeIndex(0));
        formatter.layout();

        let mut output = String::new();

        for (start_node_index, end_node_index, indentation) in formatter.lines.iter() {
            let start_range = doc.all_nodes[start_node_index.0].data_range.clone();
            let end_range = doc.all_nodes[end_node_index.0].data_range.clone();

            let _ = write!(
                output,
                "{:>2}..={:<2} : ",
                start_node_index.0, end_node_index.0,
            );

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
    }
}
