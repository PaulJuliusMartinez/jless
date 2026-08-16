use std::io;

use crate::document::YankResult;
use crate::sexp::core::{invariants, AtomMetadata, DocumentToken, ListKind, NodeIndex};
use crate::sexp::layout;
use crate::sexp::state::DocState;

use ocaml_sexplib::atom::PlausibleSerializedAtom;

#[derive(Copy, Clone, Debug)]
pub enum CopyTarget {
    Value { machine: bool },
    RecordField { machine: bool },
    Siblings { machine: bool },
    Key,
    Constructor,
    String,
    QueryPath,
    GetPath,
}

impl CopyTarget {
    pub fn from_char(ch: char) -> Result<Self, String> {
        let copy_target = match ch {
            'y' => CopyTarget::Value { machine: false },
            'Y' | 'm' => CopyTarget::Value { machine: true },
            't' | 'r' => CopyTarget::RecordField { machine: false },
            'T' | 'R' => CopyTarget::RecordField { machine: true },
            'a' => CopyTarget::Siblings { machine: false },
            'A' => CopyTarget::Siblings { machine: true },
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

enum TrailingNewline {
    WriteAlways,
    DontWriteIfSingleLine,
}

enum RootSexpComment {
    Write,
    Ignore,
}

// We want slightly different behavior when yanking single values than when we
// yank multiple values (i.e. `CopyTarget::Siblings`).
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
struct YankPrettyOpts {
    trailing_newline: TrailingNewline,
    root_sexp_comment: RootSexpComment,
}

pub fn yank_content<W: io::Write>(
    mut output: W,
    doc: &DocState,
    node_index: NodeIndex,
    target: CopyTarget,
) -> io::Result<YankResult> {
    if matches!(doc.core.token(node_index), DocumentToken::EndOfList(_)) {
        return yank_err("Focused on closing paren");
    }

    let record_field_value_node_index = doc.core.value_of_record_field(node_index);
    let value_node_index = record_field_value_node_index.unwrap_or(node_index);

    match target {
        CopyTarget::Value { machine } => {
            if machine {
                match doc.core.token(node_index) {
                    DocumentToken::LineComment | DocumentToken::BlockComment => {
                        return yank_err("Comments are not included in machine format");
                    }
                    DocumentToken::Error(_) => {
                        return yank_err("Can't machine format an error");
                    }
                    _ => yank_machine_node(output, doc, value_node_index)?,
                }
            } else {
                let opts = YankPrettyOpts {
                    trailing_newline: TrailingNewline::DontWriteIfSingleLine,
                    root_sexp_comment: RootSexpComment::Ignore,
                };
                yank_pretty_printed_node(output, doc, value_node_index, opts)?
            }
        }
        CopyTarget::RecordField { machine } => {
            if matches!(
                doc.core.token(node_index).list_kind(),
                Some(ListKind::RecordField),
            ) {
                if machine {
                    yank_machine_node(output, doc, node_index)?;
                } else {
                    let opts = YankPrettyOpts {
                        trailing_newline: TrailingNewline::DontWriteIfSingleLine,
                        root_sexp_comment: RootSexpComment::Ignore,
                    };
                    yank_pretty_printed_node(output, doc, node_index, opts)?;
                }
            } else {
                return yank_err("Can't yank record field; not focused on record field");
            }
        }
        CopyTarget::Siblings { machine } => {
            let first_sibling = match doc.core.parent_index(node_index) {
                Some(parent_index) => parent_index + 1,
                None => NodeIndex(0),
            };

            yank_siblings(output, doc, first_sibling, machine)?
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
            DocumentToken::Atom(AtomMetadata { valid, .. }) => {
                if !*valid {
                    return yank_err("Can't yank raw atom value; atom contains invalid escapes");
                }

                let raw_bytes = doc.core.raw_bytes_for_node(value_node_index);
                let atom = PlausibleSerializedAtom::new(raw_bytes)
                    .expect("doc content should always be valid-ish sexp");
                let mut scratch = vec![];

                // Someday: warn when escaping control characters, and also add yS for
                // unsafe escape (i.e., the current implementation).
                match atom.unescape(&mut scratch) {
                    Ok(atom) => output.write_all(atom.bytes())?,
                    Err(err) => {
                        // This shouldn't really happen, since we check if it's valid above.
                        return Ok(Err(format!("unable to escape atom: {err:?}")));
                    }
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

fn yank_siblings<W: io::Write>(
    mut output: W,
    doc: &DocState,
    first_sibling: NodeIndex,
    machine: bool,
) -> io::Result<()> {
    let mut next_sibling = Some(first_sibling);

    while let Some(sibling) = next_sibling {
        if machine {
            let token = doc.core.token(sibling);
            if token.is_data() && !token.is_sexp_commented_out() {
                yank_machine_node(&mut output, doc, sibling)?;
                write!(output, "\n")?;
            }
        } else {
            let opts = YankPrettyOpts {
                trailing_newline: TrailingNewline::WriteAlways,
                root_sexp_comment: RootSexpComment::Write,
            };
            yank_pretty_printed_node(&mut output, doc, sibling, opts)?;
        }

        next_sibling = doc.core.node(sibling).next_sibling();
    }

    Ok(())
}

fn yank_pretty_printed_node<W: io::Write>(
    mut output: W,
    doc: &DocState,
    node_index: NodeIndex,
    opts: YankPrettyOpts,
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

    const PRETTY_PRINTED_VALUE: CopyTarget = CopyTarget::Value { machine: false };
    const MACHINE_VALUE: CopyTarget = CopyTarget::Value { machine: true };
    const RECORD_FIELD: CopyTarget = CopyTarget::RecordField { machine: false };
    const MACHINE_RECORD_FIELD: CopyTarget = CopyTarget::RecordField { machine: true };
    const SIBLINGS: CopyTarget = CopyTarget::Siblings { machine: false };
    const MACHINE_SIBLINGS: CopyTarget = CopyTarget::Siblings { machine: true };

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

        assert_snapshot!(copy(&doc, 1, PRETTY_PRINTED_VALUE), @"1");
        assert_snapshot!(copy(&doc, 5, PRETTY_PRINTED_VALUE), @r#""two two""#);

        assert_snapshot!(copy(&doc, 9, PRETTY_PRINTED_VALUE), @r"
        (3
         4
         5)
        ");
        assert_snapshot!(copy(&doc, 9, MACHINE_VALUE), @"(3 4 5)");

        assert_snapshot!(copy(&doc, 17, PRETTY_PRINTED_VALUE), @r"
        ((m 1)
         (n 2))
        ");
        assert_snapshot!(copy(&doc, 17, MACHINE_VALUE), @"((m 1) (n 2))");

        assert_snapshot!(copy(&doc, 30, PRETTY_PRINTED_VALUE), @r"
        (Variant
          (x 1)
          (y 2))
        ");
        assert_snapshot!(copy(&doc, 30, MACHINE_VALUE), @"(Variant (x 1) (y 2))");

        assert_snapshot!(copy(&doc, 34, PRETTY_PRINTED_VALUE), @"1");
    }

    #[test]
    fn test_yank_siblings() {
        let doc = new_doc(b"one #| two |# (three four) ; five\n #; six");
        assert_snapshot!(doc.dump_all_logical_lines(), @r"
        0..=0  : one
        1..=1  : #| two |#
        2..=5  : (three four)
        6..=6  : ; five
        7..=7  : #; six
        ");

        assert_snapshot!(copy(&doc, 0, SIBLINGS), @r"
        one
        #| two |#
        (three four)
        ; five
        #; six
        ");

        assert_snapshot!(copy(&doc, 1, MACHINE_SIBLINGS), @r"
        one
        (three four)
        ");

        // Same doc, but in a list
        let doc = new_doc(b"(one #| two |# (three four) ; five\n #; six)");

        // Note also it doesn't matter which sibling we target.
        assert_snapshot!(copy(&doc, 2, SIBLINGS), @r"
        one
        #| two |#
        (three four)
        ; five
        #; six
        ");

        assert_snapshot!(copy(&doc, 3, MACHINE_SIBLINGS), @r"
        one
        (three four)
        ");

        let doc = new_doc(b"((a 1)(b 2)(c 3))");
        // Yanking siblings of a record gives just a bunch of key-value
        // pairs, which is a little strange, but seems fine.
        assert_snapshot!(copy(&doc, 1, SIBLINGS), @r"
        (a 1)
        (b 2)
        (c 3)
        ");
    }

    #[test]
    fn test_yank_machine() {
        let doc = new_doc(b"(a \"b c\" \"e f\" g)");
        // "machine" is human machine, not like the `-machine` flag in the `sexp` tool.
        assert_snapshot!(copy(&doc, 0, MACHINE_VALUE), @r#"(a "b c" "e f" g)"#);
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

        assert_snapshot!(copy(&doc, 1, PRETTY_PRINTED_VALUE), @"value1");
        assert_snapshot!(copy(&doc, 6, PRETTY_PRINTED_VALUE), @"value2");

        assert_snapshot!(copy(&doc, 1, MACHINE_VALUE), @"value1");
        assert_snapshot!(copy(&doc, 6, MACHINE_VALUE), @"value2");
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
            copy(&doc, 2, RECORD_FIELD),
            @"yank err: Can't yank record field; not focused on record field",
        );
        assert_snapshot!(
            copy(&doc, 8, MACHINE_RECORD_FIELD),
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

        assert_snapshot!(copy(&doc, 1, MACHINE_RECORD_FIELD), @"(a 1)");
        assert_snapshot!(copy(&doc, 1, CopyTarget::Key), @"a");
        assert_snapshot!(copy(&doc, 5, MACHINE_RECORD_FIELD), @"(b (2 3))");
        assert_snapshot!(copy(&doc, 12, MACHINE_RECORD_FIELD), @"(c ((d 4) (e 5)))");
        assert_snapshot!(copy(&doc, 25, MACHINE_RECORD_FIELD), @"(f (Var (g 6) (h 7)))");
        assert_snapshot!(copy(&doc, 29, MACHINE_RECORD_FIELD), @"(g 6)");
        assert_snapshot!(copy(&doc, 29, CopyTarget::Key), @"g");

        assert_snapshot!(copy(&doc, 1, RECORD_FIELD), @"(a 1)");

        assert_snapshot!(copy(&doc, 5, RECORD_FIELD), @r"
        (b (
          2
          3))
        ");

        assert_snapshot!(copy(&doc, 12, RECORD_FIELD), @r"
        (c (
          (d 4)
          (e 5)))
        ");

        assert_snapshot!(copy(&doc, 25, RECORD_FIELD), @r"
        (f (Var
          (g 6)
          (h 7)))
        ");

        assert_snapshot!(copy(&doc, 29, RECORD_FIELD), @"(g 6)");
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

        assert_snapshot!(copy(&doc, 0, CopyTarget::String), @"zero");
        assert_snapshot!(copy(&doc, 1, CopyTarget::String), @"quoted atom");
        assert_snapshot!(copy(&doc, 2, CopyTarget::String), @r"
        escaped
        atom
        ");
        assert_snapshot!(copy(&doc, 4, CopyTarget::String), @"yank err: Can't yank raw atom value; atom contains invalid escapes");
        assert_snapshot!(copy(&doc, 5, CopyTarget::String), @"control\u{1}atom");
    }

    #[test]
    fn test_yank_comments() {
        let doc = new_doc(b"; comment\n#| block comment |# (a #; (1 2 3) b)");

        assert_snapshot!(copy(&doc, 0, PRETTY_PRINTED_VALUE), @"; comment");
        assert_snapshot!(copy(&doc, 0, MACHINE_VALUE), @"yank err: Comments are not included in machine format");

        assert_snapshot!(copy(&doc, 1, PRETTY_PRINTED_VALUE), @"#| block comment |#");
        assert_snapshot!(copy(&doc, 1, MACHINE_VALUE), @"yank err: Comments are not included in machine format");

        assert_snapshot!(copy(&doc, 2, PRETTY_PRINTED_VALUE), @r"
        (a
         #; (1
             2
             3)
         b)
        ");
        assert_snapshot!(copy(&doc, 2, MACHINE_VALUE), @"(a b)");

        // Don't include sexp comment when yanking a commented out value.
        assert_snapshot!(copy(&doc, 4, PRETTY_PRINTED_VALUE), @r"
        (1
         2
         3)
        ");
        assert_snapshot!(copy(&doc, 4, MACHINE_VALUE), @"(1 2 3)");
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

        assert_snapshot!(copy(&doc, 0, PRETTY_PRINTED_VALUE), @r"
        (1
         2
         ; ERROR: Unexpected EOF while parsing list
        ");
        assert_snapshot!(copy(&doc, 3, PRETTY_PRINTED_VALUE), @"; ERROR: Unexpected EOF while parsing list");

        assert_snapshot!(copy(&doc, 0, MACHINE_VALUE), @"(1 2)");
        assert_snapshot!(copy(&doc, 3, MACHINE_VALUE), @"yank err: Can't machine format an error");
    }

    #[test]
    fn test_yank_leading_parens() {
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

        assert_snapshot!(copy(&doc, 9, PRETTY_PRINTED_VALUE), @"yank err: Focused on closing paren");
        assert_snapshot!(copy(&doc, 9, RECORD_FIELD), @"yank err: Focused on closing paren");
        assert_snapshot!(copy(&doc, 17, PRETTY_PRINTED_VALUE), @"yank err: Focused on closing paren");
        assert_snapshot!(copy(&doc, 20, PRETTY_PRINTED_VALUE), @"yank err: Focused on closing paren");
    }
}
