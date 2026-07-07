use std::fmt::Write;
use std::ops::Range;

use regex::bytes::Regex;
#[cfg(test)]
use serde::Serialize;

use ocaml_sexplib::atom::{AtomData, PlausibleSerializedAtom};
use ocaml_sexplib::tokenizer::RawToken;
use ocaml_sexplib::Ref;

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
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct OptNodeIndex(usize);

impl OptNodeIndex {
    const NONE_VAL: usize = usize::MAX;
    const NONE: Self = OptNodeIndex(Self::NONE_VAL);

    fn to_option(self) -> Option<NodeIndex> {
        if self.0 == Self::NONE_VAL {
            None
        } else {
            Some(NodeIndex(self.0))
        }
    }

    fn is_none(&self) -> bool {
        self.0 == Self::NONE_VAL
    }
}

impl std::fmt::Debug for OptNodeIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.to_option().fmt(f)
    }
}

impl From<OptNodeIndex> for Option<NodeIndex> {
    fn from(x: OptNodeIndex) -> Self {
        x.to_option()
    }
}

impl From<Option<NodeIndex>> for OptNodeIndex {
    fn from(x: Option<NodeIndex>) -> Self {
        match x {
            None => OptNodeIndex::NONE,
            Some(x) if x.0 == OptNodeIndex::NONE_VAL => {
                panic!("Tried to make an OptNodeIndex from a NodeIndex that was too big");
            }
            Some(x) => OptNodeIndex(x.0),
        }
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
    // Sexp comments can stack (e.g. "#; #; a b" comments out both "a" and "b"), but
    // also only apply at a certain depth:
    //
    //         (  4          5
    // $ echo "#; #; (1 2 3) #; 4 5 6" | sexp print
    // 6
    //
    //         (     2       4
    // $ echo "#; (1 #; 2 3) #; 4 5 6" | sexp print
    // 5
    // 6
    //
    // To handle this correctly, we track the depth (= starts_of_unterminated_lists.len())
    // we saw each sexp comment at, and then only apply it to nodes at that depth. When we
    // pop a list, and there are unused comments at that depth, it's an error.
    depths_of_pending_sexp_comments: Vec<usize>,
    scratch_buffer_for_unescaping_atoms: Vec<u8>,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct DocumentNode {
    parent_index: OptNodeIndex,
    prev_sibling: OptNodeIndex,
    next_sibling: OptNodeIndex,
    /// Only set for atoms and the start of lists. Indicates the index of the node
    /// in the parent (or amongst all top-level nodes) if line/block comments are
    /// ignored. This will be None for nodes that have been sexp commented out
    /// (though it will be set in its children).
    pub data_index_in_parent: Option<usize>,
    // For tokens that are sexp-commented out (e.g., "#; atom"), this does _not_
    // include the range of the preceding "#; ". For that, call `sexp_comment_range`.
    pub data_range: Range<usize>,
    pub token: DocumentToken,
}

impl DocumentNode {
    pub fn parent_index(&self) -> Option<NodeIndex> {
        self.parent_index.to_option()
    }

    pub fn prev_sibling(&self) -> Option<NodeIndex> {
        self.prev_sibling.to_option()
    }

    pub fn next_sibling(&self) -> Option<NodeIndex> {
        self.next_sibling.to_option()
    }

    pub fn sexp_comment_range(&self) -> Option<Range<usize>> {
        if self.token.is_sexp_commented_out() {
            let data_start = self.data_range.start;
            Some((data_start - 3)..data_start)
        } else {
            None
        }
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub enum DocumentToken {
    StartOfList(ListMetadata),
    EndOfList(EndOfListMetadata),
    Atom(AtomMetadata),
    Unit { sexp_commented_out: bool },
    LineComment,
    BlockComment,
    Error(ErrorMetadata),
}

impl DocumentToken {
    pub fn is_data(&self) -> bool {
        matches!(
            self,
            DocumentToken::StartOfList(_) | DocumentToken::Atom(_) | DocumentToken::Unit { .. },
        )
    }

    pub fn is_sexp_commented_out(&self) -> bool {
        match self {
            DocumentToken::StartOfList(ListMetadata {
                sexp_commented_out, ..
            })
            | DocumentToken::Atom(AtomMetadata {
                sexp_commented_out, ..
            })
            | DocumentToken::Unit { sexp_commented_out } => *sexp_commented_out,
            _ => false,
        }
    }

    pub fn list_metadata(&self) -> &ListMetadata {
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
            DocumentToken::StartOfList(ListMetadata { list_end_index, .. }) => {
                list_end_index.to_option()
            }
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
    last_child_index: OptNodeIndex,
    list_end_index: OptNodeIndex,
    sexp_commented_out: bool,
    data_length: usize,
    contains_non_data: bool,
}

impl ListMetadata {
    // Someday: Do we need these functions vs. just marking these fields public?

    pub fn last_child_index(&self) -> Option<NodeIndex> {
        self.last_child_index.to_option()
    }

    pub fn end_index(&self) -> Option<NodeIndex> {
        self.list_end_index.to_option()
    }

    pub fn data_length(&self) -> usize {
        self.data_length
    }
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct AtomMetadata {
    pub atom_kind: AtomKind,
    sexp_commented_out: bool,
    quoted: bool,
    valid: bool,
    // printable_ascii: bool,
    // has_escapes: bool,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct EndOfListMetadata {
    pub list_start_index: NodeIndex,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct ErrorMetadata {
    pub message: String,
}

#[cfg_attr(test, derive(Serialize))]
#[derive(Eq, PartialEq, Copy, Clone, Debug)]
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
      # Optional decimal seconds
      (
          \.          # Decimal
          [0-9]{3}    # ms
          ([0-9]{3})? # Optional us
          ([0-9]{3})? # Optional ns
      )?
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
        [0-9][0-9_]*               # Leading digits
        (\.[0-9_]*)?               # Optional decimal
        ([eE](\+|-)?[0-9][0-9_]*)? # Optional exponent
      | # Hex floating point
        0[xX]                      # Leading 0x
        [0-9A-Fa-f][0-9A-Fa-f_]*   # Leading digits
        (\.[0-9A-Fa-f_]*)?         # Optional decimal
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
    pub fn variant_constructors_are_not_sexp_commented_out() {}

    #[inline(always)]
    pub fn record_keys_are_the_first_child_of_record_fields() {}

    #[inline(always)]
    pub fn record_fields_do_not_contain_commented_out_sexps() {}

    #[inline(always)]
    pub fn variants_have_at_least_one_non_sexp_commented_out_argument() {}

    #[inline(always)]
    pub fn record_keys_always_match_record_key_regex() {}

    #[inline(always)]
    pub fn singleton_values_are_not_commented_out_sexps() {}
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
            depths_of_pending_sexp_comments: vec![],
            scratch_buffer_for_unescaping_atoms: vec![],
        }
    }

    #[cfg(test)]
    pub fn from_bytes(bytes: &'static [u8], write_eof: bool) -> Self {
        use ocaml_sexplib::input::SliceInput;
        use ocaml_sexplib::tokenizer::RawTokenizer;

        let mut doc = DocCore::new();
        let mut tokenizer = RawTokenizer::new(SliceInput::new(bytes));
        while let Some(token) = tokenizer.next_raw_token().unwrap() {
            doc.append_raw_token(token);
        }
        if write_eof {
            doc.append_eof();
        }
        doc
    }

    pub fn node(&self, node_index: NodeIndex) -> &DocumentNode {
        &self.all_nodes[node_index.0]
    }

    fn node_mut(&mut self, node_index: NodeIndex) -> &mut DocumentNode {
        &mut self.all_nodes[node_index.0]
    }

    pub fn parent_index(&self, node_index: NodeIndex) -> Option<NodeIndex> {
        self.all_nodes[node_index.0].parent_index()
    }

    pub fn depth(&self, mut node_index: NodeIndex) -> usize {
        let mut depth = 0;
        while let Some(parent_index) = self.parent_index(node_index) {
            depth += 1;
            node_index = parent_index;
        }
        depth
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

                data_index_in_parent = if token.is_data() && !token.is_sexp_commented_out() {
                    let index = self.num_top_level_data_nodes;
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

                prev_sibling = parent_metadata.last_child_index.to_option();
                parent_metadata.last_child_index = Some(new_node_index).into();

                data_index_in_parent = if token.is_data() && !token.is_sexp_commented_out() {
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
            self.node_mut(sibling_index).next_sibling = Some(new_node_index).into();
        }

        if token_is_error {
            self.error_indexes.push(new_node_index);
        }

        let document_node = DocumentNode {
            parent_index: parent_index.into(),
            prev_sibling: prev_sibling.into(),
            next_sibling: OptNodeIndex::NONE,
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

        if self.all_nodes[new_completed_top_level_node.0]
            .token
            .is_data()
        {
            self.num_top_level_data_nodes += 1;
        }

        // Wire up sibling connection between the new top-level node and the previous one.
        if let Some(prev_top_level_node_index) = self.node_index_of_last_completed_top_level_sexp {
            self.all_nodes[prev_top_level_node_index.0].next_sibling =
                Some(new_completed_top_level_node).into();
            self.all_nodes[new_completed_top_level_node.0].prev_sibling =
                Some(prev_top_level_node_index).into();

            // If the new top level node is a list, also set prev_sibling on the end of the list.
            if let Some(list_end_index) = self.all_nodes[new_completed_top_level_node.0]
                .token
                .list_end_index()
            {
                self.all_nodes[list_end_index.0].prev_sibling =
                    Some(prev_top_level_node_index).into();
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
            RawToken::Atom(plausible_atom_bytes) => self.add_atom(plausible_atom_bytes),
            RawToken::LineComment(line_comment) => {
                let data_range = self.pretty_printed.write_line_comment(line_comment.bytes());
                let _ = self.push_new_document_node(DocumentToken::LineComment, data_range);
            }
            RawToken::BlockComment(block_comment) => {
                // TODO: I need to validate these block comment bytes.
                let data_range = self
                    .pretty_printed
                    .write_block_comment(block_comment.bytes());
                let _ = self.push_new_document_node(DocumentToken::BlockComment, data_range);
            }
            RawToken::SexpComment => self.track_sexp_comment(),
        }
    }

    pub fn append_tokenizer_error(&mut self, err: ocaml_sexplib::Error) {
        self.push_new_error_node(ErrorMetadata {
            message: format!("{:?}", err),
        });
    }

    pub fn append_eof(&mut self) {
        // We just have to check for errors here, pending sexp comments and unterminated lists.

        if !self.depths_of_pending_sexp_comments.is_empty() {
            self.depths_of_pending_sexp_comments.clear();

            self.push_new_error_node(ErrorMetadata {
                message: "Unexpected EOF while sexp comment \"#;\" pending".to_string(),
            });
        }

        if self.starts_of_unterminated_lists.is_empty() {
            return;
        }

        self.push_new_error_node(ErrorMetadata {
            message: "Unexpected EOF while parsing list".to_string(),
        });

        while let Some(list_start_index) = self.starts_of_unterminated_lists.pop() {
            // We won't actually add the trailing ')' to the internal doc.
            let should_append_to_pretty_printed = false;
            self.complete_single_list(list_start_index, should_append_to_pretty_printed);
        }
    }

    fn start_new_list(&mut self) {
        let sexp_commented_out = self.consume_pending_sexp_comment();
        let list_metadata = ListMetadata {
            list_kind: ListKind::Plain,
            last_child_index: OptNodeIndex::NONE,
            list_end_index: OptNodeIndex::NONE,
            sexp_commented_out,
            data_length: 0,
            contains_non_data: false,
        };

        let data_range = self.pretty_printed.start_list(sexp_commented_out);
        let new_node_index =
            self.push_new_document_node(DocumentToken::StartOfList(list_metadata), data_range);

        self.starts_of_unterminated_lists.push(new_node_index);
    }

    fn complete_list(&mut self) {
        let mut num_unused_sexp_comments = 0;
        let current_depth = self.starts_of_unterminated_lists.len();
        while let Some(depth) = self.depths_of_pending_sexp_comments.last() {
            if *depth >= current_depth {
                self.depths_of_pending_sexp_comments.pop();
                num_unused_sexp_comments += 1;
            } else {
                break;
            }
        }

        if num_unused_sexp_comments > 0 {
            let message = if num_unused_sexp_comments == 1 {
                format!("Saw unexpected ')' while sexp comment \"#;\" pending")
            } else {
                format!("Saw unexpected ')' while {num_unused_sexp_comments} sexp comments \"#;\" pending")
            };

            self.push_new_error_node(ErrorMetadata { message });
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

        if list_metadata.last_child_index.is_none() {
            // If no data in previous list, replace it with `Unit`, instead of an actual list.
            let sexp_commented_out = list_metadata.sexp_commented_out;
            let curr_node = &mut self.all_nodes[list_start_index.0];
            curr_node.token = DocumentToken::Unit { sexp_commented_out };

            let unit_start = self.pretty_printed.len() - 1;
            let _end_list_range = self.pretty_printed.end_list();
            curr_node.data_range = unit_start..(unit_start + 2);

            // If we have no parent, then we just completed a top-level node.
            if curr_node.parent_index.is_none() {
                self.complete_top_level_node();
            }

            return;
        };

        let should_append_to_pretty_printed = true;
        self.complete_single_list(list_start_index, should_append_to_pretty_printed);
    }

    fn complete_single_list(
        &mut self,
        list_start_index: NodeIndex,
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
        list_metadata.list_end_index = Some(list_end_index).into();
        list_metadata.list_kind = list_kind;

        let end_of_list_document_node = DocumentNode {
            parent_index: list_start_node.parent_index,
            prev_sibling: list_start_node.prev_sibling,
            next_sibling: list_start_node.next_sibling,
            data_index_in_parent: None,
            data_range,
            token: DocumentToken::EndOfList(EndOfListMetadata { list_start_index }),
        };

        self.all_nodes.push(end_of_list_document_node);

        if self.starts_of_unterminated_lists.is_empty() {
            self.complete_top_level_node();
        }
    }

    fn analyze_list(&mut self, list_start_index: NodeIndex) -> ListKind {
        let mut first_elem_atom_kind = None;
        let mut first_elem_is_record_field = false;
        let mut second_elem_atom_kind = None;
        let mut second_elem_node_index = None;
        let mut all_elems_after_first_are_record_fields = true;
        let mut all_elems_are_constructors = true;
        let mut list_contains_non_data_before_first_elem = false;
        let mut list_contains_non_data = false;

        let mut next_child_index = Some(list_start_index + 1);
        let mut list_length_including_commented_out_sexps = 0;
        let mut list_length_not_including_commented_out_sexps = 0;

        while let Some(node_index) = next_child_index {
            let node = &self.node(node_index);
            next_child_index = node.next_sibling.into();

            if !node.token.is_data() {
                list_contains_non_data = true;
                if list_length_including_commented_out_sexps == 0 {
                    list_contains_non_data_before_first_elem = true;
                }
                continue;
            }

            let atom_kind = node.token.atom_kind();

            list_length_including_commented_out_sexps += 1;

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

            if !node.token.is_sexp_commented_out() {
                list_length_not_including_commented_out_sexps += 1;

                if list_length_including_commented_out_sexps == 1 {
                    invariants::variant_constructors_are_not_sexp_commented_out();
                    first_elem_atom_kind = atom_kind;
                }

                if list_length_including_commented_out_sexps == 2 {
                    second_elem_atom_kind = atom_kind;
                }
            }

            // This is a heuristic so that we don't consider a list of enums,
            // e.g. "(Monday Tuesday Wednesday Thursday Friday)", a variant
            // tuple.
            all_elems_are_constructors =
                all_elems_are_constructors && matches!(atom_kind, Some(AtomKind::Constructor));

            // To check for record and record variants, we separately track whether the first
            // element is a record, and whether everything after it is.
            if list_length_including_commented_out_sexps == 1 {
                first_elem_is_record_field =
                    matches!(node.token.list_kind(), Some(ListKind::RecordField));
            } else {
                all_elems_after_first_are_record_fields = all_elems_after_first_are_record_fields
                    && matches!(node.token.list_kind(), Some(ListKind::RecordField));
            }

            if list_length_including_commented_out_sexps == 2 {
                second_elem_node_index = Some(node_index);
            }
        }

        let first_elem_is_constructor = matches!(first_elem_atom_kind, Some(AtomKind::Constructor));
        let first_elem_is_record_key = matches!(first_elem_atom_kind, Some(AtomKind::RecordKey));

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
            // first_elem_is_atom_kind would be None if the first elem is commented out, and thus
            // first_elem_is_constructor would be false.
            invariants::variant_constructors_are_not_sexp_commented_out();
            invariants::variants_have_at_least_one_non_sexp_commented_out_argument();
            ListKind::VariantRecord
        } else if !list_contains_non_data_before_first_elem
            && first_elem_is_constructor
            && list_length_not_including_commented_out_sexps > 1
            && !all_elems_are_constructors
        {
            invariants::constructors_are_the_first_child_of_variants();
            invariants::variants_have_at_least_one_non_sexp_commented_out_argument();
            ListKind::VariantTuple
        } else if first_two_elems_are_date_time
            && list_length_including_commented_out_sexps == 2
            && list_length_not_including_commented_out_sexps == 2
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
            invariants::record_fields_do_not_contain_commented_out_sexps();

            let Some(value_index) = second_elem_node_index else {
                panic!("Making a RecordField but have no `second_elem_node_index`.");
            };

            // If the value is a RecordField, unmark it as such, and mark
            // its key as a plain atom.
            let token = self.token_mut(value_index);
            if let Some(ListKind::RecordField) = token.list_kind() {
                let DocumentToken::StartOfList(list_metadata) = token else {
                    unreachable!();
                };
                list_metadata.list_kind = ListKind::Plain;
                self.set_atom_kind_of_first_list_elem_to_plain(value_index);
            }

            ListKind::RecordField
        } else if list_length_including_commented_out_sexps == 1
            && list_length_not_including_commented_out_sexps == 1
            && !list_contains_non_data
        {
            invariants::singleton_values_are_not_commented_out_sexps();

            if first_elem_is_record_key {
                // We're not making this a RecordField, so update it to a plain atom.
                self.set_atom_kind_of_first_list_elem_to_plain(list_start_index);
            }

            ListKind::Singleton
        } else {
            if first_elem_is_record_key {
                // We're not making this a RecordField, so update it to a plain atom.
                self.set_atom_kind_of_first_list_elem_to_plain(list_start_index);
            }

            ListKind::Plain
        }
    }

    fn set_atom_kind_of_first_list_elem_to_plain(&mut self, list_start_index: NodeIndex) {
        let mut next_child_index = Some(list_start_index + 1);
        while let Some(index) = next_child_index {
            let node = self.node_mut(index);
            if matches!(node.data_index_in_parent, Some(0)) {
                let DocumentToken::Atom(atom_metadata) = &mut node.token else {
                    panic!("First data child of list elem was not an atom");
                };
                debug_assert_eq!(atom_metadata.atom_kind, AtomKind::RecordKey);
                atom_metadata.atom_kind = AtomKind::Plain;
                return;
            }
            next_child_index = self.node(index).next_sibling();
        }
    }

    fn add_atom(&mut self, serialized_atom: Ref<'_, '_, PlausibleSerializedAtom>) {
        let sexp_commented_out = self.consume_pending_sexp_comment();

        let (atom_kind, atom_data) =
            match serialized_atom.unescape(&mut self.scratch_buffer_for_unescaping_atoms) {
                Ok(atom_data) => {
                    let atom_data = match atom_data {
                        Ref::Borrowed(atom_data) | Ref::Transient(atom_data) => atom_data,
                    };
                    let atom_kind = Self::classify_atom_kind(atom_data);

                    (atom_kind, Some(atom_data))
                }
                Err(err) => {
                    self.push_new_error_node(ErrorMetadata {
                        message: format!("Unable to unescape atom: {:?}", err),
                    });

                    let atom_kind = AtomKind::Plain;

                    (atom_kind, None)
                }
            };

        let data_range = if let Some(atom_data) = atom_data {
            self.pretty_printed
                .write_atom(atom_data, sexp_commented_out)
        } else {
            self.pretty_printed
                .write_malformed_atom(serialized_atom.bytes(), sexp_commented_out)
        };

        let quoted = matches!(&self.pretty_printed.data()[data_range.start], &b'"');
        let valid = atom_data.is_some();

        let atom_metadata = AtomMetadata {
            atom_kind,
            sexp_commented_out,
            quoted,
            valid,
        };

        let atom_index =
            self.push_new_document_node(DocumentToken::Atom(atom_metadata), data_range);

        // If we classified the node as a `RecordKey`, but it's not the first child
        // of a list (or it's a top-level node), update it to just be a plain atom.
        if atom_kind == AtomKind::RecordKey {
            let node = self.node_mut(atom_index);
            if node.parent_index.is_none() || !matches!(node.data_index_in_parent, Some(0)) {
                let DocumentToken::Atom(atom_metadata) = &mut node.token else {
                    // We know we just pushed an atom node.
                    unreachable!()
                };
                atom_metadata.atom_kind = AtomKind::Plain;
            }
        }
    }

    fn classify_atom_kind(atom: &AtomData) -> AtomKind {
        let bytes = atom.bytes();

        if ATOM_BOOL_RE.is_match(bytes) {
            AtomKind::Bool
        } else if ATOM_RECORD_KEY_RE.is_match(bytes) {
            AtomKind::RecordKey
        } else if ATOM_CONSTRUCTOR_RE.is_match(bytes) {
            AtomKind::Constructor
        } else if ATOM_INTEGER_RE.is_match(bytes) || ATOM_FLOAT_RE.is_match(bytes) {
            AtomKind::Number
        } else if bytes.len() == ATOM_DATE_LENGTH && ATOM_DATE_RE.is_match(bytes) {
            AtomKind::Date
        } else if ATOM_TIME_RE.is_match(bytes) {
            AtomKind::Time
        } else {
            AtomKind::Plain
        }
    }

    fn track_sexp_comment(&mut self) {
        let current_depth = self.starts_of_unterminated_lists.len();
        self.depths_of_pending_sexp_comments.push(current_depth);
    }

    // Returns true if it did consume a pending sexp comment.
    fn consume_pending_sexp_comment(&mut self) -> bool {
        let current_depth = self.starts_of_unterminated_lists.len();
        match self.depths_of_pending_sexp_comments.last() {
            None => false,
            Some(depth) => {
                if *depth == current_depth {
                    self.depths_of_pending_sexp_comments.pop();
                    true
                } else {
                    false
                }
            }
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

    pub fn sexp_get_style_path_to_node(&self, node_index: NodeIndex) -> Option<String> {
        let mut s = String::new();

        // Treat end-of-list as start-of-list
        let node_index = match self.token(node_index) {
            DocumentToken::EndOfList(metadata) => metadata.list_start_index,
            _ => node_index,
        };

        // Move off of comments or errors to their parent nodes.
        let mut data_index = Some(node_index);
        while let Some(index) = data_index {
            if self.token(index).is_data() {
                break;
            }

            data_index = self.node(index).parent_index();
        }

        // If we're at a top level comment, don't show any path at all.
        let node_index = data_index?;

        let parent_index = self.node(node_index).parent_index();

        // When we're focused on a top-level node, show "." if it's the only node,
        // otherwise show "[n]". (The recursive version doesn't track what depth
        // our desired node is at to determine whether a lower level will add the '.'
        // as needed.)
        if parent_index.is_none() && self.num_top_level_data_nodes == 1 {
            s.push('.');
            return Some(s);
        }

        let _ = self.rec_sexp_get_style_path_to_node(&mut s, parent_index, node_index);

        Some(s)
    }

    fn rec_sexp_get_style_path_to_node(
        &self,
        buf: &mut String,
        parent_index: Option<NodeIndex>,
        child_index: NodeIndex,
    ) -> bool {
        let Some(parent_index) = parent_index else {
            if self.num_top_level_data_nodes > 1 {
                // sexp-commented out nodes won't have a `data_index_in_parent`.
                if let Some(index) = self.node(child_index).data_index_in_parent {
                    let _ = write!(buf, "[{index}]");
                } else {
                    let _ = write!(buf, "[_]");
                }
            }

            return true;
        };

        let should_write_path_from_parent_to_child = self.rec_sexp_get_style_path_to_node(
            buf,
            self.node(parent_index).parent_index(),
            parent_index,
        );

        if should_write_path_from_parent_to_child {
            let child_index_in_parent = self.node(child_index).data_index_in_parent;

            let list_kind = self
                .token(parent_index)
                .list_kind()
                .expect("can't have child if parent isn't a list");

            let try_field_accessor = match list_kind {
                ListKind::Record => true,
                ListKind::VariantRecord => {
                    match child_index_in_parent {
                        Some(index) => index != 0,
                        None => {
                            // If we don't have a child index, we know we're not the constructor,
                            // so we can try the field accessor.
                            invariants::constructors_are_the_first_child_of_variants();
                            invariants::variant_constructors_are_not_sexp_commented_out();
                            true
                        }
                    }
                }
                _ => false,
            };

            // Try using ".foo" syntax for record fields in records and variant records.
            if try_field_accessor {
                invariants::record_keys_are_the_first_child_of_record_fields();
                let name = &self.pretty_printed[self.node(child_index + 1).data_range.clone()];

                invariants::record_keys_always_match_record_key_regex();
                // Should always be ok
                if let Ok(name) = std::str::from_utf8(name) {
                    let _ = write!(buf, ".{name}");
                    return false;
                }
            }

            let child_index_in_parent = match child_index_in_parent {
                Some(index) => index,
                None => {
                    if self.token(child_index).is_sexp_commented_out() {
                        let _ = write!(buf, ".[_]");
                        return true;
                    }
                    panic!("child_index should be a data node");
                }
            };

            // Try writing ".Var[1]" for variant tuple access
            if matches!(list_kind, ListKind::VariantTuple) && child_index_in_parent > 0 {
                invariants::constructors_are_the_first_child_of_variants();
                let constructor =
                    &self.pretty_printed[self.node(parent_index + 1).data_range.clone()];

                // Should always be ok
                if let Ok(constructor) = std::str::from_utf8(constructor) {
                    let _ = write!(buf, ".{constructor}[{}]", child_index_in_parent - 1);
                    // This doesn't actually shorten the path! It just makes it more precise.
                    return true;
                }
            }

            let _ = write!(buf, ".[{child_index_in_parent}]");
            true
        } else {
            true
        }
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

    use crate::sexp::layout::tests::layout_and_show_logical_lines;

    use bstr::ByteSlice;
    use insta::assert_snapshot;

    #[test]
    fn show_core_struct_sizes() {
        use std::mem;

        assert_snapshot!(mem::size_of::<DocumentNode>(), @"88");
        assert_snapshot!(mem::align_of::<DocumentNode>(), @"8");

        assert_snapshot!(mem::offset_of!(DocumentNode, parent_index),         @"48");
        assert_snapshot!(mem::offset_of!(DocumentNode, prev_sibling),         @"56");
        assert_snapshot!(mem::offset_of!(DocumentNode, next_sibling),         @"64");
        assert_snapshot!(mem::offset_of!(DocumentNode, data_index_in_parent), @"0");
        assert_snapshot!(mem::offset_of!(DocumentNode, data_range),           @"72");
        assert_snapshot!(mem::offset_of!(DocumentNode, token),                @"16");

        assert_snapshot!(mem::size_of::<ListMetadata>(), @"32");
        assert_snapshot!(mem::align_of::<ListMetadata>(), @"8");

        assert_snapshot!(mem::offset_of!(ListMetadata, list_kind), @"24");
        assert_snapshot!(mem::offset_of!(ListMetadata, last_child_index), @"0");
        assert_snapshot!(mem::offset_of!(ListMetadata, list_end_index), @"8");
        assert_snapshot!(mem::offset_of!(ListMetadata, sexp_commented_out), @"25");
        assert_snapshot!(mem::offset_of!(ListMetadata, data_length), @"16");
        assert_snapshot!(mem::offset_of!(ListMetadata, contains_non_data), @"26");
    }

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

            let parent_index = parent_index.to_option();
            let prev_sibling = prev_sibling.to_option();
            let next_sibling = next_sibling.to_option();

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
                if token.is_sexp_commented_out() {
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
        let doc = DocCore::from_bytes(bytes, true);
        dump_doc(&doc)
    }

    fn raw_token_atom(bytes: &'static [u8]) -> RawToken<'static, 'static> {
        RawToken::Atom(Ref::Borrowed(PlausibleSerializedAtom::new(bytes).unwrap()))
    }

    #[test]
    fn test_incrementally_build_up_sexps() {
        let mut doc = DocCore::new();
        assert_snapshot!(dump_doc(&doc), @"Raw document:");

        doc.append_raw_token(RawToken::LeftParen);
        doc.append_raw_token(raw_token_atom(b"atom"));

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
        doc.append_raw_token(raw_token_atom(b"atom"));
        doc.append_raw_token(RawToken::RightParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom)

        0   0..1     <-- ^--[0 ] --> StartOfList(Singleton)   : "("
        1   1..5     <-- ^ 0[0 ] --> Atom(Plain)              : "atom"
        2   5..6     <-- ^--[--] --> EndOfList                : ")"
        "#);

        doc.append_raw_token(RawToken::LeftParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom)
        (

        0   0..1     <-- ^--[0 ] --> StartOfList(Singleton)   : "("
        1   1..5     <-- ^ 0[0 ] --> Atom(Plain)              : "atom"
        2   5..6     <-- ^--[--] --> EndOfList                : ")"
        3   7..8     <-- ^--[1 ] --> StartOfList(Plain)       : "("
        "#);

        doc.append_raw_token(RawToken::RightParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (atom)
        ()

        0   0..1     <-- ^--[0 ]  3> StartOfList(Singleton)   : "("
        1   1..5     <-- ^ 0[0 ] --> Atom(Plain)              : "atom"
        2   5..6     <-- ^--[--] --> EndOfList                : ")"
        3   7..9     <0  ^--[1 ] --> Unit                     : "()"
        "#);
    }

    #[test]
    fn test_connect_top_level_nodes_when_one_is_unit() {
        let mut doc = DocCore::new();
        doc.append_raw_token(raw_token_atom(b"atom"));
        doc.append_raw_token(RawToken::LeftParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        atom
        (

        0   0..4     <-- ^--[0 ] --> Atom(Plain)              : "atom"
        1   5..6     <-- ^--[1 ] --> StartOfList(Plain)       : "("
        "#);

        doc.append_raw_token(RawToken::RightParen);

        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        atom
        ()

        0   0..4     <-- ^--[0 ]  1> Atom(Plain)              : "atom"
        1   5..7     <0  ^--[1 ] --> Unit                     : "()"
        "#);
    }

    #[test]
    fn test_data_index_in_parent_ignores_sexp_comments() {
        // In top-level nodes
        let doc = dump(br#"im_0 #; ignore_me im_1"#);
        // BUG: NodeIndex(2) should be at index [1].
        assert_snapshot!(&doc, @r#"
        Raw document:
        im_0
        #; ignore_me
        im_1

        0   0..4     <-- ^--[0 ]  1> Atom(Plain)              : "im_0"
        1   8..17    <0  ^--[#;]  2> Atom(Plain)              : "ignore_me"
        2   18..22   <1  ^--[2 ] --> Atom(Plain)              : "im_1"
        "#);

        // Inside lists
        let doc = dump(br#"(im_0 #; ignore_me im_1)"#);
        assert_snapshot!(&doc, @r#"
        Raw document:
        (im_0 #; ignore_me im_1)

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..5     <-- ^ 0[0 ]  2> Atom(Plain)              : "im_0"
        2   9..18    <1  ^ 0[#;]  3> Atom(Plain)              : "ignore_me"
        3   19..23   <2  ^ 0[1 ] --> Atom(Plain)              : "im_1"
        4   23..24   <-- ^--[--] --> EndOfList                : ")"
        "#);
    }

    #[test]
    fn test_normalize_sexp_comment_locations() {
        let doc = dump(br#"#; #; (1 #; 2 3) #; 4 5 6"#);
        assert_snapshot!(&doc, @r#"
        Raw document:
        #; (1 #; 2 3)
        #; 4
        #; 5
        6

        0   3..4     <-- ^--[#;]  5> StartOfList(Plain)       : "("
        1   4..5     <-- ^ 0[0 ]  2> Atom(Number)             : "1"
        2   9..10    <1  ^ 0[#;]  3> Atom(Number)             : "2"
        3   11..12   <2  ^ 0[1 ] --> Atom(Number)             : "3"
        4   12..13   <-- ^--[--] --> EndOfList                : ")"
        5   17..18   <0  ^--[#;]  6> Atom(Number)             : "4"
        6   22..23   <5  ^--[#;]  7> Atom(Number)             : "5"
        7   24..25   <6  ^--[3 ] --> Atom(Number)             : "6"
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
        3   27..37   <2  ^ 0[2 ]  4> Atom(Plain)              : "record_key"
        4   38..45   <3  ^ 0[3 ]  5> Atom(Number)             : "123_456"
        5   46..53   <4  ^ 0[4 ]  6> Atom(Number)             : "7.89e10"
        6   54..58   <5  ^ 0[5 ]  7> Atom(Bool)               : "true"
        7   59..64   <6  ^ 0[6 ]  8> Atom(Bool)               : "false"
        8   65..75   <7  ^ 0[7 ]  9> Atom(Date)               : "2021-07-20"
        9   76..94   <8  ^ 0[8 ] --> Atom(Time)               : "22:42:32.000000000"
        10  94..95   <-- ^--[--] --> EndOfList                : ")"
        "#);

        // Time atoms can have exactly 0, 3, 6, or 9 decimals.
        let doc = dump(br#"(09:30:00 12:59:59.1234 16:00:00.000 20:00:00.000000)"#);

        assert_snapshot!(&doc, @r#"
        Raw document:
        (09:30:00 12:59:59.1234 16:00:00.000 20:00:00.000000)

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..9     <-- ^ 0[0 ]  2> Atom(Time)               : "09:30:00"
        2   10..23   <1  ^ 0[1 ]  3> Atom(Plain)              : "12:59:59.1234"
        3   24..36   <2  ^ 0[2 ]  4> Atom(Time)               : "16:00:00.000"
        4   37..52   <3  ^ 0[3 ] --> Atom(Time)               : "20:00:00.000000"
        5   52..53   <-- ^--[--] --> EndOfList                : ")"
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
        3   7..8     <2  ^ 1[1 ] --> Atom(Plain)              : "a"
        4   8..9     <-- ^ 0[--] --> EndOfList                : ")"
        5   10..11   <1  ^ 0[1 ] --> StartOfList(RecordField) : "("
        6   11..15   <-- ^ 5[0 ]  7> Atom(RecordKey)          : "key2"
        7   16..17   <6  ^ 5[1 ] --> Atom(Plain)              : "b"
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
        3   16..20   <2  ^ 1[0 ]  4> Atom(Plain)              : "key2"
        4   21..22   <3  ^ 1[1 ] --> Atom(Plain)              : "b"
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
        4   19..20   <3  ^ 2[1 ] --> Atom(Plain)              : "a"
        5   20..21   <1  ^ 0[--] --> EndOfList                : ")"
        6   22..23   <2  ^ 0[2 ] --> StartOfList(RecordField) : "("
        7   23..27   <-- ^ 6[0 ]  8> Atom(RecordKey)          : "key2"
        8   28..29   <7  ^ 6[1 ] --> Atom(Plain)              : "b"
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
        ()

        0   0..2     <-- ^--[0 ]  1> Unit                     : "()"
        1   3..4     <0  ^--[1 ]  4> StartOfList(Plain)       : "("
        2   4..13    <-- ^ 1[--] --> BlockComment             : "#| one |#"
        3   13..14   <0  ^--[--] --> EndOfList                : ")"
        4   15..16   <1  ^--[2 ] --> StartOfList(Plain)       : "("
        5   16..16   <-- ^ 4[--] --> Error: Saw unexpected ')' while sexp comment "#;" pending
        6   16..17   <1  ^--[--] --> EndOfList                : ")"
        "##);
    }

    #[test]
    fn test_record_keys_only_appear_in_record_fields() {
        let not_top_level = dump(b"not_key1");
        assert_snapshot!(&not_top_level, @r#"
        Raw document:
        not_key1

        0   0..8     <-- ^--[0 ] --> Atom(Plain)              : "not_key1"
        "#);

        let not_variant_tuple_value = dump(b"(Variant not_key)");
        assert_snapshot!(&not_variant_tuple_value, @r#"
        Raw document:
        (Variant not_key)

        0   0..1     <-- ^--[0 ] --> StartOfList(VariantTuple): "("
        1   1..8     <-- ^ 0[0 ]  2> Atom(Constructor)        : "Variant"
        2   9..16    <1  ^ 0[1 ] --> Atom(Plain)              : "not_key"
        3   16..17   <-- ^--[--] --> EndOfList                : ")"
        "#);

        let not_value_of_record_value = dump(b"((key not_key))");
        assert_snapshot!(&not_value_of_record_value , @r#"
        Raw document:
        ((key not_key))

        0   0..1     <-- ^--[0 ] --> StartOfList(Record)      : "("
        1   1..2     <-- ^ 0[0 ] --> StartOfList(RecordField) : "("
        2   2..5     <-- ^ 1[0 ]  3> Atom(RecordKey)          : "key"
        3   6..13    <2  ^ 1[1 ] --> Atom(Plain)              : "not_key"
        4   13..14   <-- ^ 0[--] --> EndOfList                : ")"
        5   14..15   <-- ^--[--] --> EndOfList                : ")"
        "#);

        let not_value_in_singleton = dump(b"(not_key)");
        assert_snapshot!(&not_value_in_singleton , @r#"
        Raw document:
        (not_key)

        0   0..1     <-- ^--[0 ] --> StartOfList(Singleton)   : "("
        1   1..8     <-- ^ 0[0 ] --> Atom(Plain)              : "not_key"
        2   8..9     <-- ^--[--] --> EndOfList                : ")"
        "#);

        let not_in_list_with_more_than_two_values = dump(b"(not_key 1 2)");
        assert_snapshot!(&not_in_list_with_more_than_two_values , @r#"
        Raw document:
        (not_key 1 2)

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..8     <-- ^ 0[0 ]  2> Atom(Plain)              : "not_key"
        2   9..10    <1  ^ 0[1 ]  3> Atom(Number)             : "1"
        3   11..12   <2  ^ 0[2 ] --> Atom(Number)             : "2"
        4   12..13   <-- ^--[--] --> EndOfList                : ")"
        "#);

        invariants::record_keys_are_the_first_child_of_record_fields();
        let not_if_comment_before_key = dump(b"((#| comment |# not_key 1)");
        assert_snapshot!(&not_if_comment_before_key , @r##"
        Raw document:
        ((#| comment |# not_key 1)

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..2     <-- ^ 0[0 ]  6> StartOfList(Plain)       : "("
        2   2..15    <-- ^ 1[--]  3> BlockComment             : "#| comment |#"
        3   16..23   <2  ^ 1[0 ]  4> Atom(Plain)              : "not_key"
        4   24..25   <3  ^ 1[1 ] --> Atom(Number)             : "1"
        5   25..26   <-- ^ 0[--] --> EndOfList                : ")"
        6   26..26   <1  ^ 0[--] --> Error: Unexpected EOF while parsing list
        7   26..26   <-- ^--[--] --> EndOfList                : ""
        "##);
    }

    #[test]
    fn test_record_field_values_cant_be_record_fields() {
        let record_field_value_not_record_field = dump(b"((a 1) (b (c d)))");
        assert_snapshot!(&record_field_value_not_record_field, @r#"
        Raw document:
        ((a 1) (b (c d)))

        0   0..1     <-- ^--[0 ] --> StartOfList(Record)      : "("
        1   1..2     <-- ^ 0[0 ]  5> StartOfList(RecordField) : "("
        2   2..3     <-- ^ 1[0 ]  3> Atom(RecordKey)          : "a"
        3   4..5     <2  ^ 1[1 ] --> Atom(Number)             : "1"
        4   5..6     <-- ^ 0[--] --> EndOfList                : ")"
        5   7..8     <1  ^ 0[1 ] --> StartOfList(RecordField) : "("
        6   8..9     <-- ^ 5[0 ]  7> Atom(RecordKey)          : "b"
        7   10..11   <6  ^ 5[1 ] --> StartOfList(Plain)       : "("
        8   11..12   <-- ^ 7[0 ]  9> Atom(Plain)              : "c"
        9   13..14   <8  ^ 7[1 ] --> Atom(Plain)              : "d"
        10  14..15   <6  ^ 5[--] --> EndOfList                : ")"
        11  15..16   <1  ^ 0[--] --> EndOfList                : ")"
        12  16..17   <-- ^--[--] --> EndOfList                : ")"
        "#);
    }

    #[test]
    fn test_basic_errors() {
        let unmatched_closing_paren = dump(b"one )");
        assert_snapshot!(unmatched_closing_paren, @r#"
        Raw document:
        one

        0   0..3     <-- ^--[0 ]  1> Atom(Plain)              : "one"
        1   3..3     <0  ^--[--] --> Error: Saw unexpected ')' while parsing top-level sexp
        "#);

        let pending_sexp_comment_at_eof = dump(b"a #;");
        assert_snapshot!(pending_sexp_comment_at_eof, @r##"
        Raw document:
        a

        0   0..1     <-- ^--[0 ]  1> Atom(Plain)              : "a"
        1   1..1     <0  ^--[--] --> Error: Unexpected EOF while sexp comment "#;" pending
        "##);

        let pending_sexp_comment_in_list_at_eof = dump(b"a (#;");
        assert_snapshot!(pending_sexp_comment_in_list_at_eof, @r##"
        Raw document:
        a
        (

        0   0..1     <-- ^--[0 ]  1> Atom(Plain)              : "a"
        1   2..3     <0  ^--[1 ] --> StartOfList(Plain)       : "("
        2   3..3     <-- ^ 1[--]  3> Error: Unexpected EOF while sexp comment "#;" pending
        3   3..3     <2  ^ 1[--] --> Error: Unexpected EOF while parsing list
        4   3..3     <0  ^--[--] --> EndOfList                : ""
        "##);

        let pending_sexp_comment_at_end_of_list = dump(b"a (1 #;)");
        assert_snapshot!(pending_sexp_comment_at_end_of_list, @r##"
        Raw document:
        a
        (1)

        0   0..1     <-- ^--[0 ]  1> Atom(Plain)              : "a"
        1   2..3     <0  ^--[1 ] --> StartOfList(Plain)       : "("
        2   3..4     <-- ^ 1[0 ]  3> Atom(Number)             : "1"
        3   4..4     <2  ^ 1[--] --> Error: Saw unexpected ')' while sexp comment "#;" pending
        4   4..5     <0  ^--[--] --> EndOfList                : ")"
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
        4   6..7     <3  ^ 2[1 ]  5> Atom(Plain)              : "z"
        5   7..7     <4  ^ 2[--] --> Error: Unexpected EOF while parsing list
        6   7..7     <-- ^ 1[--] --> EndOfList                : ""
        7   7..7     <0  ^--[--] --> EndOfList                : ""
        "#);
    }

    #[test]
    fn test_closest_node_to_byte_index() {
        let doc = DocCore::from_bytes(b"((one two) #; three ; four\n)", true);
        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        ((one two) #; three ; four
        )

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..2     <-- ^ 0[0 ]  5> StartOfList(RecordField) : "("
        2   2..5     <-- ^ 1[0 ]  3> Atom(RecordKey)          : "one"
        3   6..9     <2  ^ 1[1 ] --> Atom(Plain)              : "two"
        4   9..10    <-- ^ 0[--] --> EndOfList                : ")"
        5   14..19   <1  ^ 0[#;]  6> Atom(Plain)              : "three"
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

        let doc = DocCore::from_bytes(b"(a", true);
        assert_snapshot!(dump_doc(&doc), @r#"
        Raw document:
        (a

        0   0..1     <-- ^--[0 ] --> StartOfList(Plain)       : "("
        1   1..2     <-- ^ 0[0 ]  2> Atom(Plain)              : "a"
        2   2..2     <1  ^ 0[--] --> Error: Unexpected EOF while parsing list
        3   2..2     <-- ^--[--] --> EndOfList                : ""
        "#);

        assert_eq!(doc.closest_node_to_byte_index(1), NodeIndex(1));
    }

    #[test]
    fn test_sexp_get_style_paths() {
        let mut doc = DocCore::from_bytes(
            b"(one (Two 2 too) ((a 1) (b 2) (c (d 3)))) (Var (a 1)",
            false,
        );
        assert_snapshot!(layout_and_show_logical_lines(&doc), @r"
         0..=1  : (one
         2..=3  :  (Two
         4..=4  :    2
         5..=6  :    too)
         7..=11 :  ((a 1)
        12..=15 :   (b 2)
        16..=18 :   (c (
        19..=19 :     d
        20..=24 :     3))))
        ");
        // 25..=30 : (Var (a 1))

        let path = |i| doc.sexp_get_style_path_to_node(NodeIndex(i)).unwrap();

        assert_snapshot!(path(0),  @".");
        assert_snapshot!(path(1),  @".[0]");
        assert_snapshot!(path(2),  @".[1]");
        // Path to end of list should be same as path to start of list
        assert_snapshot!(path(6),  @".[1]");

        // Paths to variant tuples
        assert_snapshot!(path(5),  @".[1].Two[1]");
        assert_snapshot!(path(3),  @".[1].[0]");

        assert_snapshot!(path(7),  @".[2]");

        // Path to record field and record value are the same
        assert_snapshot!(path(8),  @".[2].a");
        assert_snapshot!(path(10), @".[2].a");
        // Path to record key also uses the field name? This is maybe a little
        // weird; should maybe be updated in "micro" mode.
        assert_snapshot!(path(9),  @".[2].a");

        // This is a "fake" record field, since it's not in a record.
        assert_snapshot!(path(18), @".[2].c");
        assert_snapshot!(path(21), @".[2].c");

        // Now there are two top-level sexps
        doc.append_raw_token(RawToken::RightParen);

        let path = |i| doc.sexp_get_style_path_to_node(NodeIndex(i)).unwrap();

        assert_snapshot!(path(0),  @"[0]");
        assert_snapshot!(path(2),  @"[0].[1]");
        assert_snapshot!(path(25),  @"[1]");

        // Variant records fields use field name accessors
        assert_snapshot!(path(27),  @"[1].a");
        // Constructors use regular indexes though
        assert_snapshot!(path(26),  @"[1].[0]");
    }

    #[test]
    fn test_sexp_get_style_paths_for_non_data_nodes() {
        let doc = DocCore::from_bytes(
            b"; comment\n((a 1) #| mid-record |# (b (#| mid-record-field |#)) #; (c (x #; (y z)))) (err",
            true,
        );
        assert_snapshot!(layout_and_show_logical_lines(&doc), @r"
         0..=0  : ; comment
         1..=5  : ((a 1)
         6..=6  :  #| mid-record |#
         7..=9  :  (b (
        10..=10 :    #| mid-record-field |#
        11..=12 :  ))
        13..=15 :  #; (c (
        16..=16 :       x
        17..=23 :       #; (y z))))
        24..=25 : (err
        26..=26 :  ERR: Unexpected EOF while parsing list
        27..=27 :
        ");

        let path = |i| {
            doc.sexp_get_style_path_to_node(NodeIndex(i))
                .unwrap_or("<none>".to_string())
        };

        // No path for a top-level comment
        assert_snapshot!(path(0),  @"<none>");

        // Comment doesn't count as data, so this is index 0
        assert_snapshot!(path(1),  @"[0]");

        // Comment in list
        assert_snapshot!(path(6),  @"[0]");

        // Comment in record field
        assert_snapshot!(path(10),  @"[0].b");

        // Comment in record field
        assert_snapshot!(path(10),  @"[0].b");

        // Path to commented out record field
        assert_snapshot!(path(13),  @"[0].c");

        // Path to value in commented out record field; still use field name
        assert_snapshot!(path(16),  @"[0].c.[0]");

        // Path to commented out value in commented out record field
        assert_snapshot!(path(17),  @"[0].c.[_]");

        // Path to value in commented out list in commented out record field
        assert_snapshot!(path(18),  @"[0].c.[_].[0]");

        // Path to error
        assert_snapshot!(path(24),  @"[1]");
    }

    #[test]
    fn test_sexp_get_style_paths_to_top_level_non_data_nodes() {
        fn path_to_single_top_level_node(input: &'static [u8]) -> String {
            let doc = DocCore::from_bytes(input, true);
            doc.sexp_get_style_path_to_node(NodeIndex(0))
                .unwrap_or("<none>".to_string())
        }

        // No path for top-level comments
        assert_snapshot!(path_to_single_top_level_node(b"; line comment\n"),  @"<none>");
        assert_snapshot!(path_to_single_top_level_node(b"#| block comment |#"),  @"<none>");
        assert_snapshot!(path_to_single_top_level_node(b"#; sexp_comment"),  @".");
        assert_snapshot!(path_to_single_top_level_node(b")"),  @"<none>");

        let doc = DocCore::from_bytes(
            b"; line comment\n#| block comment |# #; sexp_comment ) x",
            true,
        );
        assert_snapshot!(layout_and_show_logical_lines(&doc), @r"
        0..=0  : ; line comment
        1..=1  : #| block comment |#
        2..=2  : #; sexp_comment
        3..=3  : ERR: Saw unexpected ')' while parsing top-level sexp
        4..=4  : x
        ");

        let path = |i| {
            doc.sexp_get_style_path_to_node(NodeIndex(i))
                .unwrap_or("<none>".to_string())
        };

        assert_snapshot!(path(0),  @"<none>");
        assert_snapshot!(path(1),  @"<none>");
        assert_snapshot!(path(2),  @"[_]");
        assert_snapshot!(path(3),  @"<none>");
        // BUG: This should be [0], not [1].
        assert_snapshot!(path(4),  @"[1]");
    }
}
