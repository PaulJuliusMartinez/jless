use serde_json::value::{Number, Value as SerdeValue};
use std::fmt::Debug;

pub type Index = usize;

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum OptionIndex {
    Nil,
    Index(Index),
}

impl OptionIndex {
    pub fn is_nil(&self) -> bool {
        match self {
            OptionIndex::Nil => true,
            _ => false,
        }
    }

    pub fn unwrap(&self) -> Index {
        match self {
            OptionIndex::Nil => panic!("Called .unwrap() on Nil OptionIndex"),
            OptionIndex::Index(i) => *i,
        }
    }

    pub fn to_usize(&self) -> usize {
        match self {
            OptionIndex::Nil => NIL,
            OptionIndex::Index(i) => *i,
        }
    }

    pub fn from_usize(i: usize) -> OptionIndex {
        if i == NIL {
            OptionIndex::Nil
        } else {
            OptionIndex::Index(i)
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
pub struct FlatJson(Vec<Row>);

impl FlatJson {
    pub fn last_visible_index(&self) -> Index {
        self.0.len() - 1
    }

    pub fn last_visible_item(&self) -> Index {
        let last_index = self.0.len() - 1;
        // If it's a primitve, we can just return the last index
        if self.0[last_index].is_primitive() {
            return last_index;
        }
        // Otherwise, it's definitely the closing brace of a container, so
        // we can just move backwards to the last_visible_item.
        return self.prev_item(last_index).unwrap();
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

    pub fn inner_item(&self, index: Index) -> OptionIndex {
        match &self.0[index].value {
            Value::OpenContainer { first_child, .. } => OptionIndex::Index(*first_child),
            Value::CloseContainer { last_child, .. } => OptionIndex::Index(*last_child),
            _ => OptionIndex::Nil,
        }
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
    prev_sibling: OptionIndex,
    next_sibling: OptionIndex,

    depth: usize,
    index: Index,
    // start_index: usize
    // end_index: usize
    key: Option<String>,
    value: Value,
}

impl Row {
    pub fn is_primitive(&self) -> bool {
        self.value.is_primitive()
    }
    pub fn is_container(&self) -> bool {
        self.value.is_container()
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
}

#[derive(Debug)]
enum ContainerType {
    Object,
    Array,
}

#[derive(Debug)]
enum Value {
    Null,
    Boolean(bool),
    Number(Number),
    String(String),
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
        match self {
            Value::OpenContainer { .. } => true,
            Value::CloseContainer { .. } => true,
            _ => false,
        }
    }

    pub fn is_opening_of_container(&self) -> bool {
        match self {
            Value::OpenContainer { .. } => true,
            _ => false,
        }
    }

    pub fn is_closing_of_container(&self) -> bool {
        match self {
            Value::CloseContainer { .. } => true,
            _ => false,
        }
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

pub fn parse_top_level_json(json: String) -> serde_json::Result<FlatJson> {
    let serde_value = serde_json::from_str(&json)?;
    let mut flat_json = vec![];
    let mut parents = vec![OptionIndex::Nil];

    flatten_json(serde_value, &mut flat_json, &mut parents);

    Ok(FlatJson(flat_json))
}

fn flatten_json(serde_value: SerdeValue, flat_json: &mut Vec<Row>, parents: &mut Vec<OptionIndex>) {
    let depth = parents.len() - 1;
    let parent = *parents.last().unwrap();

    let row = Row {
        parent,
        prev_sibling: OptionIndex::Nil,
        next_sibling: OptionIndex::Nil,
        depth,
        index: 0,
        key: None,
        value: Value::Null,
    };

    match serde_value {
        SerdeValue::Null => flat_json.push(row),
        SerdeValue::Bool(b) => flat_json.push(Row {
            value: Value::Boolean(b),
            ..row
        }),
        SerdeValue::Number(n) => flat_json.push(Row {
            value: Value::Number(n),
            ..row
        }),
        SerdeValue::String(s) => flat_json.push(Row {
            value: Value::String(s),
            ..row
        }),
        SerdeValue::Array(vs) => {
            if vs.len() == 0 {
                flat_json.push(Row {
                    value: Value::EmptyArray,
                    ..row
                })
            } else {
                let open_index = flat_json.len();
                parents.push(OptionIndex::Index(open_index));

                flat_json.push(Row {
                    value: Value::OpenContainer {
                        container_type: ContainerType::Array,
                        collapsed: false,
                        first_child: open_index + 1,
                        // Set once done processing the array.
                        close_index: 0,
                    },
                    ..row
                });

                let mut prev_sibling: OptionIndex = OptionIndex::Nil;
                let mut child_index = 0;

                for (i, v) in vs.into_iter().enumerate() {
                    child_index = flat_json.len();

                    flatten_json(v, flat_json, parents);
                    let mut child = &mut flat_json[child_index];

                    child.index = i;
                    child.prev_sibling = prev_sibling;

                    if let OptionIndex::Index(prev_sibling_index) = prev_sibling {
                        flat_json[prev_sibling_index].next_sibling =
                            OptionIndex::Index(child_index);
                    }

                    prev_sibling = OptionIndex::Index(child_index);
                }

                let close_index = flat_json.len();
                flat_json.push(Row {
                    parent,
                    // Currently not set on the CloseContainer value.
                    prev_sibling: OptionIndex::Nil,
                    next_sibling: OptionIndex::Nil,
                    depth,
                    index: 0,
                    key: None,
                    value: Value::CloseContainer {
                        container_type: ContainerType::Array,
                        collapsed: false,
                        last_child: child_index,
                        // Set once done processing the array.
                        open_index,
                    },
                });

                if let Value::OpenContainer {
                    close_index: ref mut close_index_of_open_value,
                    ..
                } = &mut flat_json[open_index].value
                {
                    *close_index_of_open_value = close_index;
                }

                parents.pop();
            }
        }
        SerdeValue::Object(obj) => {
            if obj.len() == 0 {
                flat_json.push(Row {
                    value: Value::EmptyObject,
                    ..row
                })
            } else {
                let open_index = flat_json.len();
                parents.push(OptionIndex::Index(open_index));

                flat_json.push(Row {
                    value: Value::OpenContainer {
                        container_type: ContainerType::Object,
                        collapsed: false,
                        first_child: open_index + 1,
                        // Set once done processing the array.
                        close_index: 0,
                    },
                    ..row
                });

                let mut prev_sibling: OptionIndex = OptionIndex::Nil;
                let mut child_index = 0;

                for (i, (k, v)) in obj.into_iter().enumerate() {
                    child_index = flat_json.len();

                    flatten_json(v, flat_json, parents);
                    let mut child = &mut flat_json[child_index];

                    child.index = i;
                    child.prev_sibling = prev_sibling;
                    child.key = Some(k);

                    if let OptionIndex::Index(prev_sibling_index) = prev_sibling {
                        flat_json[prev_sibling_index].next_sibling =
                            OptionIndex::Index(child_index);
                    }

                    prev_sibling = OptionIndex::Index(child_index);
                }

                let close_index = flat_json.len();
                flat_json.push(Row {
                    parent,
                    // Currently not set on the CloseContainer value.
                    prev_sibling: OptionIndex::Nil,
                    next_sibling: OptionIndex::Nil,
                    depth,
                    index: 0,
                    key: None,
                    value: Value::CloseContainer {
                        container_type: ContainerType::Object,
                        collapsed: false,
                        last_child: child_index,
                        // Set once done processing the array.
                        open_index,
                    },
                });

                if let Value::OpenContainer {
                    close_index: ref mut close_index_of_open_value,
                    ..
                } = &mut flat_json[open_index].value
                {
                    *close_index_of_open_value = close_index;
                }

                parents.pop();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBJECT: &'static str = r#"{
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

    const OBJECT_LINES: usize = 13;

    const NESTED_OBJECT: &'static str = r#"[
        {
            "2": [
                3
            ],
            "5": {
                "6": 6
            }
        }
    ]"#;

    const NESTED_OBJECT_LINES: usize = 10;

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
