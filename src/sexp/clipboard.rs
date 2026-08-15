use std::io;

use crate::document::YankResult;
use crate::sexp::core::{invariants, AtomMetadata, DocumentToken, ListKind, NodeIndex};
use crate::sexp::layout;
use crate::sexp::state::DocState;

pub enum CopyTarget {
    PrettyPrintedValue,
    MachineValue,
    RecordField,
    MachineRecordField,
    Key,
    Constructor,
    String,
    QueryPath,
    GetPath,
}

impl CopyTarget {
    pub fn from_char(ch: char) -> Result<Self, String> {
        let copy_target = match ch {
            'y' => CopyTarget::PrettyPrintedValue,
            'm' => CopyTarget::MachineValue,
            't' | 'r' => CopyTarget::RecordField,
            'T' | 'R' => CopyTarget::MachineRecordField,
            'k' => CopyTarget::Key,
            'c' => CopyTarget::Constructor,
            's' => CopyTarget::String,
            'g' => CopyTarget::GetPath,
            'q' => CopyTarget::QueryPath,
            _ => return Err(format!("Unknown yank target {ch:?}")),
        };

        Ok(copy_target)
    }
}

fn yank_err(s: &'static str) -> io::Result<YankResult> {
    Ok(Err(s.to_string()))
}

pub fn yank_content<W: io::Write>(
    mut output: W,
    doc: &DocState,
    node_index: NodeIndex,
    target: CopyTarget,
) -> io::Result<YankResult> {
    let record_field_value_node_index = doc.core.value_of_record_field(node_index);
    let value_node_index = record_field_value_node_index.unwrap_or(node_index);

    match target {
        CopyTarget::PrettyPrintedValue => yank_pretty_printed_node(output, doc, value_node_index)?,
        CopyTarget::MachineValue => match doc.core.token(node_index) {
            DocumentToken::LineComment | DocumentToken::BlockComment => {
                return yank_err("Comments are not included in machine format");
            }
            DocumentToken::Error(_) => {
                return yank_err("Can't machine format an error");
            }
            _ => yank_machine_node(output, doc, value_node_index)?,
        },
        CopyTarget::RecordField | CopyTarget::MachineRecordField => {
            if matches!(
                doc.core.token(node_index).list_kind(),
                Some(ListKind::RecordField),
            ) {
                if matches!(target, CopyTarget::MachineRecordField) {
                    yank_machine_node(output, doc, node_index)?;
                } else {
                    yank_pretty_printed_node(output, doc, node_index)?;
                }
            } else {
                return yank_err("Can't yank record field; not focused on record field");
            }
        }
        CopyTarget::Key => {
            if matches!(
                doc.core.token(node_index).list_kind(),
                Some(ListKind::RecordField),
            ) {
                invariants::record_keys_are_the_first_child_of_record_fields();
                let key_node_index = node_index + 1;
                output.write_all(doc.core.raw_bytes_for_node(key_node_index))?;
            } else {
                return yank_err("Can't yank key; not focused on record field");
            }
        }
        CopyTarget::Constructor => {
            if matches!(
                doc.core.token(value_node_index).list_kind(),
                Some(ListKind::VariantRecord | ListKind::VariantTuple),
            ) {
                invariants::constructors_are_the_first_child_of_variants();
                let constructor_node_index = value_node_index + 1;
                output.write_all(doc.core.raw_bytes_for_node(constructor_node_index))?;
            } else {
                return yank_err("Can't yank key; not focused on variant");
            }
        }
        CopyTarget::String => match doc.core.token(value_node_index) {
            DocumentToken::Atom(AtomMetadata { quoted, valid, .. }) => {
                if *valid {
                    if !*quoted {
                        output.write_all(doc.core.raw_bytes_for_node(value_node_index))?;
                    } else {
                        return yank_err(
                            "unimplemented: Don't know how to unescape quoted values yet",
                        );
                    }
                } else {
                    return yank_err("Can't yank raw atom value; atom contains invalid escapes");
                }
            }
            _ => {
                return yank_err("Can't yank raw atom value; not focused on an atom");
            }
        },
        CopyTarget::QueryPath => return yank_err("unimplemented: yanking sexp-query paths"),
        CopyTarget::GetPath => return yank_err("unimplemented: yanking sexp-get paths"),
    };

    Ok(Ok(()))
}

