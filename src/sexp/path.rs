use crate::sexp::core::{invariants, DocCore, DocumentToken, ListKind, NodeIndex};

/// A path to a data node (i.e., not a comment or error) within a document.
pub struct DataNodePath {
    pub root_index_in_doc: Option<usize>,
    data_index: NodeIndex,
    elems: Vec<DataNodePathElem>,
}

enum DataNodePathElem {
    Index {
        list_kind: ListKind,
        index: Option<usize>,
    },
    RecordField {
        name: NodeIndex,
    },
    VariantIndex {
        constructor: NodeIndex,
        index: Option<usize>,
    },
    VariantField {
        constructor: NodeIndex,
        name: NodeIndex,
    },
}

impl DataNodePath {
    // If `node_index` points to a non-data node, this will return a path to
    // its parent.
    pub fn build(doc: &DocCore, node_index: NodeIndex) -> Option<DataNodePath> {
        let data_index = Self::data_node_to_create_path_to(doc, node_index)?;
        let mut elems = vec![];
        let root_index_in_doc = Self::rec_build_path_to_node(&mut elems, doc, data_index);

        Some(DataNodePath {
            root_index_in_doc,
            data_index,
            elems,
        })
    }

    // Get the actual data node we'll create a path to, given a destination node.
    // If that node is the end of a list, we'll create a path to the start of the
    // list. If that node is a comment or error, we'll create a path to its parent.
    fn data_node_to_create_path_to(doc: &DocCore, node_index: NodeIndex) -> Option<NodeIndex> {
        let node = doc.node(node_index);

        // Treat end-of-list as start-of-list
        if let DocumentToken::EndOfList(metadata) = &node.token {
            return Some(metadata.list_start_index);
        }

        // Move off of comments or errors to their parent nodes.
        if !node.token.is_data() {
            let parent_index = node.parent_index();

            // The parent should be a data node.
            debug_assert!(match parent_index {
                None => true,
                Some(parent_index) => doc.token(parent_index).is_data(),
            });

            return parent_index;
        }

        Some(node_index)
    }

    fn rec_build_path_to_node(
        elems: &mut Vec<DataNodePathElem>,
        doc: &DocCore,
        node_index: NodeIndex,
    ) -> Option<usize> {
        let node = doc.node(node_index);
        debug_assert!(node.token.is_data());

        let Some(parent_index) = node.parent_index() else {
            return node.data_index_in_parent;
        };

        let root_index_in_doc = Self::rec_build_path_to_node(elems, doc, parent_index);

        Self::extend_path_to_node(elems, doc, parent_index, node_index);

        root_index_in_doc
    }

    fn extend_path_to_node(
        elems: &mut Vec<DataNodePathElem>,
        doc: &DocCore,
        parent_index: NodeIndex,
        node_index: NodeIndex,
    ) {
        let node = doc.node(node_index);
        let parent = doc.node(parent_index);

        invariants::record_keys_are_the_first_child_of_record_fields();
        let name = node_index + 1;
        invariants::constructors_are_the_first_child_of_variants();
        let constructor = parent_index + 1;

        let list_kind = parent.token.list_kind().unwrap();
        match list_kind {
            // For these, we just use regular indexes:
            ListKind::Plain | ListKind::Singleton | ListKind::DateTime => {
                elems.push(DataNodePathElem::Index {
                    list_kind,
                    index: node.data_index_in_parent,
                });
            }
            ListKind::Record => {
                elems.push(DataNodePathElem::RecordField { name });
            }
            ListKind::VariantRecord => {
                // Handle path to the constructor itself.
                if matches!(node.data_index_in_parent, Some(0)) {
                    elems.push(DataNodePathElem::Index {
                        list_kind,
                        index: node.data_index_in_parent,
                    });
                } else {
                    elems.push(DataNodePathElem::VariantField { constructor, name });
                }
            }
            ListKind::VariantTuple => {
                // Handle path to the constructor itself.
                if matches!(node.data_index_in_parent, Some(0)) {
                    elems.push(DataNodePathElem::Index {
                        list_kind,
                        index: node.data_index_in_parent,
                    });
                } else {
                    elems.push(DataNodePathElem::VariantIndex {
                        constructor,
                        index: node.data_index_in_parent.map(|x| x - 1),
                    });
                }
            }
            // If the last path elem was a `RecordField` or `VariantField`, and we're
            // the value in the record field, don't do anything. But if we're the key,
            // then pop the record field, replace it with the index to the field, then
            // add an index 0. If the parent wasn't a record field, also just add the
            // index.
            ListKind::RecordField => {
                let node_is_value = node.data_index_in_parent == Some(1);
                let last_path_elem_is_field = matches!(
                    elems.last(),
                    Some(
                        DataNodePathElem::RecordField { .. }
                            | DataNodePathElem::VariantField { .. }
                    )
                );

                if last_path_elem_is_field {
                    if !node_is_value {
                        let parent_list_kind = match elems.pop() {
                            Some(DataNodePathElem::RecordField { .. }) => ListKind::Record,
                            Some(DataNodePathElem::VariantField { .. }) => ListKind::VariantRecord,
                            _ => unreachable!(),
                        };
                        elems.push(DataNodePathElem::Index {
                            list_kind: parent_list_kind,
                            index: parent.data_index_in_parent,
                        });
                        elems.push(DataNodePathElem::Index {
                            list_kind,
                            index: node.data_index_in_parent,
                        });
                    }
                } else {
                    elems.push(DataNodePathElem::Index {
                        list_kind,
                        index: node.data_index_in_parent,
                    });
                }
            }
        }
    }

