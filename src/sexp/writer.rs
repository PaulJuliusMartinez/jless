use std::io;

use crate::document::WriteOp;
use crate::sexp::core::{invariants, AtomMetadata, DocumentToken, ListKind, NodeIndex};
use crate::sexp::layout;
use crate::sexp::path::DataNodePath;
use crate::sexp::state::DocState;

use ocaml_sexplib::atom::PlausibleSerializedAtom;

#[derive(Copy, Clone, Debug)]
enum LogicalWriteTarget {
    Value { machine: bool },
    RecordField { machine: bool },
    Siblings { machine: bool },
    Key,
    Constructor,
    UnescapedAtom,
    GetPath,
    QueryPath,
}

impl LogicalWriteTarget {
    fn for_yanking(ch: char) -> Result<Self, String> {
        use LogicalWriteTarget::*;
        let yank_target = match ch {
            'y' => Value { machine: false },
            'Y' | 'm' => Value { machine: true },
            't' => RecordField { machine: false },
            'T' => RecordField { machine: true },
            'a' => Siblings { machine: false },
            'A' => Siblings { machine: true },
            'k' => Key,
            'c' => Constructor,
            's' => UnescapedAtom,
            'g' => GetPath,
            'q' => QueryPath,
            _ => return Err(format!("Unknown yank target {ch:?}")),
        };

        Ok(yank_target)
    }

    fn for_printing(ch: char) -> Result<Self, String> {
        use LogicalWriteTarget::*;
        let print_target = match ch {
            'p' => Value { machine: false },
            'P' | 'm' => Value { machine: true },
            't' => RecordField { machine: false },
            'T' => RecordField { machine: true },
            'a' => Siblings { machine: false },
            'A' => Siblings { machine: true },
            'k' => Key,
            'c' => Constructor,
            's' => UnescapedAtom,
            'g' => GetPath,
            'q' => QueryPath,
            _ => return Err(format!("Unknown print target {ch:?}")),
        };

        Ok(print_target)
    }
}

pub enum PhysicalWriteTarget {
    Node {
        node_index: NodeIndex,
        machine: bool,
    },
    Siblings {
        first_sibling: NodeIndex,
        machine: bool,
    },
    UnescapedAtom(NodeIndex),
    GetPath(DataNodePath),
}

// We want slightly different behavior when yanking single values than when we
// yank multiple values (i.e. `LogicalWriteTarget::Siblings`).
//
// Newlines:
// When yanking a single big sexp, we want to include a trailing newline, because
// the value will likely be pasted as a block comment somewhere. But if the value
// is just a single atom, then the value will likely be pasted inline, so we don't
// want a trailing newline. When we yank multiple siblings at once, however, we
// definitely want a newline after each one.
//
// Root sexp comments:
// When yanking a single value, you don't care if it's sexp-commented out; that only
// matters in the context of the larger doc. But if you are yanking multiple siblings,
// you do want to include the sexp comments so you know which nodes are included and
// which ones aren't. (And when used at the top-level sexp, this means yanking the
// whole document, so obviously you want to include them in that case.)

enum TrailingNewline {
    WriteAlways,
    DontWriteIfSingleLine,
}

enum RootSexpComment {
    Write,
    Ignore,
}

struct WritePrettyOpts {
    trailing_newline: TrailingNewline,
    root_sexp_comment: RootSexpComment,
}

pub fn write_pretty_printed_doc<W: io::Write>(output: W, doc: &DocState) -> io::Result<()> {
    let machine = false;
    write_node_and_subsequent_siblings(output, doc, NodeIndex(0), machine)
}

pub fn write_additional_top_level_nodes_pretty_printed<W: io::Write>(
    output: W,
    doc: &DocState,
    next_top_level_node_index: NodeIndex,
) -> io::Result<()> {
    let machine = false;
    write_node_and_subsequent_siblings(output, doc, next_top_level_node_index, machine)
}

fn write_target_err(s: &'static str) -> Result<PhysicalWriteTarget, String> {
    Err(s.to_string())
}

