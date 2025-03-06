use std::fmt::{Debug, Write};
use std::ops::Range;

use crate::jsonparser;
use crate::lineprinter;
use crate::yamlparser;

#[cfg(feature = "sexp")]
use crate::jsonstringunescaper::{unsafe_unescape_json_string, UnescapeError};

pub type Index = usize;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OptionIndex {
    Nil,
    Index(Index),
}

impl OptionIndex {
    pub fn is_nil(&self) -> bool {
        matches!(self, OptionIndex::Nil)
    }

    pub fn is_some(&self) -> bool {
        !self.is_nil()
    }

    pub fn unwrap(&self) -> Index {
        match self {
            OptionIndex::Nil => panic!("Called .unwrap() on Nil OptionIndex"),
            OptionIndex::Index(i) => *i,
        }
    }
}

pub const NIL: usize = usize::MAX;

impl From<usize> for OptionIndex {
    fn from(i: usize) -> Self {
        if i == NIL {
            OptionIndex::Nil
        } else {
            OptionIndex::Index(i)
        }
    }
}

#[derive(PartialEq, Copy, Clone)]
pub enum PathType {
    Dot,
    Bracket,
    Query,
    // Just used for the status bar.
    DotWithTopLevelIndex,
}

#[derive(Debug)]
pub struct FlatJson(
    pub Vec<Row>,
    // Single-line pretty printed version of the JSON.
    // Rows will contain references into this.
    pub String,
    // Max nesting depth.
    pub usize,
);

impl FlatJson {
    pub fn last_visible_index(&self) -> Index {
        let last_index = self.0.len() - 1;

        let row = &self.0[last_index];

        if row.is_container() && row.is_collapsed() {
            row.pair_index().unwrap()
        } else {
            last_index
        }
    }

    pub fn last_visible_item(&self) -> Index {
        let mut last_index = self.0.len() - 1;

        loop {
            let row = &self.0[last_index];

            if row.is_primitive() {
                return last_index;
            }

            if row.is_closing_of_container() && row.is_collapsed() {
                return row.pair_index().unwrap();
            }

            last_index -= 1;
        }
    }

    pub fn prev_visible_row(&self, index: Index) -> OptionIndex {
        if index == 0 {
            return OptionIndex::Nil;
        }

        let row = &self.0[index - 1];

        if row.is_closing_of_container() && row.is_collapsed() {
            row.pair_index()
        } else {
            OptionIndex::Index(index - 1)
        }
    }

    pub fn next_visible_row(&self, mut index: Index) -> OptionIndex {
        // If row is collapsed container, jump to closing char and move past there.
        if self.0[index].is_opening_of_container() && self.0[index].is_collapsed() {
            index = self.0[index].pair_index().unwrap();
        }

        // We can always go to the next row, unless we're at the end of the file.
        if index == self.0.len() - 1 {
            return OptionIndex::Nil;
        }

        OptionIndex::Index(index + 1)
    }

    pub fn prev_item(&self, mut index: Index) -> OptionIndex {
        while let OptionIndex::Index(i) = self.prev_visible_row(index) {
            if !self.0[i].is_closing_of_container() {
                return OptionIndex::Index(i);
            }

            index = i;
        }

        OptionIndex::Nil
    }

    pub fn next_item(&self, mut index: Index) -> OptionIndex {
        while let OptionIndex::Index(i) = self.next_visible_row(index) {
            if !self.0[i].is_closing_of_container() {
                return OptionIndex::Index(i);
            }

            index = i;
        }

        OptionIndex::Nil
    }

    pub fn expand(&mut self, index: Index) {
        if let OptionIndex::Index(pair) = self.0[index].pair_index() {
            self.0[pair].expand();
        }
        self.0[index].expand();
    }

    pub fn collapse(&mut self, index: Index) {
        if let OptionIndex::Index(pair) = self.0[index].pair_index() {
            self.0[pair].collapse();
        }
        self.0[index].collapse();
    }

    pub fn toggle_collapsed(&mut self, index: Index) {
        if let OptionIndex::Index(pair) = self.0[index].pair_index() {
            self.0[pair].toggle_collapsed();
        }
        self.0[index].toggle_collapsed();
    }

    pub fn first_visible_ancestor(&self, mut index: Index) -> Index {
        let mut visible_ancestor = index;
        while let OptionIndex::Index(parent) = self[index].parent {
            if self[parent].is_collapsed() {
                visible_ancestor = parent;
            }
            index = parent;
        }
        visible_ancestor
    }

