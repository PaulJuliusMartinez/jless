use serde_json::value::{Number, Value};

#[derive(Debug)]
pub enum NodeState {
    Expanded,
    Inlined,
    Collapsed,
}

#[derive(Debug)]
pub struct JNode {
    value: JValue,
    start_index: usize,
    end_index: usize,
    state: NodeState,
}

#[derive(Debug)]
pub enum JValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<JNode>),
    Object(Vec<(String, JNode)>),
}

pub fn parse_json(json: String) -> serde_json::Result<Vec<JNode>> {
    let serde_value = serde_json::from_str(&json)?;

    Ok(vec![convert_to_jnode(serde_value)])
}

fn convert_to_jnode(serde_value: Value) -> JNode {
    let value = match serde_value {
        Value::Null => JValue::Null,
        Value::Bool(b) => JValue::Bool(b),
        Value::Number(n) => JValue::Number(n),
        Value::String(s) => JValue::String(s),
        Value::Array(vs) => JValue::Array(vs.into_iter().map(convert_to_jnode).collect()),
        Value::Object(obj) => JValue::Object(
            obj.into_iter()
                .map(|(k, val)| (k, convert_to_jnode(val)))
                .collect(),
        ),
    };

    JNode {
        value,
        start_index: 0,
        end_index: 0,
        state: NodeState::Expanded,
    }
}
