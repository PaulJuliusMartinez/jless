use std::ops::Range;

use regex::bytes::Regex;
#[cfg(test)]
use serde::Serialize;

use ocaml_sexplib::atom::Atom;
use ocaml_sexplib::input::InputRef;
use ocaml_sexplib::tokenizer::{RawBytes, RawToken, UnescapedBytes};

use crate::sexp::pretty::PrettyPrinted;
use crate::sorted_ranges::SortedRanges;

#[cfg_attr(test, derive(Serialize))]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NodeIndex(pub usize);

impl std::ops::Add<usize> for NodeIndex {
    type Output = NodeIndex;

    fn add(self, rhs: usize) -> Self {
        NodeIndex(self.0 + rhs)
    }
}

impl std::ops::Sub<usize> for NodeIndex {
    type Output = NodeIndex;

    fn sub(self, rhs: usize) -> Self {
        NodeIndex(self.0 - rhs)
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct DocCore {
    // Someday: This shouldn't be marked public.
    // Pretty-printed data
    pub pretty_printed: PrettyPrinted,
    data_len_of_completed_sexps: usize,
    // Imagine we've ingested two and a half sexps:
    // > (a) (b) (
    // > 012 345 6 (node indexes)
    // then `node_index_of_last_completed_top_level_sexp` will be 3,
    // and `last_node_index_of_part_of_completed_sexp` will be 5.
    pub node_index_of_last_completed_top_level_sexp: Option<NodeIndex>,
    pub last_node_index_of_part_of_completed_sexp: Option<NodeIndex>,

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
pub struct DocumentNode {
    pub parent_index: Option<NodeIndex>,
    pub prev_sibling: Option<NodeIndex>,
    pub next_sibling: Option<NodeIndex>,
    /// Only set for atoms and the start of lists. Indicates the index of the node
    /// in the parent (or amongst all top-level nodes) if comments are ignored.
    pub data_index_in_parent: Option<usize>,
    pub data_range: Range<usize>,
    pub token: DocumentToken,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub enum DocumentToken {
    StartOfList(ListMetadata),
    EndOfList(EndOfListMetadata),
    Atom(AtomMetadata),
    Unit { commented_out: bool },
    LineComment,
    BlockComment,
    Error(ErrorMetadata),
}

impl DocumentToken {
    fn is_data(&self) -> bool {
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

    pub fn list_end_index(&self) -> Option<NodeIndex> {
        match self {
            DocumentToken::StartOfList(ListMetadata { list_end_index, .. }) => *list_end_index,
            _ => None,
        }
    }

    pub fn list_start_index(&self) -> Option<NodeIndex> {
        match self {
            DocumentToken::EndOfList(EndOfListMetadata {
                list_start_index, ..
            }) => Some(*list_start_index),
            _ => None,
        }
    }

    pub fn list_kind(&self) -> Option<ListKind> {
        match self {
            DocumentToken::StartOfList(ListMetadata { list_kind, .. }) => Some(*list_kind),
            DocumentToken::Unit { .. } => Some(ListKind::Plain),
            _ => None,
        }
    }

    pub fn atom_kind(&self) -> Option<AtomKind> {
        match self {
            DocumentToken::Atom(AtomMetadata { atom_kind, .. }) => Some(*atom_kind),
            _ => None,
        }
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct ListMetadata {
    pub list_kind: ListKind,
    // first_child_index is just our own index + 1.
    last_child_index: Option<NodeIndex>,
    list_end_index: Option<NodeIndex>,
    commented_out: bool,
    data_length: usize,
    contains_non_data: bool,
}

impl ListMetadata {
    pub fn last_child_index(&self) -> Option<NodeIndex> {
        self.last_child_index
    }

    pub fn end_index(&self) -> Option<NodeIndex> {
        self.list_end_index
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct AtomMetadata {
    pub atom_kind: AtomKind,
    commented_out: bool,
    quoted: bool,
    valid: bool,
    // printable_ascii: bool,
    // has_escapes: bool,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct EndOfListMetadata {
    pub list_start_index: NodeIndex,
    // We display closing parens on their own line if:
    // 1) the last child of the list is a line comment (so we're forced to), or
    // 2) the last child of the list is an error
    should_display_on_own_line: bool,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct ErrorMetadata {
    pub message: String,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Copy, Clone, Debug)]
pub enum AtomKind {
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

// Invariants about list kind classification that can be referenced when making
// certain assumptions about, e.g., lengths of lists, or whether comments can
// appear somewhere.
pub mod invariants {
    #[inline(always)]
    pub fn date_times_have_no_comments_errors_or_commented_out_sexps() {}

    #[inline(always)]
    pub fn constructors_are_the_first_child_of_variants() {}

    #[inline(always)]
    pub fn record_keys_are_the_first_child_of_record_fields() {}

    #[inline(always)]
    pub fn variants_have_at_least_one_argument() {}
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Copy, Clone, Debug)]
pub enum ListKind {
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
    /// Anything else.
    Plain,
}

impl ListKind {
    pub fn is_variant(&self) -> bool {
        matches!(self, ListKind::VariantRecord | ListKind::VariantTuple)
    }
}

impl DocCore {
    pub fn new() -> DocCore {
        DocCore {
            pretty_printed: PrettyPrinted::new(),
            data_len_of_completed_sexps: 0,
            node_index_of_last_completed_top_level_sexp: None,
            last_node_index_of_part_of_completed_sexp: None,
            all_nodes: vec![],
            last_top_level_node_index: None,
            num_top_level_data_nodes: 0,
            error_indexes: vec![],
            starts_of_unterminated_lists: vec![],
            num_pending_sexp_comments: 0,
            scratch_buffer_for_unescaping_atoms: vec![],
        }
    }

    #[cfg(test)]
    pub fn from_bytes(bytes: &'static [u8]) -> Self {
        use ocaml_sexplib::input::SliceInput;
        use ocaml_sexplib::tokenizer::RawTokenizer;

        let mut doc = DocCore::new();
        let mut tokenizer = RawTokenizer::new(SliceInput::new(bytes));
        while let Some(token) = tokenizer.next_raw_token().unwrap() {
            doc.append_raw_token(token);
        }
        doc.append_eof();
        doc
    }

    pub fn node(&self, node_index: NodeIndex) -> &DocumentNode {
        &self.all_nodes[node_index.0]
    }

    fn node_mut(&mut self, node_index: NodeIndex) -> &mut DocumentNode {
        &mut self.all_nodes[node_index.0]
    }

    pub fn token(&self, node_index: NodeIndex) -> &DocumentToken {
        &self.all_nodes[node_index.0].token
    }

    fn token_mut(&mut self, node_index: NodeIndex) -> &mut DocumentToken {
        &mut self.all_nodes[node_index.0].token
    }

    // Creates a new `DocumentNode` for the given token, and updates all the bookkeeping
    // appropriately:
    // - On parent node (or document itself if top-level node):
    //   - last child index
    //   - # of data nodes
    // - On previous sibling:
    //   - next sibling (if not adding a top-level node)
    // - On new node
    //   - parent index
    //   - data index in parent
    //   - prev sibling (if not adding a top-level node)
    //
    // This should _not_ be called when adding a new `EndOfList` token.
    fn push_new_document_node(
        &mut self,
        token: DocumentToken,
        data_range: Range<usize>,
    ) -> NodeIndex {
        assert!(!matches!(token, DocumentToken::EndOfList(_)));

        let new_node_index = NodeIndex(self.all_nodes.len());
        let token_is_start_of_list = matches!(token, DocumentToken::StartOfList(_));
        let token_is_error = matches!(token, DocumentToken::Error(_));

        let parent_index;
        let prev_sibling;
        let data_index_in_parent;

        match self.starts_of_unterminated_lists.last() {
            None => {
                // We're adding a new top-level node.
                self.last_top_level_node_index = Some(new_node_index);
                parent_index = None;

                // We'll connect this new node to the previous top-level sexp in
                // `complete_top_level_node`.
                prev_sibling = None;

                data_index_in_parent = if token.is_data() && !token.is_commented_out() {
                    let index = self.num_top_level_data_nodes;
                    self.num_top_level_data_nodes += 1;
                    Some(index)
                } else {
                    None
                };
            }
            Some(list_start_index) => {
                let list_start_index = *list_start_index; // End borrow on self.

                // We're adding a node inside a list; update the list metadata.
                let parent_metadata = self.token_mut(list_start_index).list_metadata_mut();

                parent_index = Some(list_start_index);

                prev_sibling = parent_metadata.last_child_index;
                parent_metadata.last_child_index = Some(new_node_index);

                data_index_in_parent = if token.is_data() && !token.is_commented_out() {
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
            self.node_mut(sibling_index).next_sibling = Some(new_node_index);
        }

        if token_is_error {
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

        // When we start a new list, we don't add it to `starts_of_unterminated_lists`
        // until we call this function and get the index, so it will be empty, and we
        // obviously don't want to complete a top-level node in that case, hence the
        // `!token_is_start_of_list`.
        if self.starts_of_unterminated_lists.is_empty() && !token_is_start_of_list {
            self.complete_top_level_node();
        }

        new_node_index
    }

    fn complete_top_level_node(&mut self) {
        // We can't have a completed node if we still have unterminated lists.
        assert!(self.starts_of_unterminated_lists.is_empty());

        // This means we've tried completing this node twice.
        assert!(self.last_node_index_of_part_of_completed_sexp != self.last_top_level_node_index);

        let new_completed_top_level_node = self
            .last_top_level_node_index
            .expect("must have created a top-level node before calling `complete_top_level_node`");

        self.pretty_printed.complete_top_level_node();
        self.data_len_of_completed_sexps = self.pretty_printed.len();

        // Wire up sibling connection between the new top-level node and the previous one.
        if let Some(prev_top_level_node_index) = self.node_index_of_last_completed_top_level_sexp {
            self.all_nodes[prev_top_level_node_index.0].next_sibling =
                Some(new_completed_top_level_node);
            self.all_nodes[new_completed_top_level_node.0].prev_sibling =
                Some(prev_top_level_node_index);

            // If the new top level node is a list, also set prev_sibling on the end of the list.
            if let Some(list_end_index) = self.all_nodes[new_completed_top_level_node.0]
                .token
                .list_end_index()
            {
                self.all_nodes[list_end_index.0].prev_sibling = Some(prev_top_level_node_index);
            }
        }

        self.node_index_of_last_completed_top_level_sexp = Some(new_completed_top_level_node);
        self.last_node_index_of_part_of_completed_sexp = Some(NodeIndex(self.all_nodes.len() - 1));
    }

    fn push_new_error_node(&mut self, error_metadata: ErrorMetadata) {
        // We'll say error nodes exist at an empty data range based on the current
        // end of the pretty printed doc.
        let end_of_doc = self.pretty_printed.len();
        let data_range = end_of_doc..end_of_doc;
        let _error_node_index =
            self.push_new_document_node(DocumentToken::Error(error_metadata), data_range);
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
                let _ = self.push_new_document_node(DocumentToken::LineComment, data_range);
            }
            RawToken::BlockComment(block_comment) => {
                // TODO: I need to validate these block comment bytes.
                let data_range = self
                    .pretty_printed
                    .write_block_comment(block_comment.raw_bytes());
                let _ = self.push_new_document_node(DocumentToken::BlockComment, data_range);
            }
            RawToken::SexpComment => self.add_sexp_comment(),
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
        let new_node_index =
            self.push_new_document_node(DocumentToken::StartOfList(list_metadata), data_range);

        self.starts_of_unterminated_lists.push(new_node_index);
    }

    fn complete_list(&mut self) {
        if self.num_pending_sexp_comments > 0 {
            // We didn't see a another list or an atom after a sexp-comment. We'll
            // clear it back to 0 to prevent further problems.
            self.num_pending_sexp_comments = 0;

            self.push_new_error_node(ErrorMetadata {
                message: "Saw unexpected ')' after sexp comment \"#;\"".to_string(),
            });
        }

        let Some(list_start_index) = self.starts_of_unterminated_lists.pop() else {
            // Saw a ')' while not in a list!
            self.push_new_error_node(ErrorMetadata {
                message: "Saw unexpected ')' while parsing top-level sexp".to_string(),
            });

            // Don't add the ')' to `pretty_printed`; it's invalid
            return;
        };

        let list_metadata = self.token(list_start_index).list_metadata();

        let Some(last_child_index) = list_metadata.last_child_index else {
            // If no data in previous list, replace it with `Unit`, instead of an actual list.
            let commented_out = list_metadata.commented_out;
            let curr_node = &mut self.all_nodes[list_start_index.0];
            curr_node.token = DocumentToken::Unit { commented_out };

            let unit_start = self.pretty_printed.len() - 1;
            let _end_list_range = self.pretty_printed.end_list();
            curr_node.data_range = unit_start..(unit_start + 2);

            // If we have no parent, then we just completed a top-level node.
            if curr_node.parent_index.is_none() {
                self.complete_top_level_node();
            }

            return;
        };

        let should_display_on_own_line = match &self.token(last_child_index) {
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
                data_range,
                token: DocumentToken::EndOfList(end_of_list_metadata),
            }
        };

        self.all_nodes.push(end_of_list_document_node);

        if self.starts_of_unterminated_lists.is_empty() {
            self.complete_top_level_node();
        }
    }

    fn analyze_list(&self, list_start_index: NodeIndex) -> ListKind {
        let mut first_elem_atom_kind = None;
        let mut first_elem_list_kind = None;
        let mut second_elem_atom_kind = None;
        let mut all_elems_after_first_are_record_fields = true;
        let mut all_elems_are_constructors = true;
        let mut list_contains_non_data_before_first_elem = false;
        let mut list_contains_non_data = false;

        let mut next_child_index = Some(list_start_index + 1);
        let mut list_length_including_commented_out_sexps = 0;
        let mut list_length_not_including_commented_out_sexps = 0;

        while let Some(node_index) = next_child_index {
            let node = &self.node(node_index);
            next_child_index = node.next_sibling;

            if !node.token.is_data() {
                list_contains_non_data = true;
                if list_length_including_commented_out_sexps == 0 {
                    list_contains_non_data_before_first_elem = true;
                }
                continue;
            }

            let atom_kind = node.token.atom_kind();

            // Commented-out sexps are tricky. We'll assume that users normally
            // only comment out parts of valid data structures, so they should be
            // be used to e.g. disqualify something from being a record field
            // (e.g. "(one #; two three)" is not a record field), but they also
            // can't form a critical part of a structure, so "(#; one two)" isn't
            // a record field either, and "( #; Constructor one two three)" isn't
            // a variant tuple.
            //
            // We can accomplish this by excluding commented out sexps from the
            // fields we use to identify certain structures, but including them
            // when considering aggregate info.

            if !node.token.is_commented_out() {
                if list_length_including_commented_out_sexps == 0 {
                    first_elem_atom_kind = atom_kind;
                    first_elem_list_kind = node.token.list_kind();
                }

                if list_length_including_commented_out_sexps == 1 {
                    second_elem_atom_kind = atom_kind;
                }

                list_length_not_including_commented_out_sexps += 1;
            }

            // This is a heuristic so that we don't consider a list of enums,
            // e.g. "(Monday Tuesday Wednesday Thursday Friday)", a variant
            // tuple.
            all_elems_are_constructors =
                all_elems_are_constructors && matches!(atom_kind, Some(AtomKind::Constructor));

            if list_length_including_commented_out_sexps > 0 {
                all_elems_after_first_are_record_fields = all_elems_after_first_are_record_fields
                    && matches!(node.token.list_kind(), Some(ListKind::RecordField));
            }

            list_length_including_commented_out_sexps += 1;
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
        } else if !list_contains_non_data_before_first_elem
            && first_elem_is_constructor
            && all_elems_after_first_are_record_fields
            && list_length_not_including_commented_out_sexps > 1
        {
            invariants::constructors_are_the_first_child_of_variants();
            invariants::variants_have_at_least_one_argument();
            ListKind::VariantRecord
        } else if !list_contains_non_data_before_first_elem
            && first_elem_is_constructor
            && list_length_not_including_commented_out_sexps > 1
            && !all_elems_are_constructors
        {
            invariants::constructors_are_the_first_child_of_variants();
            invariants::variants_have_at_least_one_argument();
            ListKind::VariantTuple
        } else if first_two_elems_are_date_time
            && list_length_including_commented_out_sexps == 2
            && !list_contains_non_data
        {
            invariants::date_times_have_no_comments_errors_or_commented_out_sexps();
            ListKind::DateTime
        } else if !list_contains_non_data_before_first_elem
            && first_elem_is_record_key
            && list_length_including_commented_out_sexps == 2
            && list_length_not_including_commented_out_sexps == 2
        {
            invariants::record_keys_are_the_first_child_of_record_fields();
            ListKind::RecordField
        } else if list_length_including_commented_out_sexps == 1
            && list_length_not_including_commented_out_sexps == 1
            && !list_contains_non_data
        {
            ListKind::Singleton
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

        let _ = self.push_new_document_node(DocumentToken::Atom(atom_metadata), data_range);
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

    pub fn raw_bytes_of_complete_content(&self) -> &[u8] {
        &self.pretty_printed.data()[..self.data_len_of_completed_sexps]
    }

    pub fn closest_node_to_byte_index(&self, byte_index: usize) -> NodeIndex {
        debug_assert!(byte_index < self.pretty_printed.len());
        // The last node will always end at `self.pretty_printed.len()`, so we will
        // always find a value and can unwrap safely.
        NodeIndex(self.index_of_first_elem_ending_after(byte_index).unwrap())
    }
}

impl SortedRanges for DocCore {
    type Elem = DocumentNode;

    fn elems(&self) -> &[Self::Elem] {
        &self.all_nodes
    }

    fn elem_start(elem: &Self::Elem) -> usize {
        elem.data_range.start
    }

    fn elem_end(elem: &Self::Elem) -> usize {
        elem.data_range.end
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fmt::Write;

    use bstr::ByteSlice;
    use insta::assert_snapshot;

    fn dump_doc(doc: &DocCore) -> String {
        let mut output = String::new();

        let _ = writeln!(output, "Raw document:");
        let _ = writeln!(output, "{}", doc.pretty_printed.data().as_bstr());

        let _ = writeln!(output, "");

        for (i, node) in doc.all_nodes.iter().enumerate() {
            let _ = write!(output, "{:<4}", i);
            let _ = write!(output, "{:<9}", format!("{:?}", &node.data_range));

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

            let data = &doc.pretty_printed[data_range.clone()];
            let _ = write!(output, ": {:?}", data.as_bstr());

            let _ = writeln!(output, "");
        }

        output
    }

    fn dump(bytes: &'static [u8]) -> String {
        let doc = DocCore::from_bytes(bytes);
        dump_doc(&doc)
    }

    #[test]
    fn test_incrementally_build_up_sexps() {
        let mut doc = DocCore::new();
        assert_snapshot!(dump_doc(&doc), @"Raw document:");

        doc.append_raw_token(RawToken::LeftParen);
        doc.append_raw_token(RawToken::Atom(InputRef::Transient(&RawBytes::new(b"atom"))));

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..5     <-- ^ 0[0 ] --> Atom(RecordKey)          : "atom"
        "#);

        doc.append_raw_token(RawToken::LeftParen);

        // Next/prev sibling pointers are immediately setup for the atom and new list,
        // even though the new list isn't complete yet.
        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom (

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..5     <-- ^ 0[0 ]  2> Atom(RecordKey)          : "atom"
        2   6..7     <1  ^ 0[1 ] --> StartOfList(Plain)       : "("
        "#);

        doc.append_raw_token(RawToken::RightParen);

        // The start of list is converted into a Unit.
        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom ()

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..5     <-- ^ 0[0 ]  2> Atom(RecordKey)          : "atom"
        2   6..8     <1  ^ 0[1 ] --> Unit                     : "()"
        "#);

        doc.append_raw_token(RawToken::RightParen);
        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom ())

        0   0..1     <-- ^--[0 ] --> StartOfList(RecordField) : "("
        1   1..5     <-- ^ 0[0 ]  2> Atom(RecordKey)          : "atom"
        2   6..8     <1  ^ 0[1 ] --> Unit                     : "()"
        3   8..9     <-- ^--[--] --> EndOfList                : ")"
        "#);
    }

    #[test]
    fn test_incrementally_build_up_multiple_top_level_sexps() {
        let mut doc = DocCore::new();
        doc.append_raw_token(RawToken::LeftParen);
        doc.append_raw_token(RawToken::Atom(InputRef::Transient(&RawBytes::new(b"atom"))));
        doc.append_raw_token(RawToken::RightParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom)

        0   0..1     <-- ^--[0 ] --> StartOfList(Singleton)   : "("
        1   1..5     <-- ^ 0[0 ] --> Atom(RecordKey)          : "atom"
        2   5..6     <-- ^--[--] --> EndOfList                : ")"
        "#);

        doc.append_raw_token(RawToken::LeftParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom)
        (

        0   0..1     <-- ^--[0 ] --> StartOfList(Singleton)   : "("
        1   1..5     <-- ^ 0[0 ] --> Atom(RecordKey)          : "atom"
        2   5..6     <-- ^--[--] --> EndOfList                : ")"
        3   7..8     <-- ^--[1 ] --> StartOfList(Plain)       : "("
        "#);

        doc.append_raw_token(RawToken::RightParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom)
        ()

        0   0..1     <-- ^--[0 ]  3> StartOfList(Singleton)   : "("
        1   1..5     <-- ^ 0[0 ] --> Atom(RecordKey)          : "atom"
        2   5..6     <-- ^--[--] --> EndOfList                : ")"
        3   7..9     <0  ^--[1 ] --> Unit                     : "()"
        "#);
    }

    #[test]
    fn test_connect_top_level_nodes_when_one_is_unit() {
        let mut doc = DocCore::new();
        doc.append_raw_token(RawToken::Atom(InputRef::Transient(&RawBytes::new(b"atom"))));
        doc.append_raw_token(RawToken::LeftParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        atom
        (

        0   0..4     <-- ^--[0 ] --> Atom(RecordKey)          : "atom"
        1   5..6     <-- ^--[1 ] --> StartOfList(Plain)       : "("
        "#);

        doc.append_raw_token(RawToken::RightParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        atom
        ()

        0   0..4     <-- ^--[0 ]  1> Atom(RecordKey)          : "atom"
        1   5..7     <0  ^--[1 ] --> Unit                     : "()"
        "#);
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

        invariants::record_keys_are_the_first_child_of_record_fields();
        let not_a_record = dump(b"((#| comment |# key2 b))");
        assert_snapshot!(&not_a_record, @r##"
        Raw document:
        ((#| comment |# key2 b))

        0   0..1     <-- ^--[0 ] --> StartOfList(Singleton)   : "("
        1   1..2     <-- ^ 0[0 ] --> StartOfList(Plain)       : "("
        2   2..15    <-- ^ 1[--]  3> BlockComment             : "#| comment |#"
        3   16..20   <2  ^ 1[0 ]  4> Atom(RecordKey)          : "key2"
        4   21..22   <3  ^ 1[1 ] --> Atom(RecordKey)          : "b"
        5   22..23   <-- ^ 0[--] --> EndOfList                : ")"
        6   23..24   <-- ^--[--] --> EndOfList                : ")"
        "##);

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

        invariants::constructors_are_the_first_child_of_variants();
        let not_a_variant_record = dump(b"(#| comment |# Constructor (a 1))");
        assert_snapshot!(&not_a_variant_record, @r##"
        Raw document:
        (#| comment |# Constructor (a 1))

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..14    <-- ^ 0[--]  2> BlockComment             : "#| comment |#"
        2   15..26   <1  ^ 0[0 ]  3> Atom(Constructor)        : "Constructor"
        3   27..28   <2  ^ 0[1 ] --> StartOfList(RecordField) : "("
        4   28..29   <-- ^ 3[0 ]  5> Atom(RecordKey)          : "a"
        5   30..31   <4  ^ 3[1 ] --> Atom(Number)             : "1"
        6   31..32   <2  ^ 0[--] --> EndOfList                : ")"
        7   32..33   <-- ^--[--] --> EndOfList                : ")"
        "##);

        invariants::constructors_are_the_first_child_of_variants();
        let not_a_variant_tuple = dump(b"(#| comment |# Constructor 1)");
        assert_snapshot!(&not_a_variant_tuple, @r##"
        Raw document:
        (#| comment |# Constructor 1)

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..14    <-- ^ 0[--]  2> BlockComment             : "#| comment |#"
        2   15..26   <1  ^ 0[0 ]  3> Atom(Constructor)        : "Constructor"
        3   27..28   <2  ^ 0[1 ] --> Atom(Number)             : "1"
        4   28..29   <-- ^--[--] --> EndOfList                : ")"
        "##);

        let singleton_is_not_a_variant = dump(b"(Constructor)");
        assert_snapshot!(&singleton_is_not_a_variant, @r#"
        Raw document:
        (Constructor)

        0   0..1     <-- ^--[0 ] --> StartOfList(Singleton)   : "("
        1   1..12    <-- ^ 0[0 ] --> Atom(Constructor)        : "Constructor"
        2   12..13   <-- ^--[--] --> EndOfList                : ")"
        "#);

        let list_of_constructors_is_not_a_variant = dump(b"(One Two Three)");
        assert_snapshot!(&list_of_constructors_is_not_a_variant, @r#"
        Raw document:
        (One Two Three)

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..4     <-- ^ 0[0 ]  2> Atom(Constructor)        : "One"
        2   5..8     <1  ^ 0[1 ]  3> Atom(Constructor)        : "Two"
        3   9..14    <2  ^ 0[2 ] --> Atom(Constructor)        : "Three"
        4   14..15   <-- ^--[--] --> EndOfList                : ")"
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

        let unit = dump(b"() (#| one |#) ( #; )");
        assert_snapshot!(&unit, @r##"
        Raw document:
        ()
        (#| one |#)
        (#;)

        0   0..2     <-- ^--[0 ]  1> Unit                     : "()"
        1   3..4     <0  ^--[1 ]  4> StartOfList(Plain)       : "("
        2   4..13    <-- ^ 1[--] --> BlockComment             : "#| one |#"
        3   13..14   <0  ^--[--] --> EndOfList                : ")"
        4   15..16   <1  ^--[2 ] --> StartOfList(Plain)       : "("
        5   18..18   <-- ^ 4[--] --> Error: Saw unexpected ')' after sexp comment "#;"
        6   18..19   <1  ^--[--] --> EndOfList                : ")"
        "##);
    }

    #[test]
    fn test_basic_errors() {
        let unmatched_closing_paren = dump(b"one )");
        assert_snapshot!(unmatched_closing_paren, @r#"
        Raw document:
        one

        0   0..3     <-- ^--[0 ]  1> Atom(RecordKey)          : "one"
        1   3..3     <0  ^--[--] --> Error: Saw unexpected ')' while parsing top-level sexp
        "#);

        let pending_sexp_comment_at_eof = dump(b"a #;");
        assert_snapshot!(pending_sexp_comment_at_eof, @r##"
        Raw document:
        a
        #;

        0   0..1     <-- ^--[0 ]  1> Atom(RecordKey)          : "a"
        1   4..4     <0  ^--[--] --> Error: Unexpected EOF after sexp comment "#;"
        "##);

        let pending_sexp_comment_in_list_at_eof = dump(b"a (#;");
        assert_snapshot!(pending_sexp_comment_in_list_at_eof, @r##"
        Raw document:
        a
        (#;

        0   0..1     <-- ^--[0 ]  1> Atom(RecordKey)          : "a"
        1   2..3     <0  ^--[1 ] --> StartOfList(Plain)       : "("
        2   5..5     <-- ^ 1[--]  3> Error: Unexpected EOF after sexp comment "#;"
        3   5..5     <2  ^ 1[--] --> Error: Unexpected EOF while parsing list
        4   5..5     <0  ^--[--] --> EndOfList                : ""
        "##);

        let pending_sexp_comment_at_end_of_list = dump(b"a (1 #;)");
        assert_snapshot!(pending_sexp_comment_at_end_of_list, @r##"
        Raw document:
        a
        (1 #;)

        0   0..1     <-- ^--[0 ]  1> Atom(RecordKey)          : "a"
        1   2..3     <0  ^--[1 ] --> StartOfList(Plain)       : "("
        2   3..4     <-- ^ 1[0 ]  3> Atom(Number)             : "1"
        3   7..7     <2  ^ 1[--] --> Error: Saw unexpected ')' after sexp comment "#;"
        4   7..8     <0  ^--[--] --> EndOfList                : ")"
        "##);

        let invalid_atom_escape = dump(br#""\xGG""#);
        assert_snapshot!(invalid_atom_escape, @r#"
        Raw document:

        "\xGG"

        0   0..0     <-- ^--[--]  1> Error: Unable to unescape atom: InvalidHexadecimalEscape
        1   1..7     <0  ^--[0 ] --> Atom(Plain)              : "\"\\xGG\""
        "#);

        let eof_before_list_end = dump(b"1 ((a z");
        assert_snapshot!(eof_before_list_end, @r#"
        Raw document:
        1
        ((a z

        0   0..1     <-- ^--[0 ]  1> Atom(Number)             : "1"
        1   2..3     <0  ^--[1 ] --> StartOfList(Record)      : "("
        2   3..4     <-- ^ 1[0 ] --> StartOfList(RecordField) : "("
        3   4..5     <-- ^ 2[0 ]  4> Atom(RecordKey)          : "a"
        4   6..7     <3  ^ 2[1 ]  5> Atom(RecordKey)          : "z"
        5   7..7     <4  ^ 2[--] --> Error: Unexpected EOF while parsing list
        6   7..7     <-- ^ 1[--] --> EndOfList                : ""
        7   7..7     <0  ^--[--] --> EndOfList                : ""
        "#);
    }

    #[test]
    fn test_closest_node_to_byte_index() {
        let doc = DocCore::from_bytes(b"((one two) #; three ; four\n)");
        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        ((one two) #; three ; four
        )

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..2     <-- ^ 0[0 ]  5> StartOfList(RecordField) : "("
        2   2..5     <-- ^ 1[0 ]  3> Atom(RecordKey)          : "one"
        3   6..9     <2  ^ 1[1 ] --> Atom(RecordKey)          : "two"
        4   9..10    <-- ^ 0[--] --> EndOfList                : ")"
        5   14..19   <1  ^ 0[#;]  6> Atom(RecordKey)          : "three"
        6   20..26   <5  ^ 0[--] --> LineComment              : "; four"
        7   27..28   <-- ^--[--] --> EndOfList                : ")"
        "#);

        assert_eq!(doc.closest_node_to_byte_index(0), NodeIndex(0));
        assert_eq!(doc.closest_node_to_byte_index(1), NodeIndex(1));
        assert_eq!(doc.closest_node_to_byte_index(4), NodeIndex(2));
        assert_eq!(doc.closest_node_to_byte_index(5), NodeIndex(3));
        assert_eq!(doc.closest_node_to_byte_index(10), NodeIndex(5));
        assert_eq!(doc.closest_node_to_byte_index(12), NodeIndex(5));
        assert_eq!(doc.closest_node_to_byte_index(27), NodeIndex(7));
        assert_eq!(doc.pretty_printed.len(), 28);

        let doc = DocCore::from_bytes(b"(a");
        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (a

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..2     <-- ^ 0[0 ]  2> Atom(RecordKey)          : "a"
        2   2..2     <1  ^ 0[--] --> Error: Unexpected EOF while parsing list
        3   2..2     <-- ^--[--] --> EndOfList                : ""
        "#);

        assert_eq!(doc.closest_node_to_byte_index(1), NodeIndex(1));
    }
}