    pub fn build_path_to_node(&self, path_type: PathType, index: Index) -> Result<String, String> {
        let mut buf = String::new();

        // Some special handling for top-level elements.
        if self[index].parent.is_nil() {
            match path_type {
                PathType::Dot | PathType::Bracket => {
                    return Err("Cannot build path to top-level element".to_string());
                }
                PathType::Query => {
                    return Ok(".".to_string());
                }
                PathType::DotWithTopLevelIndex => { /* Handled in impl */ }
            }
        }

        self.build_path_to_node_impl(path_type, index, &mut buf)?;
        Ok(buf)
    }

    fn build_path_to_node_impl(
        &self,
        path_type: PathType,
        index: Index,
        buf: &mut String,
    ) -> Result<(), String> {
        let row = &self[index];

        if row.is_closing_of_container() {
            return self.build_path_to_node_impl(path_type, row.pair_index().unwrap(), buf);
        }

        if let OptionIndex::Index(parent_index) = row.parent {
            self.build_path_to_node_impl(path_type, parent_index, buf)?;
        }

        let res = if let Some(key_range) = &row.key_range {
            let key_open_delimiter = &self.1[key_range.start..key_range.start + 1];
            let key = &self.1[key_range.start + 1..key_range.end - 1];

            // For non-string keys in YAML.
            if key_open_delimiter == "[" {
                if path_type == PathType::Query {
                    return Err(
                        "Path to node contains non-string keys not supported in JSON".to_string(),
                    );
                }

                write!(buf, "[{key}]")
            } else {
                if path_type != PathType::Bracket && lineprinter::JS_IDENTIFIER.is_match(key) {
                    write!(buf, ".{key}")
                } else {
                    if path_type == PathType::Query && row.depth == 1 {
                        // Handle square brackets as the first part of the path.
                        write!(buf, ".[\"{key}\"]")
                    } else {
                        write!(buf, "[\"{key}\"]")
                    }
                }
            }
        } else {
            if row.parent.is_nil() {
                // We only print the top level index for this PathType,
                // but we don't print it out if there's only a single
                // top-level element.
                if path_type == PathType::DotWithTopLevelIndex
                    && (index != 0 || row.next_sibling.is_some())
                {
                    write!(buf, "[{}]", row.index_in_parent)
                } else {
                    Ok(())
                }
            } else {
                match path_type {
                    PathType::Query => {
                        if row.depth == 1 {
                            // Handle square brackets as the first part of the path.
                            write!(buf, ".[]")
                        } else {
                            write!(buf, "[]")
                        }
                    }
                    _ => write!(buf, "[{}]", row.index_in_parent),
                }
            }
        };

        res.map_err(|e| e.to_string())
    }

    pub fn pretty_printed(&self) -> String {
        let mut buf = String::new();

        for row in self.0.iter() {
            for _ in 0..row.depth {
                buf.push_str("  ");
            }
            if let Some(ref key_range) = row.key_range {
                buf.push_str(&self.1[key_range.clone()]);
                buf.push_str(": ");
            }
            let mut trailing_comma = row.parent.is_some() && row.next_sibling.is_some();
            if let Some(container_type) = row.value.container_type() {
                if row.value.is_opening_of_container() {
                    buf.push_str(container_type.open_str());
                    // Don't print trailing commas after { or [.
                    trailing_comma = false;
                } else {
                    buf.push_str(container_type.close_str());
                    // Check container opening to see if we have a next sibling.
                    trailing_comma = row.parent.is_some()
                        && self[row.pair_index().unwrap()].next_sibling.is_some();
                }
            } else {
                buf.push_str(&self.1[row.range.clone()]);
            }
            if trailing_comma {
                buf.push(',');
            }
            buf.push('\n');
        }

        buf
    }

    #[cfg(feature = "sexp")]
    fn sexp_atom_needs_escaping(s: &str) -> bool {
        // See: https://github.com/janestreet/sexplib0/blob/master/src/sexp.ml#L58
        if s.len() == 0 {
            return true;
        }

        let bytes = s.as_bytes();
        let last_ch_index = bytes.len() - 1;
        for (i, ch) in bytes.iter().enumerate() {
            match ch {
                // sexp syntactical characters must be escaped
                b' ' | b'"' | b'(' | b')' | b';' | b'\\' => return true,
                // The start or end of a multiline comment "#| comment |#" must be escaped
                b'|' => {
                    if i < last_ch_index && bytes[i + 1] == b'#' {
                        return true;
                    }
                }
                b'#' => {
                    if i < last_ch_index && bytes[i + 1] == b'|' {
                        return true;
                    }
                }
                // sexplib0 source matches [0 .. 32]. 32 is space, and I included
                // that above more explicitly.
                0..=31 | 127..=255 => return true,
                _ => (),
            }
        }

        false
    }

