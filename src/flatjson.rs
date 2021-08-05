use serde_json::value::{Number, Value};

type Index = usize;
type OptionIndex = Option<Index>;

#[derive(Debug)]
pub struct FlatJson {
    parent: OptionIndex,
    // Should these also be set on the CloseContainers?
    prev_sibling: OptionIndex,
    next_sibling: OptionIndex,

    depth: usize,
    index: Index,
    // start_index: usize
    // end_index: usize
    key: Option<String>,
    value: FlatJsonValue,
}

#[derive(Debug)]
enum JsonContainer {
    Object,
    Array,
}

#[derive(Debug)]
enum FlatJsonValue {
    Null,
    Boolean(bool),
    Number(Number),
    String(String),
    EmptyObject,
    EmptyArray,
    OpenContainer {
        container_type: JsonContainer,
        first_child: Index,
        close_index: Index,
    },
    CloseContainer {
        container_type: JsonContainer,
        last_child: Index,
        open_index: Index,
    },
}

pub fn parse_top_level_json(json: String) -> serde_json::Result<Vec<FlatJson>> {
    let serde_value = serde_json::from_str(&json)?;
    let mut flat_json = vec![];
    let mut parents = vec![None];

    flatten_json(serde_value, &mut flat_json, &mut parents);

    Ok(flat_json)
}

fn flatten_json(serde_value: Value, flat_json: &mut Vec<FlatJson>, parents: &mut Vec<OptionIndex>) {
    let depth = parents.len() - 1;
    let parent = *parents.last().unwrap();

    let value = FlatJson {
        parent,
        prev_sibling: None,
        next_sibling: None,
        depth,
        index: 0,
        key: None,
        value: FlatJsonValue::Null,
    };

    match serde_value {
        Value::Null => flat_json.push(value),
        Value::Bool(b) => flat_json.push(FlatJson {
            value: FlatJsonValue::Boolean(b),
            ..value
        }),
        Value::Number(n) => flat_json.push(FlatJson {
            value: FlatJsonValue::Number(n),
            ..value
        }),
        Value::String(s) => flat_json.push(FlatJson {
            value: FlatJsonValue::String(s),
            ..value
        }),
        Value::Array(vs) => {
            if vs.len() == 0 {
                flat_json.push(FlatJson {
                    value: FlatJsonValue::EmptyArray,
                    ..value
                })
            } else {
                let open_index = flat_json.len();
                parents.push(Some(open_index));

                flat_json.push(FlatJson {
                    value: FlatJsonValue::OpenContainer {
                        container_type: JsonContainer::Array,
                        first_child: open_index + 1,
                        // Set once done processing the array.
                        close_index: 0,
                    },
                    ..value
                });

                let mut prev_sibling: OptionIndex = None;
                let mut child_index = 0;

                for (i, v) in vs.into_iter().enumerate() {
                    child_index = flat_json.len();

                    flatten_json(v, flat_json, parents);
                    let mut child = &mut flat_json[child_index];

                    child.index = i;
                    child.prev_sibling = prev_sibling;

                    if let Some(prev_sibling_index) = prev_sibling {
                        flat_json[prev_sibling_index].next_sibling = Some(child_index);
                    }

                    prev_sibling = Some(child_index);
                }

                let close_index = flat_json.len();
                flat_json.push(FlatJson {
                    parent,
                    // Currently not set on the CloseContainer value.
                    prev_sibling: None,
                    next_sibling: None,
                    depth,
                    index: 0,
                    key: None,
                    value: FlatJsonValue::CloseContainer {
                        container_type: JsonContainer::Array,
                        last_child: child_index,
                        // Set once done processing the array.
                        open_index,
                    },
                });

                if let FlatJsonValue::OpenContainer {
                    close_index: ref mut close_index_of_open_value,
                    ..
                } = &mut flat_json[open_index].value
                {
                    *close_index_of_open_value = close_index;
                }

                parents.pop();
            }
        }
        Value::Object(obj) => {
            if obj.len() == 0 {
                flat_json.push(FlatJson {
                    value: FlatJsonValue::EmptyObject,
                    ..value
                })
            } else {
                let open_index = flat_json.len();
                parents.push(Some(open_index));

                flat_json.push(FlatJson {
                    value: FlatJsonValue::OpenContainer {
                        container_type: JsonContainer::Object,
                        first_child: open_index + 1,
                        // Set once done processing the array.
                        close_index: 0,
                    },
                    ..value
                });

                let mut prev_sibling: OptionIndex = None;
                let mut child_index = 0;

                for (i, (k, v)) in obj.into_iter().enumerate() {
                    child_index = flat_json.len();

                    flatten_json(v, flat_json, parents);
                    let mut child = &mut flat_json[child_index];

                    child.index = i;
                    child.prev_sibling = prev_sibling;
                    child.key = Some(k);

                    if let Some(prev_sibling_index) = prev_sibling {
                        flat_json[prev_sibling_index].next_sibling = Some(child_index);
                    }

                    prev_sibling = Some(child_index);
                }

                let close_index = flat_json.len();
                flat_json.push(FlatJson {
                    parent,
                    // Currently not set on the CloseContainer value.
                    prev_sibling: None,
                    next_sibling: None,
                    depth,
                    index: 0,
                    key: None,
                    value: FlatJsonValue::CloseContainer {
                        container_type: JsonContainer::Object,
                        last_child: child_index,
                        // Set once done processing the array.
                        open_index,
                    },
                });

                if let FlatJsonValue::OpenContainer {
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
