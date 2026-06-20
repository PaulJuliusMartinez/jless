use std::ops::{Index, Range};

use ocaml_sexplib::atom::AtomData;
#[cfg(test)]
use serde::Serialize;

#[cfg_attr(test, derive(Serialize))]
#[derive(Debug)]
pub struct PrettyPrinted {
    data: Vec<u8>,
    space_before_next_node: bool,
    newline_before_next_node: bool,
}

impl PrettyPrinted {
    pub fn new() -> Self {
        PrettyPrinted {
            data: vec![],
            space_before_next_node: false,
            newline_before_next_node: false,
        }
    }

    fn maybe_write_whitespace(&mut self) {
        if self.newline_before_next_node {
            self.data.push(b'\n');
            self.newline_before_next_node = false;
        }

        if self.space_before_next_node {
            self.data.push(b' ');
        }
    }

    fn maybe_write_sexp_comment(&mut self, write_comment: bool) {
        if write_comment {
            self.data.extend(b"#; ");
        }
    }

    pub fn start_list(&mut self, commented_out: bool) -> Range<usize> {
        self.maybe_write_whitespace();
        self.maybe_write_sexp_comment(commented_out);

        let start = self.data.len();
        let end = start + 1;

        self.data.push(b'(');
        self.space_before_next_node = false;

        start..end
    }

    pub fn end_list(&mut self) -> Range<usize> {
        let start = self.data.len();
        let end = start + 1;

        self.data.push(b')');
        self.space_before_next_node = true;

        start..end
    }

    pub fn write_atom(&mut self, atom: &AtomData, commented_out: bool) -> Range<usize> {
        self.maybe_write_whitespace();
        self.maybe_write_sexp_comment(commented_out);

        let start = self.data.len();

        // Writing to a Vec<u8> shouldn't fail.
        let _ = atom.serialize_io(&mut self.data);
        self.space_before_next_node = true;

        let end = self.data.len();

        start..end
    }

    pub fn write_malformed_atom(&mut self, bytes: &[u8], commented_out: bool) -> Range<usize> {
        self.maybe_write_whitespace();
        self.maybe_write_sexp_comment(commented_out);

        let start = self.data.len();
        let end = start + bytes.len();

        self.data.extend(bytes);
        self.space_before_next_node = true;

        start..end
    }

    pub fn write_line_comment(&mut self, comment: &[u8]) -> Range<usize> {
        self.maybe_write_whitespace();

        let start = self.data.len();
        let end = start + comment.len();

        self.data.extend(comment);
        self.data.push(b'\n');
        self.space_before_next_node = false;

        start..end
    }

    pub fn write_block_comment(&mut self, comment: &[u8]) -> Range<usize> {
        self.maybe_write_whitespace();

        let start = self.data.len();
        let end = start + comment.len();

        self.data.extend(comment);
        self.space_before_next_node = true;

        start..end
    }

    pub fn complete_top_level_node(&mut self) {
        self.space_before_next_node = false;
        self.newline_before_next_node = true;
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn len(&self) -> usize {
        self.data.len()
    }
}

impl Index<Range<usize>> for PrettyPrinted {
    type Output = [u8];

    fn index(&self, range: Range<usize>) -> &[u8] {
        &self.data[range]
    }
}