    #[cfg(feature = "sexp")]
    fn escape_and_write_sexp_atom(buf: &mut String, atom: &str) {
        // https://github.com/janestreet/sexplib0/blob/master/src/sexp.ml#L81-L132
        buf.push('"');
        for ch in atom.bytes() {
            match ch {
                // Double quote and backslashes are escaped
                b'"' => buf.push_str(r#"\""#),
                b'\\' => buf.push_str(r#"\\"#),
                // White space get special escape codes
                b'\n' => buf.push_str(r#"\n"#),
                b'\t' => buf.push_str(r#"\t"#),
                b'\r' => buf.push_str(r#"\r"#),
                8 /* backspace */ => buf.push_str(r#"\b"#),
                32..=126 => buf.push(ch as char),
                _ => {
                    buf.push('\\');
                    let zero = b'0';
                    buf.push((zero + (ch / 100)) as char);
                    buf.push((zero + ((ch / 10) % 10)) as char);
                    buf.push((zero + (ch % 10)) as char);
                }
            }
        }
        buf.push('"');
    }

    #[cfg(feature = "sexp")]
    fn write_sexp_atom(&self, buf: &mut String, range: Range<usize>) -> Result<(), UnescapeError> {
        let quoteless_range = (range.start + 1)..(range.end - 1);
        let string_value = &self.1[quoteless_range];

        match unsafe_unescape_json_string(string_value) {
            Ok(unescaped) => {
                if Self::sexp_atom_needs_escaping(&unescaped) {
                    Self::escape_and_write_sexp_atom(buf, &unescaped);
                } else {
                    buf.push_str(&unescaped);
                }
                Ok(())
            }
            Err(err) => Err(err),
        }
    }

    #[cfg(feature = "sexp")]
    pub fn sexp_string(&self) -> Result<String, UnescapeError> {
        let mut buf = String::new();

        for row in self.0.iter() {
            // Write a space between elements
            if row.parent.is_some() && row.prev_sibling.is_some() {
                buf.push(' ');
            }

            // Write start of key-value tuple
            if let Some(ref key_range) = row.key_range {
                buf.push('(');
                self.write_sexp_atom(&mut buf, key_range.clone())?;
                buf.push(' ');
            }

            match &row.value {
                Value::Null | Value::EmptyObject | Value::EmptyArray => buf.push_str("()"),
                Value::Boolean | Value::Number => buf.push_str(&self.1[row.range.clone()]),
                Value::String => self.write_sexp_atom(&mut buf, row.range.clone())?,
                Value::OpenContainer { .. } => buf.push('('),
                Value::CloseContainer { .. } => buf.push(')'),
            }

            // Close key-value tuple if we wrote a primitive. If we wrote the closing of a
            // container, check if the opening had a key, and close it.
            if row.is_primitive() && row.key_range.is_some() {
                buf.push(')');
            } else if row.is_closing_of_container() {
                let opening_of_container_row = &self.0[row.pair_index().unwrap()];
                if opening_of_container_row.key_range.is_some() {
                    buf.push(')');
                }
            }

            // Write newline after every top-level sexp.
            if row.value.is_closing_of_container() && row.parent.is_nil() {
                buf.push('\n');
            }
        }

        Ok(buf)
    }

    // A lot of the code here is almost identical to pretty_printed, but
    // there are some subtle enough differences, and the code isn't that
    // complicated, that I don't think it's worth it to try to have them
    // share an implementation.
    pub fn pretty_printed_value(&self, value_index: Index) -> Result<String, std::fmt::Error> {
        if self[value_index].is_primitive() {
            return Ok(self.1[self[value_index].range.clone()].to_string());
        }

        let mut buf = String::new();

        let container_type = self[value_index].value.container_type().unwrap();
        let depth_offset = self[value_index].depth;
        let pair_index = self[value_index].pair_index().unwrap();

        let start_index = value_index.min(pair_index);
        let end_index = value_index.max(pair_index);

        writeln!(buf, "{}", container_type.open_str())?;

        for index in start_index + 1..end_index {
            let row = &self[index];
            for _ in 0..(row.depth - depth_offset) {
                write!(buf, "  ")?;
            }
            if let Some(ref key_range) = row.key_range {
                write!(buf, "{}: ", &self.1[key_range.clone()])?;
            }
            let mut trailing_comma = row.parent.is_some() && row.next_sibling.is_some();
            if let Some(container_type) = row.value.container_type() {
                if row.value.is_opening_of_container() {
                    write!(buf, "{}", container_type.open_str())?;
                    // Don't print trailing commas after { or [.
                    trailing_comma = false;
                } else {
                    write!(buf, "{}", container_type.close_str())?;
                    // Check container opening to see if we have a next sibling.
                    trailing_comma = row.parent.is_some()
                        && self[row.pair_index().unwrap()].next_sibling.is_some();
                }
            } else {
                write!(buf, "{}", &self.1[row.range.clone()])?;
            }
            if trailing_comma {
                write!(buf, ",")?;
            }
            writeln!(buf)?;
        }

        writeln!(buf, "{}", container_type.close_str())?;

        Ok(buf)
    }
}

impl std::ops::Index<usize> for FlatJson {
    type Output = Row;

    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl std::ops::IndexMut<usize> for FlatJson {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

#[derive(Debug)]
pub struct Row {
    pub parent: OptionIndex,
    // Should these also be set on the CloseContainers?
    pub prev_sibling: OptionIndex,
    pub next_sibling: OptionIndex,

    pub depth: usize,
    pub index_in_parent: usize,
    pub range: Range<usize>,
    pub key_range: Option<Range<usize>>,
    pub value: Value,
}

impl Row {
    pub fn is_primitive(&self) -> bool {
        self.value.is_primitive()
    }
    pub fn is_container(&self) -> bool {
        self.value.is_container()
    }
    pub fn is_string(&self) -> bool {
        self.value.is_string()
    }
    pub fn is_opening_of_container(&self) -> bool {
        self.value.is_opening_of_container()
    }
    pub fn is_closing_of_container(&self) -> bool {
        self.value.is_closing_of_container()
    }
    pub fn is_collapsed(&self) -> bool {
        self.value.is_collapsed()
    }
    pub fn is_expanded(&self) -> bool {
        self.value.is_expanded()
    }
    pub fn is_array(&self) -> bool {
        self.value.is_array()
    }

    fn expand(&mut self) {
        self.value.expand()
    }
    fn collapse(&mut self) {
        self.value.collapse()
    }
    fn toggle_collapsed(&mut self) {
        self.value.toggle_collapsed()
    }

    pub fn first_child(&self) -> OptionIndex {
        self.value.first_child()
    }
    pub fn last_child(&self) -> OptionIndex {
        self.value.last_child()
    }
    pub fn pair_index(&self) -> OptionIndex {
        self.value.pair_index()
    }

    // The range of what the row represents on the screen. If the row is
    // a container, and it is collapsed, this includes the entire range
    // of the container, but if it is expanded, it just represents the
    // single opening character.
    //
    // This also includes the key range for objects.
    pub fn range_represented_by_row(&self) -> Range<usize> {
        let start = match &self.key_range {
            Some(key_range) => key_range.start,
            None => self.range.start,
        };

        let end = if self.is_container() && self.is_expanded() {
            self.range.start + 1
        } else {
            self.range.end
        };

        start..end
    }
}

#[derive(Copy, Clone, Debug)]
pub enum ContainerType {
    Object,
    Array,
}

impl ContainerType {
    pub fn open_str(&self) -> &'static str {
        match self {
            ContainerType::Object => "{",
            ContainerType::Array => "[",
        }
    }

    pub fn close_str(&self) -> &'static str {
        match self {
            ContainerType::Object => "}",
            ContainerType::Array => "]",
        }
    }

    pub fn collapsed_preview(&self) -> &'static str {
        match self {
            ContainerType::Object => "{…}",
            ContainerType::Array => "[…]",
        }
    }
}

#[derive(Debug)]
pub enum Value {
    Null,
    Boolean,
    Number,
    String,
    EmptyObject,
    EmptyArray,
    OpenContainer {
        container_type: ContainerType,
        collapsed: bool,
        first_child: Index,
        close_index: Index,
    },
    CloseContainer {
        container_type: ContainerType,
        collapsed: bool,
        last_child: Index,
        open_index: Index,
    },
}

impl Value {
    pub fn is_primitive(&self) -> bool {
        !self.is_container()
    }

    pub fn is_container(&self) -> bool {
        matches!(
            self,
            Value::OpenContainer { .. } | Value::CloseContainer { .. }
        )
    }

    pub fn is_string(&self) -> bool {
        matches!(self, Value::String)
    }

    pub fn container_type(&self) -> Option<ContainerType> {
        match self {
            Value::OpenContainer { container_type, .. } => Some(*container_type),
            Value::CloseContainer { container_type, .. } => Some(*container_type),
            _ => None,
        }
    }

    pub fn is_opening_of_container(&self) -> bool {
        matches!(self, Value::OpenContainer { .. })
    }

    pub fn is_closing_of_container(&self) -> bool {
        matches!(self, Value::CloseContainer { .. })
    }

    pub fn is_collapsed(&self) -> bool {
        match self {
            Value::OpenContainer { collapsed, .. } => *collapsed,
            Value::CloseContainer { collapsed, .. } => *collapsed,
            _ => false,
        }
    }

    pub fn is_expanded(&self) -> bool {
        !self.is_collapsed()
    }

    pub fn is_array(&self) -> bool {
        matches!(
            self,
            Value::OpenContainer {
                container_type: ContainerType::Array,
                ..
            } | Value::CloseContainer {
                container_type: ContainerType::Array,
                ..
            }
        )
    }

    fn toggle_collapsed(&mut self) {
        self.set_collapsed(!self.is_collapsed())
    }

    fn expand(&mut self) {
        self.set_collapsed(false)
    }

    fn collapse(&mut self) {
        self.set_collapsed(true)
    }

    fn set_collapsed(&mut self, val: bool) {
        match self {
            Value::OpenContainer {
                ref mut collapsed, ..
            } => *collapsed = val,
            Value::CloseContainer {
                ref mut collapsed, ..
            } => *collapsed = val,
            _ => {}
        }
    }

    fn first_child(&self) -> OptionIndex {
        match self {
            Value::OpenContainer { first_child, .. } => OptionIndex::Index(*first_child),
            _ => OptionIndex::Nil,
        }
    }

    fn last_child(&self) -> OptionIndex {
        match self {
            Value::CloseContainer { last_child, .. } => OptionIndex::Index(*last_child),
            _ => OptionIndex::Nil,
        }
    }

    fn pair_index(&self) -> OptionIndex {
        match self {
            Value::OpenContainer { close_index, .. } => OptionIndex::Index(*close_index),
            Value::CloseContainer { open_index, .. } => OptionIndex::Index(*open_index),
            _ => OptionIndex::Nil,
        }
    }
}

pub fn parse_top_level_json(json: String) -> Result<FlatJson, String> {
    let (rows, pretty, depth) = jsonparser::parse(json)?;
    Ok(FlatJson(rows, pretty, depth))
}

pub fn parse_top_level_yaml(yaml: String) -> Result<FlatJson, String> {
    let (rows, pretty, depth) = yamlparser::parse(yaml)?;
    Ok(FlatJson(rows, pretty, depth))
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: &str = r#"{
        "1": 1,
        "2": [
            3,
            "4"
        ],
        "6": {
            "7": null,
            "8": true,
            "9": 9
        },
        "11": 11
    }"#;

    const NESTED_OBJECT: &str = r#"[
        {
            "2": [
                3
            ],
            "5": {
                "6": 6
            }
        }
    ]"#;

    #[test]
    fn test_flatten_json() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();

        assert_flat_json_fields(
            "parent",
            &fj,
            vec![NIL, 0, 0, 2, 2, 0, 0, 6, 6, 6, 0, 0, NIL],
            |elem| elem.parent,
        );

        assert_flat_json_fields(
            "prev_sibling",
            &fj,
            vec![NIL, NIL, 1, NIL, 3, NIL, 2, NIL, 7, 8, NIL, 6, NIL],
            |elem| elem.prev_sibling,
        );

        assert_flat_json_fields(
            "next_sibling",
            &fj,
            vec![NIL, 2, 6, 4, NIL, NIL, 11, 8, 9, NIL, NIL, NIL, NIL],
            |elem| elem.next_sibling,
        );

        assert_flat_json_fields(
            "first_child",
            &fj,
            vec![1, NIL, 3, NIL, NIL, NIL, 7, NIL, NIL, NIL, NIL, NIL, NIL],
            |elem| elem.first_child(),
        );

        assert_flat_json_fields(
            "last_child",
            &fj,
            vec![NIL, NIL, NIL, NIL, NIL, 4, NIL, NIL, NIL, NIL, 9, NIL, 11],
            |elem| elem.last_child(),
        );

        assert_flat_json_fields(
            "{open,close}_index",
            &fj,
            vec![12, NIL, 5, NIL, NIL, 2, 10, NIL, NIL, NIL, 6, NIL, 0],
            |elem| elem.pair_index(),
        );

        assert_flat_json_fields(
            "depth",
            &fj,
            vec![0, 1, 1, 2, 2, 1, 1, 2, 2, 2, 1, 1, 0],
            |elem| OptionIndex::Index(elem.depth),
        );
    }

    fn assert_flat_json_fields<T: Into<OptionIndex> + Debug + Copy>(
        field: &'static str,
        fj: &FlatJson,
        field_values: Vec<T>,
        accessor_fn: fn(&Row) -> OptionIndex,
    ) {
        assert_eq!(
            fj.0.len(),
            field_values.len(),
            "length of flat json and field_values don't match",
        );

        for (i, (elem, expected_value)) in fj.0.iter().zip(field_values.iter()).enumerate() {
            assert_eq!(
                accessor_fn(elem),
                Into::<OptionIndex>::into(*expected_value),
                "incorrect {field} at index {i}",
            );
        }
    }

    #[test]
    fn test_first_visible_ancestor() {
        let mut fj = parse_top_level_json(NESTED_OBJECT.to_owned()).unwrap();
        assert_eq!(fj.first_visible_ancestor(3), 3);
        assert_eq!(fj.first_visible_ancestor(6), 6);
        fj.collapse(5);
        assert_eq!(fj.first_visible_ancestor(6), 5);
        assert_eq!(fj.first_visible_ancestor(5), 5);
        fj.collapse(1);
        assert_eq!(fj.first_visible_ancestor(6), 1);
        fj.expand(5);
        assert_eq!(fj.first_visible_ancestor(6), 1);
        fj.collapse(0);
        assert_eq!(fj.first_visible_ancestor(6), 0);
    }

    #[test]
    fn test_move_by_visible_rows_simple() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();

        assert_visited_rows(&fj, vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, NIL]);
    }