pub fn validate_write_target_ch(
    doc: &DocState,
    node_index: NodeIndex,
    op: WriteOp,
    target_ch: char,
) -> Result<PhysicalWriteTarget, String> {
    let logical_target = match op {
        WriteOp::Yank => LogicalWriteTarget::for_yanking(target_ch),
        WriteOp::Print => LogicalWriteTarget::for_printing(target_ch),
    }?;

    validate_logical_write_target(doc, node_index, op, logical_target)
}

fn validate_logical_write_target(
    doc: &DocState,
    node_index: NodeIndex,
    op: WriteOp,
    target: LogicalWriteTarget,
) -> Result<PhysicalWriteTarget, String> {
    if matches!(doc.core.token(node_index), DocumentToken::EndOfList(_)) {
        return write_target_err("Focused on closing paren");
    }

    let record_field_value_node_index = doc.core.value_of_record_field(node_index);
    let value_node_index = record_field_value_node_index.unwrap_or(node_index);

    let physical_write_target = match target {
        LogicalWriteTarget::Value { machine } => {
            if machine {
                match doc.core.token(node_index) {
                    DocumentToken::LineComment | DocumentToken::BlockComment => {
                        return write_target_err("Comments are not included in machine format");
                    }
                    DocumentToken::Error(_) => {
                        return write_target_err("Cannot machine format an error");
                    }
                    _ => (),
                }
            }

            PhysicalWriteTarget::Node {
                node_index: value_node_index,
                machine,
            }
        }
        LogicalWriteTarget::RecordField { machine } => {
            if !matches!(
                doc.core.token(node_index).list_kind(),
                Some(ListKind::RecordField),
            ) {
                return Err(format!(
                    "Cannot {op} record field; not focused on record field"
                ));
            }

            PhysicalWriteTarget::Node {
                node_index,
                machine,
            }
        }
        LogicalWriteTarget::Siblings { machine } => {
            let first_sibling = match doc.core.parent_index(node_index) {
                Some(parent_index) => parent_index + 1,
                None => NodeIndex(0),
            };

            PhysicalWriteTarget::Siblings {
                first_sibling,
                machine,
            }
        }
        LogicalWriteTarget::Key => {
            if !matches!(
                doc.core.token(node_index).list_kind(),
                Some(ListKind::RecordField),
            ) {
                return Err(format!("Cannot {op} key; not focused on record field"));
            }

            invariants::record_keys_are_the_first_child_of_record_fields();
            let key_node_index = node_index + 1;

            PhysicalWriteTarget::Node {
                node_index: key_node_index,
                machine: true,
            }
        }
        LogicalWriteTarget::Constructor => {
            if !matches!(
                doc.core.token(value_node_index).list_kind(),
                Some(ListKind::VariantRecord | ListKind::VariantTuple),
            ) {
                return Err(format!("Cannot {op} constructor; not focused on variant"));
            }

            invariants::constructors_are_the_first_child_of_variants();
            let constructor_node_index = value_node_index + 1;

            PhysicalWriteTarget::Node {
                node_index: constructor_node_index,
                machine: true,
            }
        }
        LogicalWriteTarget::UnescapedAtom => match doc.core.token(value_node_index) {
            DocumentToken::Atom(AtomMetadata { valid, .. }) => {
                if !*valid {
                    return Err(format!(
                        "Cannot {op} raw atom value; atom contains invalid escapes"
                    ));
                }

                PhysicalWriteTarget::UnescapedAtom(value_node_index)
            }
            _ => {
                return Err(format!(
                    "Cannot {op} raw atom value; not focused on an atom"
                ));
            }
        },
        LogicalWriteTarget::GetPath => {
            if !doc.core.token(node_index).is_data() {
                return Err(format!("Cannot {op} path to non-data"));
            }

            let Some(path) = DataNodePath::build(&doc.core, node_index) else {
                return write_target_err("UNEXPECTED: Unable to build path to node");
            };

            PhysicalWriteTarget::GetPath(path)
        }
        LogicalWriteTarget::QueryPath => {
            return write_target_err("unimplemented: generating sexp-query paths")
        }
    };

    Ok(physical_write_target)
}