    pub fn write_index<W: std::fmt::Write>(w: &mut W, index: Option<usize>) {
        let _ = match index {
            None => write!(w, "[_]"),
            Some(index) => write!(w, "[{index}]"),
        };
    }

    fn write_atom<W: std::fmt::Write>(w: &mut W, doc: &DocCore, node_index: NodeIndex) {
        // Should always be borrowed; we're only writing constructors and record keys.
        let atom = String::from_utf8_lossy(doc.raw_bytes_for_node(node_index));
        let _ = write!(w, "{}", atom);
    }

    pub fn format_for_status_bar(&self, doc: &DocCore) -> String {
        let mut s = String::new();

        // There are four cases, based on whether there is 1 or multiple top-level
        // data nodes, and whether the path is empty or not:
        //
        // 1 top-level data node,  empty path => .
        // 1 top-level data node,  path       => .path.to.[0].node
        // N top-level data nodes, empty path => [N]
        // N top-level data nodes, path       => [N].path.to.node

        if doc.num_top_level_data_nodes == 1 {
            if self.elems.is_empty() {
                s.push('.');
            } else {
                self.write_elems_as_sexp_get_style_path(&mut s, doc);
            }
        } else {
            Self::write_index(&mut s, self.root_index_in_doc);
            if !self.elems.is_empty() {
                self.write_elems_as_sexp_get_style_path(&mut s, doc);
            }
        }

        s
    }