    #[test]
    fn test_move_by_visible_rows_collapsed() {
        let mut fj = parse_top_level_json(NESTED_OBJECT.to_owned()).unwrap();

        fj.collapse(2);
        assert_visited_rows(&fj, vec![1, 2, 5, 6, 7, 8, 9, NIL]);

        fj.collapse(5);
        assert_visited_rows(&fj, vec![1, 2, 5, 8, 9, NIL]);

        fj.collapse(1);
        assert_visited_rows(&fj, vec![1, 9, NIL]);

        fj.collapse(0);
        assert_visited_rows(&fj, vec![NIL]);
    }

    #[test]
    fn test_move_by_items_simple() {
        let fj = parse_top_level_json(OBJECT.to_owned()).unwrap();
        assert_visited_items(&fj, vec![1, 2, 3, 4, 6, 7, 8, 9, 11, NIL]);

        let fj = parse_top_level_json(NESTED_OBJECT.to_owned()).unwrap();
        assert_visited_items(&fj, vec![1, 2, 3, 5, 6, NIL]);
    }

    #[test]
    fn test_move_by_items_collapsed() {
        let mut fj = parse_top_level_json(NESTED_OBJECT.to_owned()).unwrap();

        fj.collapse(2);
        assert_visited_items(&fj, vec![1, 2, 5, 6, NIL]);

        fj.collapse(5);
        assert_visited_items(&fj, vec![1, 2, 5, NIL]);

        fj.expand(2);
        assert_visited_items(&fj, vec![1, 2, 3, 5, NIL]);

        fj.collapse(1);
        assert_visited_items(&fj, vec![1, NIL]);

        fj.collapse(0);
        assert_visited_items(&fj, vec![NIL]);
    }