fn yank_pretty_printed_node<W: io::Write>(
    mut output: W,
    doc: &DocState,
    node_index: NodeIndex,
) -> io::Result<()> {
    let pretty_printed_logical_lines = layout::layout_fully_expanded_node(&doc.core, node_index);
    let doc_content = doc.core.raw_bytes_of_complete_content();

    let multiple_lines = pretty_printed_logical_lines.len() > 1;

    // If we're yanking a value that is sexp-commented out, we want to format it as if it's
    // just a regular node, but `layout` will include extra indentation on all the subsequent
    // lines, so we need to remove that.
    //
    // Someday: This is pretty messy and the logic to ignore the top-level sexp-comment should
    // probably go in `layout`.
    let indentation_offset = {
        let first_line = &pretty_printed_logical_lines[0];
        if doc
            .core
            .token(first_line.start_index())
            .is_sexp_commented_out()
        {
            3
        } else {
            0
        }
    };

    for line in pretty_printed_logical_lines.into_iter() {
        for _ in 0..(line.indentation().saturating_sub(indentation_offset)) {
            output.write_all(b" ")?;
        }

        let start_index = line.start_index();
        let end_index = line.end_index();

        if let DocumentToken::Error(e) = doc.core.token(start_index) {
            output.write_all(b"; ERROR: ")?;
            output.write_all(e.message.as_bytes())?;
            continue;
        }

        if let Some(range) = doc.core.node(line.start_index()).sexp_comment_range() {
            // Only include nested sexp-comments, not the one on the value we're
            // yanking.
            if line.start_index() != node_index {
                output.write_all(&doc_content[range])?;
            }
        }

        let line_content = doc.core.raw_bytes_for_node_range(start_index, end_index);
        output.write_all(line_content)?;

        if multiple_lines {
            output.write_all(b"\n")?;
        }
    }

    Ok(())
}