pub fn write_physical_target<W: io::Write>(
    mut output: W,
    doc: &DocState,
    op: WriteOp,
    target: PhysicalWriteTarget,
) -> io::Result<()> {
    let need_trailing_newline_if_printing;

    match target {
        PhysicalWriteTarget::Node {
            node_index,
            machine,
        } => {
            if machine {
                need_trailing_newline_if_printing = true;
                write_machine_node(&mut output, doc, node_index)?;
            } else {
                // We don't need to print the trailing newline in this function, because
                // we're telling `write_pretty_printed_node` to do it.
                need_trailing_newline_if_printing = false;

                let trailing_newline = match op {
                    WriteOp::Yank => TrailingNewline::DontWriteIfSingleLine,
                    WriteOp::Print => TrailingNewline::WriteAlways,
                };
                let opts = WritePrettyOpts {
                    trailing_newline,
                    root_sexp_comment: RootSexpComment::Ignore,
                };
                write_pretty_printed_node(&mut output, doc, node_index, opts)?;
            }
        }
        PhysicalWriteTarget::Siblings {
            first_sibling,
            machine,
        } => {
            need_trailing_newline_if_printing = false;
            write_node_and_subsequent_siblings(&mut output, doc, first_sibling, machine)?;
        }
        PhysicalWriteTarget::UnescapedAtom(node_index) => {
            // Unescaped atoms are written as is.
            need_trailing_newline_if_printing = true;

            let raw_bytes = doc.core.raw_bytes_for_node(node_index);
            let atom = PlausibleSerializedAtom::new(raw_bytes)
                .expect("doc content should always be valid-ish sexp");
            let mut scratch = vec![];

            // Someday: warn when escaping control characters, and also add yS for
            // unsafe escape (i.e., the current implementation).
            match atom.unescape(&mut scratch) {
                Ok(atom) => output.write_all(atom.bytes())?,
                Err(err) => {
                    // This shouldn't really happen, since we check if it's valid in
                    // `validate_logical_write_target`.
                    panic!("atom was marked valid, but unescaping failed: {:?}", err);
                }
            }
        }
        PhysicalWriteTarget::GetPath(path) => {
            need_trailing_newline_if_printing = true;
            let get_path = path.format_for_sexp_get(&doc.core);
            output.write_all(get_path.as_bytes())?;
        }
    }

    if need_trailing_newline_if_printing && matches!(op, WriteOp::Print) {
        write!(output, "\n")?;
    }

    Ok(())
}

fn write_node_and_subsequent_siblings<W: io::Write>(
    mut output: W,
    doc: &DocState,
    node_index: NodeIndex,
    machine: bool,
) -> io::Result<()> {
    let mut next_sibling = Some(node_index);

    while let Some(sibling) = next_sibling {
        if machine {
            let token = doc.core.token(sibling);
            if token.is_data() && !token.is_sexp_commented_out() {
                write_machine_node(&mut output, doc, sibling)?;
                write!(output, "\n")?;
            }
        } else {
            let opts = WritePrettyOpts {
                trailing_newline: TrailingNewline::WriteAlways,
                root_sexp_comment: RootSexpComment::Write,
            };
            write_pretty_printed_node(&mut output, doc, sibling, opts)?;
        }

        next_sibling = doc.core.node(sibling).next_sibling();
    }

    Ok(())
}

fn write_pretty_printed_node<W: io::Write>(
    mut output: W,
    doc: &DocState,
    node_index: NodeIndex,
    opts: WritePrettyOpts,
) -> io::Result<()> {
    let pretty_printed_logical_lines = layout::layout_fully_expanded_node(&doc.core, node_index);
    let doc_content = doc.core.raw_bytes_of_complete_content();

    let multiple_lines = pretty_printed_logical_lines.len() > 1;

    // If we're yanking a value that is sexp-commented out, we may want to format it as if it's
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
            match opts.root_sexp_comment {
                RootSexpComment::Write => {
                    output.write_all(b"#; ")?;
                    0
                }
                RootSexpComment::Ignore => 3,
            }
        } else {
            0
        }
    };

    for (i, line) in pretty_printed_logical_lines.into_iter().enumerate() {
        if i > 0 {
            output.write_all(b"\n")?;
        }

        // Annoyingly, if we layout a sexp-commented out node, then every line will
        // have an extra 3 indentation, *except* the first line, which is why we
        // have the `saturating_sub` here.
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
    }

    if multiple_lines || matches!(opts.trailing_newline, TrailingNewline::WriteAlways) {
        output.write_all(b"\n")?;
    }

    Ok(())
}