    fn assert_row_iter(
        movement_name: &'static str,
        fj: &FlatJson,
        start_index: Index,
        expected_visited_rows: &Vec<usize>,
        movement_fn: fn(&FlatJson, Index) -> OptionIndex,
    ) {
        let mut curr_index = start_index;
        for expected_index in expected_visited_rows.iter() {
            let next_index = movement_fn(fj, curr_index).to_usize();

            assert_eq!(
                next_index,
                *expected_index,
                "expected {}({}) to be {:?}",
                movement_name,
                curr_index,
                OptionIndex::from(*expected_index),
            );

            curr_index = next_index;
        }
    }

    fn assert_visited_rows(fj: &FlatJson, mut expected: Vec<usize>) {
        assert_next_visited_rows(fj, 0, &expected);
        let mut start = 0;
        if expected.len() > 1 {
            // Want to turn: 0; [1, 2, 3, 4, NIL]
            // into:         4; [3, 2, 1, 0, NIL]
            start = expected[expected.len() - 2];
            expected.pop();
            expected.pop();
            expected.reverse();
            expected.push(0);
            expected.push(NIL);
        }
        assert_prev_visited_rows(fj, start, &expected);
    }

    fn assert_next_visited_rows(fj: &FlatJson, start_index: Index, expected: &Vec<usize>) {
        assert_row_iter(
            "next_visible_row",
            fj,
            start_index,
            expected,
            FlatJson::next_visible_row,
        );
    }

