use serde_json::value::{Number, Value};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
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

impl JNode {
    fn is_primitive(&self) -> bool {
        self.value.is_primitive()
    }

    fn is_container(&self) -> bool {
        self.value.is_container()
    }

    fn size(&self) -> usize {
        debug_assert!(
            self.is_container(),
            "cannot call .size on a primitive JNode"
        );
        self.value.size()
    }

    fn is_empty(&self) -> bool {
        debug_assert!(
            self.is_container(),
            "cannot call .is_empty on a primitive JNode"
        );
        self.value.is_empty()
    }

    fn is_expanded(&self) -> bool {
        self.state == NodeState::Expanded
    }
}

impl std::ops::Index<usize> for JNode {
    type Output = JNode;

    fn index(&self, index: usize) -> &Self::Output {
        debug_assert!(self.is_container(), "cannot index into primitive JNode");

        match &self.value {
            JValue::Array(v) => &v[index],
            JValue::Object(kvp) => &kvp[index].1,
            JValue::TopLevel(j) => &j[index],
            _ => panic!("JValue::index(i) called on a primitive"),
        }
    }
}

impl std::ops::IndexMut<usize> for JNode {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        debug_assert!(self.is_container(), "cannot index into primitive JNode");

        match &mut self.value {
            JValue::Array(v) => &mut v[index],
            JValue::Object(kvp) => &mut kvp[index].1,
            JValue::TopLevel(j) => &mut j[index],
            _ => panic!("JValue::index(i) called on a primitive"),
        }
    }
}

#[derive(Debug)]
pub enum JValue {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    Array(Vec<JNode>),
    Object(Vec<(String, JNode)>),
    // Special node to represent the root of the document which
    // may consist of multiple JSON objects concatenated.
    TopLevel(Vec<JNode>),
}

impl JValue {
    fn is_primitive(&self) -> bool {
        match self {
            JValue::Null | JValue::Bool(_) | JValue::Number(_) | JValue::String(_) => true,
            _ => false,
        }
    }

    fn is_container(&self) -> bool {
        !self.is_primitive()
    }

    fn size(&self) -> usize {
        debug_assert!(
            self.is_container(),
            "cannot call .size on a primitive JValue"
        );

        match self {
            JValue::Array(v) => v.len(),
            JValue::Object(kvp) => kvp.len(),
            JValue::TopLevel(j) => j.len(),
            _ => panic!("JValue::size called on a primitive"),
        }
    }

    fn is_empty(&self) -> bool {
        debug_assert!(
            self.is_container(),
            "cannot call .is_empty on a primitive JValue"
        );
        self.size() == 0
    }
}

pub struct Focus<'a>(Vec<(&'a JNode, usize)>);

impl<'a> Focus<'a> {
    pub fn indexes(&'a self) -> Vec<usize> {
        self.0.iter().map(|(_, i)| *i).collect::<Vec<usize>>()
    }
}

pub fn parse_json(json: String) -> serde_json::Result<JNode> {
    let serde_value = serde_json::from_str(&json)?;

    let top_level = JValue::TopLevel(vec![convert_to_jnode(serde_value)]);

    Ok(JNode {
        value: top_level,
        start_index: 0,
        end_index: 0,
        state: NodeState::Expanded,
    })
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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    ToggleInline,
    FirstElem,
    LastElem,
    // NextOccurrenceOfKey
    // PrevOccurrenceOfKey
    TopOfTree,
    BottomOfTree,
}

pub fn perform_action(focus: &mut Focus, action: Action) {
    debug_assert!(validate_focus(focus));

    match action {
        Action::Up => move_up(focus),
        Action::Down => move_down(focus),
        Action::Left => move_left(focus),
        Action::Right => move_right(focus),
        _ => {}
    }

    debug_assert!(validate_focus(focus));
}

// Make sure our focus is valid.
fn validate_focus(focus: &Focus) -> bool {
    assert!(focus.0.len() > 0);

    for (node, index) in focus.0.iter() {
        assert!(node.is_container());
        assert!(*index < node.size());
    }

    true
}

fn move_up(focus: &mut Focus) {}

fn move_down(focus: &mut Focus) {
    let (parent_node, index) = focus.0.last().unwrap();
    let current_node = &parent_node[*index];
    let mut depth_index = focus.0.len() - 1;

    if current_node.is_container() && current_node.is_expanded() && !current_node.is_empty() {
        focus.0.push((current_node, 0));
    } else {
        // next sibling (easy case is index += 1)
        while depth_index > 0 {
            let (node, curr_index) = focus.0[depth_index];

            if curr_index + 1 < node.size() {
                focus.0.truncate(depth_index + 1);
                focus.0[depth_index].1 += 1;
                break;
            }

            depth_index -= 1;
        }
    }

    // If current node is expanded, to go first child
    //      Note: empty array/object can't expand, so act as if collapsed
    // If current node is collapsed / inlined, go to next sibling
    // If current node is last child, navigate to next sibling of parent
    //      (and recurse if necessary)
    // If actually the last node in the tree, do nothing
    //
    // If current node is primitive, go to next sibling
}

fn move_left(focus: &mut Focus) {}
fn move_right(focus: &mut Focus) {}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_OBJ: &'static str = r#"{
        "a": { "aa": 1, "ab": 2, "ac": 3 },
        "b": [1, 2, 3]
    }"#;

    #[test]
    fn test_movement_down_simple() {
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        let mut focus: Focus = Focus(vec![(&top_level, 0)]);

        assert_movements(
            &mut focus,
            vec![
                (Action::Down, vec![0, 0].as_slice()),
                (Action::Down, vec![0, 0, 0].as_slice()),
                (Action::Down, vec![0, 0, 1].as_slice()),
                (Action::Down, vec![0, 0, 2].as_slice()),
                (Action::Down, vec![0, 1].as_slice()),
                (Action::Down, vec![0, 1, 0].as_slice()),
                (Action::Down, vec![0, 1, 1].as_slice()),
                (Action::Down, vec![0, 1, 2].as_slice()),
                // Stay on last node
                (Action::Down, vec![0, 1, 2].as_slice()),
            ]
            .as_slice(),
        );
    }

    #[test]
    fn test_movement_down_skips_collapsed() {
        let mut top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        top_level[0][0].state = NodeState::Collapsed;
        let mut focus: Focus = Focus(vec![(&top_level, 0)]);

        assert_movements(
            &mut focus,
            vec![
                (Action::Down, vec![0, 0].as_slice()),
                (Action::Down, vec![0, 1].as_slice()),
                (Action::Down, vec![0, 1, 0].as_slice()),
            ]
            .as_slice(),
        );
    }

    fn assert_focus_indexes(focus: &Focus, indexes: &[usize]) {
        assert_eq!(focus.indexes().as_slice(), indexes);
    }

    fn assert_movements(focus: &mut Focus, actions_and_focuses: &[(Action, &[usize])]) {
        println!("Starting focus: {:?}", focus.indexes());
        for (action, new_focus_indexes) in actions_and_focuses.iter() {
            perform_action(focus, *action);
            println!(
                "Performed action: {:?}, new focus: {:?}",
                action,
                focus.indexes()
            );
            assert_focus_indexes(focus, new_focus_indexes);
        }
    }
}