fn write_machine_node<W: io::Write>(
    mut output: W,
    doc: &DocState,
    mut node_index: NodeIndex,
) -> std::io::Result<()> {
    let end_node_index_incl = match doc.core.token(node_index) {
        DocumentToken::StartOfList(list_metadata) => list_metadata
            .end_index()
            .expect("list that we're writing to be complete"),
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
    use insta::{allow_duplicates, assert_debug_snapshot, assert_snapshot};

    use LogicalWriteTarget::*;

    const PRETTY_PRINTED_VALUE: LogicalWriteTarget = Value { machine: false };
    const MACHINE_VALUE: LogicalWriteTarget = Value { machine: true };
    const RECORD_FIELD: LogicalWriteTarget = RecordField { machine: false };
    const MACHINE_RECORD_FIELD: LogicalWriteTarget = RecordField { machine: true };
    const SIBLINGS: LogicalWriteTarget = Siblings { machine: false };
    const MACHINE_SIBLINGS: LogicalWriteTarget = Siblings { machine: true };

    fn new_doc(bytes: &'static [u8]) -> DocState {
        DocState::new_from_bytes(bytes)
    }

    fn new_partial_doc(bytes: &'static [u8]) -> DocState {
        DocState::new_partial_doc_from_bytes(bytes)
    }

    fn write(doc: &DocState, node_index: usize, op: WriteOp, target: LogicalWriteTarget) -> String {
        let node_index = NodeIndex(node_index);
        match validate_logical_write_target(doc, node_index, op, target) {
            Err(err) => format!("{op} err: {err}"),
            Ok(physical_target) => {
                let mut output = vec![];
                match write_physical_target(&mut output, doc, op, physical_target) {
                    Ok(()) => format!("{}", output.as_slice().as_bstr()),
                    Err(err) => format!("write err: {err}"),
                }
            }
        }
    }

    fn yank(doc: &DocState, node_index: usize, yank_target: LogicalWriteTarget) -> String {
        write(doc, node_index, WriteOp::Yank, yank_target)
    }

    fn print(doc: &DocState, node_index: usize, print_target: LogicalWriteTarget) -> String {
        write(doc, node_index, WriteOp::Print, print_target)
    }

    #[test]
    fn test_write_values() {
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

        // Printing always includes trailing newline, but yanking does not.
        assert_debug_snapshot!(yank(&doc, 1, PRETTY_PRINTED_VALUE), @r#""1""#);
        assert_debug_snapshot!(print(&doc, 1, PRETTY_PRINTED_VALUE), @r#""1\n""#);

        assert_snapshot!(yank(&doc, 5, PRETTY_PRINTED_VALUE), @r#""two two""#);

        // It doesn't add an _extra_ newline though.
        assert_debug_snapshot!(yank(&doc, 9, PRETTY_PRINTED_VALUE), @r#""(3\n 4\n 5)\n""#);
        assert_debug_snapshot!(print(&doc, 9, PRETTY_PRINTED_VALUE), @r#""(3\n 4\n 5)\n""#);

        // It adds a newline to the end of machine values too.
        assert_debug_snapshot!(yank(&doc, 9, MACHINE_VALUE), @r#""(3 4 5)""#);
        assert_debug_snapshot!(print(&doc, 9, MACHINE_VALUE), @r#""(3 4 5)\n""#);

        assert_snapshot!(yank(&doc, 17, PRETTY_PRINTED_VALUE), @r"
        ((m 1)
         (n 2))
        ");
        assert_snapshot!(yank(&doc, 17, MACHINE_VALUE), @"((m 1) (n 2))");

        assert_snapshot!(yank(&doc, 30, PRETTY_PRINTED_VALUE), @r"
        (Variant
          (x 1)
          (y 2))
        ");
        assert_snapshot!(yank(&doc, 30, MACHINE_VALUE), @"(Variant (x 1) (y 2))");

        assert_snapshot!(yank(&doc, 34, PRETTY_PRINTED_VALUE), @"1");
    }

    #[test]
    fn test_write_siblings() {
        for op in [WriteOp::Yank, WriteOp::Print] {
            allow_duplicates! {
                let doc = new_doc(b"one #| two |# (three four) ; five\n #; six");
                assert_snapshot!(doc.dump_all_logical_lines(), @r"
                0..=0  : one
                1..=1  : #| two |#
                2..=5  : (three four)
                6..=6  : ; five
                7..=7  : #; six
                ");

                assert_snapshot!(write(&doc, 0, op, SIBLINGS), @r"
                one
                #| two |#
                (three four)
                ; five
                #; six
                ");

                assert_snapshot!(write(&doc, 1, op, MACHINE_SIBLINGS), @r"
                one
                (three four)
                ");

                // Same doc, but in a list
                let doc = new_doc(b"(one #| two |# (three four) ; five\n #; six)");

                // Note also it doesn't matter which sibling we target.
                assert_snapshot!(write(&doc, 2, op, SIBLINGS), @r"
                one
                #| two |#
                (three four)
                ; five
                #; six
                ");

                assert_snapshot!(write(&doc, 3, op, MACHINE_SIBLINGS), @r"
                one
                (three four)
                ");

                let doc = new_doc(b"((a 1)(b 2)(c 3))");
                // Yanking siblings of a record gives just a bunch of key-value
                // pairs, which is a little strange, but seems fine.
                assert_snapshot!(write(&doc, 1, op, SIBLINGS), @r"
                (a 1)
                (b 2)
                (c 3)
                ");
            }
        }
    }

    #[test]
    fn test_write_doc() {
        let mut doc = new_partial_doc(b"(one two three)");
        let mut output = vec![];
        write_pretty_printed_doc(&mut output, &doc).unwrap();
        assert_snapshot!(output.as_slice().as_bstr(), @r"
        (one
         two
         three)
        ");

        doc.append(b"(four five)(six");
        let mut output = vec![];
        write_additional_top_level_nodes_pretty_printed(&mut output, &doc, NodeIndex(5)).unwrap();
        assert_snapshot!(output.as_slice().as_bstr(), @"(four five)");

        doc.append(b" seven)");
        doc.eof();
        let mut output = vec![];
        write_additional_top_level_nodes_pretty_printed(&mut output, &doc, NodeIndex(9)).unwrap();
        assert_snapshot!(output.as_slice().as_bstr(), @"(six seven)");

        let mut output = vec![];
        write_pretty_printed_doc(&mut output, &doc).unwrap();
        assert_snapshot!(output.as_slice().as_bstr(), @r"
        (one
         two
         three)
        (four five)
        (six seven)
        ");
    }

    #[test]
    fn test_yank_machine_is_more_like_human_than_actual_machine() {
        let doc = new_doc(b"(a \"b c\" \"e f\" g)");
        // "machine" is human machine, not like the `-machine` flag in the `sexp` tool.
        assert_snapshot!(yank(&doc, 0, MACHINE_VALUE), @r#"(a "b c" "e f" g)"#);
    }

    #[test]
    fn test_writing_record_field_values_with_comments() {
        let doc = new_doc(b"((a ; comment\n value1)(b value2 #| comment |#))");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
         0..=2  : ((a
         3..=3  :     ; comment
         4..=5  :     value1)
         6..=8  :  (b value2
         9..=9  :   #| comment |#
        10..=11 : ))
        ");

        assert_snapshot!(yank(&doc, 1, PRETTY_PRINTED_VALUE), @"value1");
        assert_debug_snapshot!(print(&doc, 1, PRETTY_PRINTED_VALUE), @r#""value1\n""#);
        assert_snapshot!(yank(&doc, 6, PRETTY_PRINTED_VALUE), @"value2");

        assert_snapshot!(yank(&doc, 1, MACHINE_VALUE), @"value1");
        assert_snapshot!(yank(&doc, 6, MACHINE_VALUE), @"value2");
    }

    #[test]
    fn test_writing_fields_and_keys() {
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
            yank(&doc, 2, RECORD_FIELD),
            @"yank err: Cannot yank record field; not focused on record field",
        );
        assert_snapshot!(
            print(&doc, 8, MACHINE_RECORD_FIELD),
            @"print err: Cannot print record field; not focused on record field",
        );

        // Not keys
        assert_snapshot!(
            yank(&doc, 2, Key),
            @"yank err: Cannot yank key; not focused on record field",
        );
        assert_snapshot!(
            print(&doc, 8, Key),
            @"print err: Cannot print key; not focused on record field",
        );

        assert_snapshot!(yank(&doc, 1, MACHINE_RECORD_FIELD), @"(a 1)");
        assert_snapshot!(yank(&doc, 1, Key), @"a");
        assert_snapshot!(yank(&doc, 5, MACHINE_RECORD_FIELD), @"(b (2 3))");
        assert_snapshot!(yank(&doc, 12, MACHINE_RECORD_FIELD), @"(c ((d 4) (e 5)))");
        assert_snapshot!(yank(&doc, 25, MACHINE_RECORD_FIELD), @"(f (Var (g 6) (h 7)))");
        assert_snapshot!(yank(&doc, 29, MACHINE_RECORD_FIELD), @"(g 6)");
        assert_snapshot!(yank(&doc, 29, Key), @"g");

        assert_snapshot!(yank(&doc, 1, RECORD_FIELD), @"(a 1)");

        assert_snapshot!(yank(&doc, 5, RECORD_FIELD), @r"
        (b (
          2
          3))
        ");

        assert_snapshot!(yank(&doc, 12, RECORD_FIELD), @r"
        (c (
          (d 4)
          (e 5)))
        ");

        assert_snapshot!(yank(&doc, 25, RECORD_FIELD), @r"
        (f (Var
          (g 6)
          (h 7)))
        ");

        assert_snapshot!(yank(&doc, 29, RECORD_FIELD), @"(g 6)");
    }

    #[test]
    fn test_writing_constructor() {
        let doc = new_doc(b"zero (Variant 1 2)");

        assert_snapshot!(yank(&doc, 1, Constructor), @"Variant");
        assert_debug_snapshot!(print(&doc, 1, Constructor), @r#""Variant\n""#);

        assert_snapshot!(yank(&doc, 0, Constructor), @"yank err: Cannot yank constructor; not focused on variant");
        assert_snapshot!(yank(&doc, 2, Constructor), @"yank err: Cannot yank constructor; not focused on variant");
        assert_snapshot!(print(&doc, 3, Constructor), @"print err: Cannot print constructor; not focused on variant");
    }

    #[test]
    fn test_write_string_values() {
        let doc = new_doc(
            b"zero \"quoted atom\" \"escaped\\natom\" \"invalid\\xxxatom\" \"control\\x01atom\"",
        );
        assert_snapshot!(doc.dump_all_logical_lines(), @r#"
        0..=0  : zero
        1..=1  : "quoted atom"
        2..=2  : "escaped\natom"
        3..=3  : ERR: Unable to unescape atom: InvalidHexadecimalEscape
        4..=4  : "invalid\xxxatom"
        5..=5  : "control\x01atom"
        "#);

        assert_debug_snapshot!(yank(&doc, 0, UnescapedAtom), @r#""zero""#);
        assert_debug_snapshot!(print(&doc, 0, UnescapedAtom), @r#""zero\n""#);
        assert_snapshot!(yank(&doc, 1, UnescapedAtom), @"quoted atom");

        assert_debug_snapshot!(yank(&doc, 2, UnescapedAtom), @r#""escaped\natom""#);
        assert_debug_snapshot!(print(&doc, 2, UnescapedAtom), @r#""escaped\natom\n""#);

        assert_snapshot!(yank(&doc, 4, UnescapedAtom), @"yank err: Cannot yank raw atom value; atom contains invalid escapes");
        assert_snapshot!(yank(&doc, 5, UnescapedAtom), @"control\u{1}atom");
    }

    #[test]
    fn test_write_comments() {
        let doc = new_doc(b"; comment\n#| block comment |# (a #; (1 2 3) b)");

        assert_snapshot!(yank(&doc, 0, PRETTY_PRINTED_VALUE), @"; comment");
        assert_debug_snapshot!(print(&doc, 0, PRETTY_PRINTED_VALUE), @r#""; comment\n""#);

        assert_snapshot!(yank(&doc, 0, MACHINE_VALUE), @"yank err: Comments are not included in machine format");

        assert_snapshot!(yank(&doc, 1, PRETTY_PRINTED_VALUE), @"#| block comment |#");
        assert_snapshot!(yank(&doc, 1, MACHINE_VALUE), @"yank err: Comments are not included in machine format");

        assert_snapshot!(yank(&doc, 2, PRETTY_PRINTED_VALUE), @r"
        (a
         #; (1
             2
             3)
         b)
        ");
        assert_snapshot!(yank(&doc, 2, MACHINE_VALUE), @"(a b)");

        // Don't include sexp comment when yanking a commented out value.
        assert_snapshot!(yank(&doc, 4, PRETTY_PRINTED_VALUE), @r"
        (1
         2
         3)
        ");
        assert_snapshot!(yank(&doc, 4, MACHINE_VALUE), @"(1 2 3)");
    }

    #[test]
    fn test_write_errors() {
        let doc = new_doc(b"(1 2");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=1  : (1
        2..=2  :  2
        3..=3  :  ERR: Unexpected EOF while parsing list
        4..=4  :
        ");

        assert_snapshot!(yank(&doc, 0, PRETTY_PRINTED_VALUE), @r"
        (1
         2
         ; ERROR: Unexpected EOF while parsing list
        ");
        assert_snapshot!(yank(&doc, 3, PRETTY_PRINTED_VALUE), @"; ERROR: Unexpected EOF while parsing list");
        assert_debug_snapshot!(print(&doc, 3, PRETTY_PRINTED_VALUE), @r#""; ERROR: Unexpected EOF while parsing list\n""#);

        assert_snapshot!(yank(&doc, 0, MACHINE_VALUE), @"(1 2)");
        assert_snapshot!(yank(&doc, 3, MACHINE_VALUE), @"yank err: Cannot machine format an error");
    }

    #[test]
    fn test_write_leading_parens() {
        let doc = new_doc(b"((a 1) (b 2 #| x |#) (c (Var 3 4 #| y |#)) #| z |#)");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
         0..=4  : ((a 1)
         5..=7  :  (b 2
         8..=8  :   #| x |#
         9..=9  :  )
        10..=13 :  (c (Var
        14..=14 :    3
        15..=15 :    4
        16..=16 :    #| y |#
        17..=18 :  ))
        19..=19 :  #| z |#
        20..=20 : )
        ");

        assert_snapshot!(yank(&doc, 9, PRETTY_PRINTED_VALUE), @"yank err: Focused on closing paren");
        assert_snapshot!(yank(&doc, 9, RECORD_FIELD), @"yank err: Focused on closing paren");
        assert_snapshot!(yank(&doc, 17, PRETTY_PRINTED_VALUE), @"yank err: Focused on closing paren");
        assert_snapshot!(print(&doc, 20, PRETTY_PRINTED_VALUE), @"print err: Focused on closing paren");
    }

    #[test]
    fn test_write_paths() {
        let doc =
            new_doc(b"a #| comment |# #; b ((a 1) #; (x 0) (b (2 #; 2.5 #| inner comment |# 3)))");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
         0..=0  : a
         1..=1  : #| comment |#
         2..=2  : #; b
         3..=7  : ((a 1)
         8..=11 :  #; (x 0)
        12..=14 :  (b (
        15..=15 :    2
        16..=16 :    #; 2.5
        17..=17 :    #| inner comment |#
        18..=21 :    3)))
        ");

        assert_snapshot!(yank(&doc, 0, GetPath), @".");
        assert_snapshot!(yank(&doc, 1, GetPath), @"yank err: Cannot yank path to non-data");
        assert_snapshot!(yank(&doc, 2, GetPath), @".");

        assert_snapshot!(yank(&doc, 4, GetPath), @".a");
        assert_debug_snapshot!(print(&doc, 4, GetPath), @r#"".a\n""#);

        assert_snapshot!(yank(&doc, 8, GetPath), @".x");
        assert_snapshot!(yank(&doc, 16, GetPath), @".b.[_]");
        assert_snapshot!(yank(&doc, 17, GetPath), @"yank err: Cannot yank path to non-data");
        assert_snapshot!(yank(&doc, 18, GetPath), @".b.[1]");
    }
}