    fn assert_prev_visited_rows(fj: &FlatJson, start_index: Index, expected: &Vec<usize>) {
        assert_row_iter(
            "prev_visible_row",
            fj,
            start_index,
            expected,
            FlatJson::prev_visible_row,
        );
    }

    fn assert_visited_items(fj: &FlatJson, mut expected: Vec<usize>) {
        assert_next_visited_items(fj, 0, &expected);
        let mut start = 0;
        if expected.len() > 1 {
            // Want to turn: 0; [1, 2, 3, 4, NIL]
            // into:         4; [3, 2, 1, 0, NIL]
            start = expected[expected.len() - 2];
            expected.pop();
            expected.pop();
            expected.reverse();
            expected.push(0);
            expected.push(NIL);
        }
        assert_prev_visited_items(fj, start, &expected);
    }

    fn assert_next_visited_items(fj: &FlatJson, start_index: Index, expected: &Vec<usize>) {
        assert_row_iter("next_item", fj, start_index, expected, FlatJson::next_item);
    }

    fn assert_prev_visited_items(fj: &FlatJson, start_index: Index, expected: &Vec<usize>) {
        assert_row_iter("prev_item", fj, start_index, expected, FlatJson::prev_item);
    }

    #[test]
    fn test_root_object_build_path_to_node() {
        use PathType::*;

        const ROOT_OBJECT: &str = r#"{
            "non js key": 1,
            "plain_key": [
                {},
                {
                    "nested": 5,
                },
            ],
        }"#;