fn yank_machine_node<W: io::Write>(
    mut output: W,
    doc: &DocState,
    mut node_index: NodeIndex,
) -> std::io::Result<()> {
    let end_node_index_incl = match doc.core.token(node_index) {
        DocumentToken::StartOfList(list_metadata) => list_metadata
            .end_index()
            .expect("list that we're yanking to be complete"),
        _ => node_index,
    };

    let mut need_space_before_next_node = false;
    let first_node_index = node_index;
    while node_index <= end_node_index_incl {
        let token_content = doc.core.raw_bytes_for_node(node_index);

        match doc.core.token(node_index) {
            DocumentToken::StartOfList(list_metadata) => {
                // Skip commented out nodes, unless we're focused on the commented out node
                // itself.
                if list_metadata.sexp_commented_out && node_index != first_node_index {
                    if let Some(list_end_index) = list_metadata.end_index() {
                        node_index = list_end_index + 1;
                        continue;
                    } else {
                        break;
                    }
                }

                if need_space_before_next_node {
                    output.write_all(b" (")?;
                } else {
                    output.write_all(b"(")?;
                }

                need_space_before_next_node = false;
            }
            DocumentToken::EndOfList(_) => {
                output.write_all(b")")?;
                need_space_before_next_node = true;
            }
            DocumentToken::Atom(AtomMetadata {
                sexp_commented_out, ..
            })
            | DocumentToken::Unit { sexp_commented_out } => {
                // Skip commented out nodes, unless we're focused on the commented out node
                // itself.
                if *sexp_commented_out && node_index != first_node_index {
                    node_index = node_index + 1;
                    continue;
                }

                if need_space_before_next_node {
                    output.write_all(b" ")?;
                }

                output.write_all(token_content)?;
                need_space_before_next_node = true;
            }
            // Don't print comments or errors
            DocumentToken::LineComment | DocumentToken::BlockComment | DocumentToken::Error(_) => {
                ()
            }
        }

        node_index = node_index + 1;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use bstr::ByteSlice;
    use insta::assert_snapshot;

    fn new_doc(bytes: &'static [u8]) -> DocState {
        DocState::new_from_bytes(bytes)
    }

    fn copy(doc: &DocState, node_index: usize, copy_target: CopyTarget) -> String {
        let mut output = vec![];
        match yank_content(&mut output, doc, NodeIndex(node_index), copy_target) {
            Ok(Ok(())) => format!("{}", output.as_slice().as_bstr()),
            Ok(Err(err)) => format!("yank err: {err}"),
            Err(err) => format!("write err: {err}"),
        }
    }

    #[test]
    fn test_yank_values() {
        let doc =
            new_doc(b"((a 1)(b \"two two\")(c (3 4 5))(d ((m 1)(n 2)))(e (Variant (x 1)(y 2))))");
        assert_snapshot!(doc.dump_all_logical_lines(), @r#"
         0..=4  : ((a 1)
         5..=8  :  (b "two two")
         9..=11 :  (c (
        12..=12 :    3
        13..=13 :    4
        14..=16 :    5))
        17..=19 :  (d (
        20..=23 :    (m 1)
        24..=29 :    (n 2)))
        30..=33 :  (e (Variant
        34..=37 :    (x 1)
        38..=44 :    (y 2))))
        "#);

        assert_snapshot!(copy(&doc, 1, CopyTarget::PrettyPrintedValue), @"1");
        assert_snapshot!(copy(&doc, 5, CopyTarget::PrettyPrintedValue), @r#""two two""#);

        assert_snapshot!(copy(&doc, 9, CopyTarget::PrettyPrintedValue), @r"
        (3
         4
         5)
        ");
        assert_snapshot!(copy(&doc, 9, CopyTarget::MachineValue), @"(3 4 5)");

        assert_snapshot!(copy(&doc, 17, CopyTarget::PrettyPrintedValue), @r"
        ((m 1)
         (n 2))
        ");
        assert_snapshot!(copy(&doc, 17, CopyTarget::MachineValue), @"((m 1) (n 2))");

        assert_snapshot!(copy(&doc, 30, CopyTarget::PrettyPrintedValue), @r"
        (Variant
          (x 1)
          (y 2))
        ");
        assert_snapshot!(copy(&doc, 30, CopyTarget::MachineValue), @"(Variant (x 1) (y 2))");

        assert_snapshot!(copy(&doc, 34, CopyTarget::PrettyPrintedValue), @"1");
    }

    #[test]
    fn test_yank_machine() {
        let doc = new_doc(b"(a \"b c\" \"e f\" g)");
        // "machine" is human machine, not like the `-machine` flag in the `sexp` tool.
        assert_snapshot!(copy(&doc, 0, CopyTarget::MachineValue), @r#"(a "b c" "e f" g)"#);
    }

    #[test]
    fn test_yanking_record_field_values_with_comments() {
        let doc = new_doc(b"((a ; comment\n value1)(b value2 #| comment |#))");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
         0..=2  : ((a
         3..=3  :     ; comment
         4..=5  :     value1)
         6..=8  :  (b value2
         9..=9  :   #| comment |#
        10..=11 : ))
        ");

        assert_snapshot!(copy(&doc, 1, CopyTarget::PrettyPrintedValue), @"value1");
        assert_snapshot!(copy(&doc, 6, CopyTarget::PrettyPrintedValue), @"value2");

        assert_snapshot!(copy(&doc, 1, CopyTarget::MachineValue), @"value1");
        assert_snapshot!(copy(&doc, 6, CopyTarget::MachineValue), @"value2");
    }

    #[test]
    fn test_yanking_fields_and_keys() {
        let doc = new_doc(b"((a 1)(b (2 3))(c ((d 4)(e 5)))(f (Var (g 6) (h 7))))((a ; comment\n value #| comment |#))");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
         0..=4  : ((a 1)
         5..=7  :  (b (
         8..=8  :    2
         9..=11 :    3))
        12..=14 :  (c (
        15..=18 :    (d 4)
        19..=24 :    (e 5)))
        25..=28 :  (f (Var
        29..=32 :    (g 6)
        33..=39 :    (h 7))))
        40..=42 : ((a
        43..=43 :     ; comment
        44..=44 :     value
        45..=45 :     #| comment |#
        46..=47 : ))
        ");

        // Not fields
        assert_snapshot!(
            copy(&doc, 2, CopyTarget::RecordField),
            @"yank err: Can't yank record field; not focused on record field",
        );
        assert_snapshot!(
            copy(&doc, 8, CopyTarget::MachineRecordField),
            @"yank err: Can't yank record field; not focused on record field",
        );

        // Not keys
        assert_snapshot!(
            copy(&doc, 2, CopyTarget::Key),
            @"yank err: Can't yank key; not focused on record field",
        );
        assert_snapshot!(
            copy(&doc, 8, CopyTarget::Key),
            @"yank err: Can't yank key; not focused on record field",
        );

        assert_snapshot!(copy(&doc, 1, CopyTarget::MachineRecordField), @"(a 1)");
        assert_snapshot!(copy(&doc, 1, CopyTarget::Key), @"a");
        assert_snapshot!(copy(&doc, 5, CopyTarget::MachineRecordField), @"(b (2 3))");
        assert_snapshot!(copy(&doc, 12, CopyTarget::MachineRecordField), @"(c ((d 4) (e 5)))");
        assert_snapshot!(copy(&doc, 25, CopyTarget::MachineRecordField), @"(f (Var (g 6) (h 7)))");
        assert_snapshot!(copy(&doc, 29, CopyTarget::MachineRecordField), @"(g 6)");
        assert_snapshot!(copy(&doc, 29, CopyTarget::Key), @"g");

        assert_snapshot!(copy(&doc, 1, CopyTarget::RecordField), @"(a 1)");

        assert_snapshot!(copy(&doc, 5, CopyTarget::RecordField), @r"
        (b (
          2
          3))
        ");

        assert_snapshot!(copy(&doc, 12, CopyTarget::RecordField), @r"
        (c (
          (d 4)
          (e 5)))
        ");

        assert_snapshot!(copy(&doc, 25, CopyTarget::RecordField), @r"
        (f (Var
          (g 6)
          (h 7)))
        ");

        assert_snapshot!(copy(&doc, 29, CopyTarget::RecordField), @"(g 6)");
    }

    #[test]
    fn test_yanking_constructor() {
        let doc = new_doc(b"zero (Variant 1 2)");

        assert_snapshot!(copy(&doc, 1, CopyTarget::Constructor), @"Variant");

        assert_snapshot!(copy(&doc, 0, CopyTarget::Constructor), @"yank err: Can't yank key; not focused on variant");
        assert_snapshot!(copy(&doc, 2, CopyTarget::Constructor), @"yank err: Can't yank key; not focused on variant");
        assert_snapshot!(copy(&doc, 3, CopyTarget::Constructor), @"yank err: Can't yank key; not focused on variant");
    }

    #[test]
    fn test_yank_string_values() {
        let doc = new_doc(b"zero \"quoted atom\" \"escaped\\natom\" \"invalid\\xxxatom\"");

        assert_snapshot!(copy(&doc, 0, CopyTarget::String), @"zero");
        assert_snapshot!(copy(&doc, 1, CopyTarget::String), @"yank err: unimplemented: Don't know how to unescape quoted values yet");
        assert_snapshot!(copy(&doc, 2, CopyTarget::String), @"yank err: unimplemented: Don't know how to unescape quoted values yet");
        assert_snapshot!(copy(&doc, 3, CopyTarget::String), @"yank err: Can't yank raw atom value; not focused on an atom");
    }

    #[test]
    fn test_yank_comments() {
        let doc = new_doc(b"; comment\n#| block comment |# (a #; (1 2 3) b)");

        assert_snapshot!(copy(&doc, 0, CopyTarget::PrettyPrintedValue), @"; comment");
        assert_snapshot!(copy(&doc, 0, CopyTarget::MachineValue), @"yank err: Comments are not included in machine format");

        assert_snapshot!(copy(&doc, 1, CopyTarget::PrettyPrintedValue), @"#| block comment |#");
        assert_snapshot!(copy(&doc, 1, CopyTarget::MachineValue), @"yank err: Comments are not included in machine format");

        assert_snapshot!(copy(&doc, 2, CopyTarget::PrettyPrintedValue), @r"
        (a
         #; (1
             2
             3)
         b)
        ");
        assert_snapshot!(copy(&doc, 2, CopyTarget::MachineValue), @"(a b)");

        // Don't include sexp comment when yanking a commented out value.
        assert_snapshot!(copy(&doc, 4, CopyTarget::PrettyPrintedValue), @r"
        (1
         2
         3)
        ");
        assert_snapshot!(copy(&doc, 4, CopyTarget::MachineValue), @"(1 2 3)");
    }

    #[test]
    fn test_yank_errors() {
        let doc = new_doc(b"(1 2");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=1  : (1
        2..=2  :  2
        3..=3  :  ERR: Unexpected EOF while parsing list
        4..=4  :
        ");

        assert_snapshot!(copy(&doc, 0, CopyTarget::PrettyPrintedValue), @r"
        (1
         2
         ; ERROR: Unexpected EOF while parsing list
        ");
        assert_snapshot!(copy(&doc, 3, CopyTarget::PrettyPrintedValue), @"; ERROR: Unexpected EOF while parsing list");

        assert_snapshot!(copy(&doc, 0, CopyTarget::MachineValue), @"(1 2)");
        assert_snapshot!(copy(&doc, 3, CopyTarget::MachineValue), @"yank err: Can't machine format an error");
    }
}
