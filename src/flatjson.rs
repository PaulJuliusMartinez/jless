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

    pub fn next_visible_row(&self, index: Index) -> OptionIndex {
        if index == self.0.len() - 1 {
            return OptionIndex::Nil;
        }

        // The next Row is ALWAYS visible.
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
    type Output = Row;

    fn index(&mut self, index: usize) -> &mut Self::Output {
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
            Value::OpenContainer { .. } => true,
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
        match &mut self {
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

    const OBJECT_1: &'static str = r#"{
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

    #[test]
    fn test_flatten_json() {
        let fj = parse_top_level_json(OBJECT_1.to_owned()).unwrap();

        assert_flat_json_indexes(
            "parent",
            &fj,
            vec![NIL, 0, 0, 2, 2, 0, 0, 6, 6, 6, 0, 0, NIL],
            |elem| elem.parent,
        );

        assert_flat_json_indexes(
            "prev_sibling",
            &fj,
            vec![NIL, NIL, 1, NIL, 3, NIL, 2, NIL, 7, 8, NIL, 6, NIL],
            |elem| elem.prev_sibling,
        );

        assert_flat_json_indexes(
            "next_sibling",
            &fj,
            vec![NIL, 2, 6, 4, NIL, NIL, 11, 8, 9, NIL, NIL, NIL, NIL],
            |elem| elem.next_sibling,
        );

        assert_flat_json_indexes(
            "first_child",
            &fj,
            vec![1, NIL, 3, NIL, NIL, NIL, 7, NIL, NIL, NIL, NIL, NIL, NIL],
            |elem| elem.first_child(),
        );

        assert_flat_json_indexes(
            "last_child",
            &fj,
            vec![NIL, NIL, NIL, NIL, NIL, 4, NIL, NIL, NIL, NIL, 9, NIL, 11],
            |elem| elem.last_child(),
        );

        assert_flat_json_indexes(
            "{open,close}_index",
            &fj,
            vec![12, NIL, 5, NIL, NIL, 2, 10, NIL, NIL, NIL, 6, NIL, 0],
            |elem| elem.pair_index(),
        );
    }

    fn assert_flat_json_indexes<T: Into<OptionIndex> + Debug + Copy>(
        field: &'static str,
        fj: &FlatJson,
        indexes: Vec<T>,
        accessor_fn: fn(&Row) -> OptionIndex,
    ) {
        assert_eq!(
            fj.0.len(),
            indexes.len(),
            "length of flat json and indexes don't match",
        );

        for (i, (elem, expected_index)) in fj.0.iter().zip(indexes.iter()).enumerate() {
            assert_eq!(
                Into::<OptionIndex>::into(*expected_index),
                accessor_fn(elem),
                "incorrect {} at index {}",
                field,
                i,
            );
        }
    }
}