        let fj = parse_top_level_json(ROOT_OBJECT.to_owned()).unwrap();

        assert!(fj.build_path_to_node(Dot, 0).is_err());
        assert!(fj.build_path_to_node(Bracket, 0).is_err());
        assert_eq!(".", fj.build_path_to_node(Query, 0).unwrap());
        assert_eq!("", fj.build_path_to_node(DotWithTopLevelIndex, 0).unwrap());

        let path = r#"["non js key"]"#;
        let paths = (path, path, r#".["non js key"]"#, path);
        assert_paths_to_node(&fj, 1, paths);

        let nested_paths = (
            ".plain_key[1].nested",
            r#"["plain_key"][1]["nested"]"#,
            ".plain_key[].nested",
            ".plain_key[1].nested",
        );
        assert_paths_to_node(&fj, 5, nested_paths);
    }

    #[test]
    fn test_root_array_build_path_to_node() {
        use PathType::*;

        const ROOT_ARRAY: &str = r#"[
            1,
            {
                "nested": {
                    "more nested": 4,
                },
            },
        ]"#;

        let fj = parse_top_level_json(ROOT_ARRAY.to_owned()).unwrap();

        assert!(fj.build_path_to_node(Dot, 0).is_err());
        assert!(fj.build_path_to_node(Bracket, 0).is_err());
        assert_eq!(".", fj.build_path_to_node(Query, 0).unwrap());
        assert_eq!("", fj.build_path_to_node(DotWithTopLevelIndex, 0).unwrap());

        let paths = ("[0]", "[0]", ".[]", "[0]");
        assert_paths_to_node(&fj, 1, paths);

