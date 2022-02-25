use std::fmt::Debug;
use std::ops::Range;

use crate::jsonparser;
use crate::yamlparser;

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
    pub index: Index,
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

    pub fn full_range(&self) -> Range<usize> {
        match &self.key_range {
            Some(key_range) => key_range.start..self.range.end,
            None => self.range.clone(),
        }
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
                "incorrect {} at index {}",
                field,
                i,
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
}