    fn write_elems_as_sexp_get_style_path<W: std::fmt::Write>(&self, w: &mut W, doc: &DocCore) {
        for elem in self.elems.iter() {
            let _ = write!(w, ".");

            match elem {
                DataNodePathElem::Index { index, .. } => Self::write_index(w, *index),
                DataNodePathElem::VariantIndex { constructor, index } => {
                    Self::write_atom(w, doc, *constructor);
                    Self::write_index(w, *index);
                }
                DataNodePathElem::RecordField { name, .. }
                | DataNodePathElem::VariantField { name, .. } => {
                    Self::write_atom(w, doc, *name);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::fmt::Write;

    use crate::sexp::layout::tests::layout_and_show_logical_lines;
    use crate::sexp::state::DocState;

    use insta::assert_snapshot;

    fn show_path(doc: &DocCore, node_index: NodeIndex) -> String {
        let path = DataNodePath::build(doc, node_index);

        let Some(path) = path else {
            return "<no data node to build path to>".to_string();
        };

        let mut s = String::new();

        if path.data_index != node_index {
            let _ = writeln!(
                s,
                "(path to {:?} instead of #{node_index:?}",
                path.data_index
            );
        }

        if path.elems.is_empty() {
            s.push_str("<empty path>");
            return s;
        }

        for (i, path_elem) in path.elems.iter().enumerate() {
            if i == 0 {
                DataNodePath::write_index(&mut s, path.root_index_in_doc);
                s.push('.');
            } else {
                s.push_str("\n   .");
            }

            let _ = match path_elem {
                DataNodePathElem::Index { list_kind, index } => match index {
                    None => write!(s, "Index[_] in {list_kind:?}"),
                    Some(index) => write!(s, "Index[{index}] in {list_kind:?}"),
                },
                DataNodePathElem::RecordField { name } => {
                    let name = doc.raw_str_for_node(*name);
                    write!(s, "Field[{name}]")
                }
                DataNodePathElem::VariantIndex { constructor, index } => {
                    let constructor = doc.raw_str_for_node(*constructor);
                    match index {
                        None => write!(s, "VarIndex[{constructor}, _]"),
                        Some(index) => write!(s, "VarIndex[{constructor}, {index}]"),
                    }
                }
                DataNodePathElem::VariantField { constructor, name } => {
                    let constructor = doc.raw_str_for_node(*constructor);
                    let name = doc.raw_str_for_node(*name);
                    write!(s, "VarField[{constructor}, {name}]")
                }
            };
        }

        s
    }

    fn status_bar_path(doc: &DocCore, node_index: NodeIndex) -> String {
        let path = DataNodePath::build(doc, node_index);

        let Some(path) = path else {
            return "<no data node to build path to>".to_string();
        };

        path.format_for_status_bar(doc)
    }

    #[test]
    fn test_paths_to_elems() {
        let doc = DocCore::from_bytes(b"(one (Two 2 too) ((a 1) (b 2) (c (Var (d 3)))))", false);
        assert_snapshot!(layout_and_show_logical_lines(&doc), @r"
         0..=1  : (one
         2..=3  :  (Two
         4..=4  :    2
         5..=6  :    too)
         7..=11 :  ((a 1)
        12..=15 :   (b 2)
        16..=19 :   (c (Var
        20..=27 :     (d 3)))))
        ");

        let path = |i| show_path(&doc, NodeIndex(i));

        assert_snapshot!(path(0),  @"<empty path>");
        assert_snapshot!(path(1),  @"[0].Index[0] in Plain");
        assert_snapshot!(path(2),  @"[0].Index[1] in Plain");
        // Path to end of list should be same as path to start of list
        assert_snapshot!(path(6),  @r"
        (path to NodeIndex(2) instead of #NodeIndex(6)
        [0].Index[1] in Plain
        ");

        // Paths to variant tuples
        assert_snapshot!(path(5),  @r"
        [0].Index[1] in Plain
           .VarIndex[Two, 1]
        ");
        // Path to constructor uses a regular index
        assert_snapshot!(path(3),  @r"
        [0].Index[1] in Plain
           .Index[0] in VariantTuple
        ");

        assert_snapshot!(path(7),  @"[0].Index[2] in Plain");

        // Path to record field and record value are the same
        assert_snapshot!(path(12),  @r"
        [0].Index[2] in Plain
           .Field[b]
        ");
        assert_snapshot!(path(14), @r"
        [0].Index[2] in Plain
           .Field[b]
        ");
        // Path to record key uses two indexes into record and then into record field.
        assert_snapshot!(path(13),  @r"
        [0].Index[2] in Plain
           .Index[1] in Record
           .Index[0] in RecordField
        ");

        // Variant record field, path to field and value are the same
        assert_snapshot!(path(20), @r"
        [0].Index[2] in Plain
           .Field[c]
           .VarField[Var, d]
        ");
        assert_snapshot!(path(22), @r"
        [0].Index[2] in Plain
           .Field[c]
           .VarField[Var, d]
        ");
        // Key of variant record field
        assert_snapshot!(path(21), @r"
        [0].Index[2] in Plain
           .Field[c]
           .Index[1] in VariantRecord
           .Index[0] in RecordField
        ");
        // Variant record constructor
        assert_snapshot!(path(19), @r"
        [0].Index[2] in Plain
           .Field[c]
           .Index[0] in VariantRecord
        ");
    }

    #[test]
    fn test_paths_to_non_data_nodes() {
        let doc = DocCore::from_bytes(
            b"; comment\n((a 1) #| mid-record |# (b (#| mid-record-field |#)) #; (c (x #; (y z)))) ((err",
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
        24..=26 : ((err
        27..=27 :   ERR: Unexpected EOF while parsing list
        28..=29 :
        ");

        let path = |i| show_path(&doc, NodeIndex(i));

        // No path for a top-level comment
        assert_snapshot!(path(0),  @"<no data node to build path to>");

        // Top level node, so no path
        assert_snapshot!(path(1),  @"<empty path>");

        // Comment in list
        assert_snapshot!(path(6),  @r"
        (path to NodeIndex(1) instead of #NodeIndex(6)
        <empty path>
        ");

        // Comment in record field
        assert_snapshot!(path(10),  @r"
        (path to NodeIndex(9) instead of #NodeIndex(10)
        [0].Field[b]
        ");

        // Path to commented out record field
        assert_snapshot!(path(13),  @"[0].Field[c]");

        // Path to value in commented out record field; still use field name
        assert_snapshot!(path(16),  @r"
        [0].Field[c]
           .Index[0] in Plain
        ");

        // Path to commented out value in commented out record field
        assert_snapshot!(path(17),  @r"
        [0].Field[c]
           .Index[_] in Plain
        ");

        // Path to value in commented out list in commented out record field
        assert_snapshot!(path(18),  @r"
        [0].Field[c]
           .Index[_] in Plain
           .Index[0] in RecordField
        ");

        // Path to error
        assert_snapshot!(path(27),  @r"
        (path to NodeIndex(25) instead of #NodeIndex(27)
        [1].Index[0] in Singleton
        ");
    }

    #[test]
    fn test_status_bar_paths() {
        let mut doc = DocState::new_partial_doc_from_bytes(b"(1 2)");
        let path = |doc: &DocState, i| status_bar_path(&doc.core, NodeIndex(i));

        assert_snapshot!(path(&doc, 0), @".");
        assert_snapshot!(path(&doc, 1), @".[0]");

        // Block comments are not data nodes, so we still don't show top-level index.
        doc.append(b"#| abc |#");

        assert_snapshot!(path(&doc, 1), @".[0]");

        doc.append(b"((a 3) (b 4))");

        assert_snapshot!(path(&doc, 0), @"[0]");
        assert_snapshot!(path(&doc, 1), @"[0].[0]");
        assert_snapshot!(path(&doc, 5), @"[1]");
        assert_snapshot!(path(&doc, 6), @"[1].a");
    }

    #[test]
    fn test_status_bar_paths_to_top_level_non_data_nodes() {
        fn path_to_single_top_level_node(input: &'static [u8]) -> String {
            let doc = DocCore::from_bytes(input, true);
            status_bar_path(&doc, NodeIndex(0))
        }

        // No path for top-level comments
        assert_snapshot!(path_to_single_top_level_node(b"; line comment\n"),  @"<no data node to build path to>");
        assert_snapshot!(path_to_single_top_level_node(b"#| block comment |#"),  @"<no data node to build path to>");

        // sexp comments are like normal
        assert_snapshot!(path_to_single_top_level_node(b"#; sexp_comment"),  @".");

        // No path for top-level errors
        assert_snapshot!(path_to_single_top_level_node(b")"),  @"<no data node to build path to>");

        // Now test with multiple top-level nodes
        let doc = DocCore::from_bytes(
            b"; line comment\n#| block comment |# #; sexp_comment )",
            true,
        );

        assert_snapshot!(layout_and_show_logical_lines(&doc), @r"
        0..=0  : ; line comment
        1..=1  : #| block comment |#
        2..=2  : #; sexp_comment
        3..=3  : ERR: Saw unexpected ')' while parsing top-level sexp
        ");

        let path = |i| status_bar_path(&doc, NodeIndex(i));

        assert_snapshot!(path(0),  @"<no data node to build path to>");
        assert_snapshot!(path(1),  @"<no data node to build path to>");
        assert_snapshot!(path(2),  @".");
        assert_snapshot!(path(3),  @"<no data node to build path to>");

        // Now test with multiple top-level *data* nodes (same as above but with
        // an extra atom at the end).
        let doc = DocCore::from_bytes(
            b"; line comment\n#| block comment |# #; sexp_comment ) x",
            true,
        );

        let path = |i| status_bar_path(&doc, NodeIndex(i));

        assert_snapshot!(path(2),  @"[_]");
        assert_snapshot!(path(3),  @"<no data node to build path to>");
        assert_snapshot!(path(4),  @"[0]");
    }
}