        let nested_paths = (
            r#"[1].nested["more nested"]"#,
            r#"[1]["nested"]["more nested"]"#,
            r#".[].nested["more nested"]"#,
            r#"[1].nested["more nested"]"#,
        );
        assert_paths_to_node(&fj, 4, nested_paths);
    }

    #[test]
    fn test_multi_top_level_build_path_to_node() {
        use PathType::*;

        const MULTI_TOP_LEVEL: &str = r#"[
            {
                "nested": [
                    3,
                ],
            },
        ]
        {
            "plain_key": [
                {
                    "nested": 10,
                },
            ],
        }"#;

        let fj = parse_top_level_json(MULTI_TOP_LEVEL.to_owned()).unwrap();

        assert!(fj.build_path_to_node(Dot, 0).is_err());
        assert!(fj.build_path_to_node(Bracket, 0).is_err());
        assert_eq!(".", fj.build_path_to_node(Query, 0).unwrap());
        assert_eq!(
            "[0]",
            fj.build_path_to_node(DotWithTopLevelIndex, 0).unwrap()
        );

        assert!(fj.build_path_to_node(Dot, 7).is_err());
        assert!(fj.build_path_to_node(Bracket, 7).is_err());
        assert_eq!(".", fj.build_path_to_node(Query, 7).unwrap());
        assert_eq!(
            "[1]",
            fj.build_path_to_node(DotWithTopLevelIndex, 7).unwrap()
        );

        let paths = (
            "[0].nested[0]",
            r#"[0]["nested"][0]"#,
            ".[].nested[]",
            "[0][0].nested[0]",
        );
        assert_paths_to_node(&fj, 3, paths);

        let paths = (
            ".plain_key[0].nested",
            r#"["plain_key"][0]["nested"]"#,
            ".plain_key[].nested",
            "[1].plain_key[0].nested",
        );
        assert_paths_to_node(&fj, 10, paths);
    }

    #[test]
    fn test_build_path_to_node_yaml_non_string_key() {
        use PathType::*;

        const YAML: &str = r#"{
            [1, 1]: 1,
        }"#;
        let fj = parse_top_level_yaml(YAML.to_owned()).unwrap();
        assert_eq!("[[1, 1]]", fj.build_path_to_node(Dot, 1).unwrap());
        assert_eq!("[[1, 1]]", fj.build_path_to_node(Bracket, 1).unwrap());
        assert!(fj.build_path_to_node(Query, 1).is_err());
    }

    #[track_caller]
    fn assert_paths_to_node(fj: &FlatJson, index: Index, paths: (&str, &str, &str, &str)) {
        use PathType::*;

        let dot = fj.build_path_to_node(Dot, index).unwrap();
        let bracket = fj.build_path_to_node(Bracket, index).unwrap();
        let query = fj.build_path_to_node(Query, index).unwrap();
        let dot_top_level = fj.build_path_to_node(DotWithTopLevelIndex, index).unwrap();

        assert_eq!(
            paths,
            (
                dot.as_str(),
                bracket.as_str(),
                query.as_str(),
                dot_top_level.as_str()
            )
        );
    }

    #[test]
    fn test_pretty_print() {
        const JSON: &str = r#"{"a":1,"b":[2,{},[],false],"c":null}
            [ "d"   , [1,{      "e" :   7 }]   ]"#;
        const PRETTY: &str = r#"{
  "a": 1,
  "b": [
    2,
    {},
    [],
    false
  ],
  "c": null
}
[
  "d",
  [
    1,
    {
      "e": 7
    }
  ]
]
"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();
        assert_eq!(PRETTY, fj.pretty_printed());
    }

    #[test]
    #[cfg(feature = "sexp")]
    fn test_sexp_string() {
        const JSON: &str = r#"{"a":1,"b":[2,{},[],false],"c":null}
            [ "d"   , [1,{      "e" :   7 }]   ]"#;
        const PRETTY: &str = r#"((a 1) (b (2 () () false)) (c ()))
(d (1 ((e 7))))
"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();
        assert_eq!(PRETTY, fj.sexp_string().unwrap());
    }

    #[test]
    #[cfg(feature = "sexp")]
    fn test_sexp_string_escaping() {
        const JSON: &str = r#"["", "a b", "a\"b", "a\\b", "a#|b", "a|#b", "a;b", {"a(b": "a)b"}]
            ["\n\t\r\b", "\u0000\u001f\u007f"]"#;
        const PRETTY: &str = r#"("" "a b" "a\"b" "a\\b" "a#|b" "a|#b" "a;b" (("a(b" "a)b")))
("\n\t\r\b" "\000\031\127")
"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();
        assert_eq!(PRETTY, fj.sexp_string().unwrap());
    }

    #[test]
    fn test_pretty_printed_value() {
        const JSON: &str = r#"[[{"3":3,"4":[5, 6, {"8": false}]}]]"#;
        let fj = parse_top_level_json(JSON.to_owned()).unwrap();
        const PRETTY_INNER_OBJ: &str = r#"{
  "3": 3,
  "4": [
    5,
    6,
    {
      "8": false
    }
  ]
}
"#;
        assert_eq!(PRETTY_INNER_OBJ, fj.pretty_printed_value(2).unwrap());
        assert_eq!("3", fj.pretty_printed_value(3).unwrap());

        const PRETTY_ARRAY: &str = r#"[
  5,
  6,
  {
    "8": false
  }
]
"#;
        assert_eq!(PRETTY_ARRAY, fj.pretty_printed_value(4).unwrap());
        assert_eq!("6", fj.pretty_printed_value(6).unwrap());

        const PRETTY_NESTED_OBJ: &str = "{\n  \"8\": false\n}\n";
        assert_eq!(PRETTY_NESTED_OBJ, fj.pretty_printed_value(7).unwrap());
    }
}
