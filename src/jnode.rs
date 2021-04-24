use serde_json::value::{Number, Value};

use std::ops::{Index, IndexMut};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Expanded,
    Inlined,
    Collapsed,
}

#[derive(Debug)]
pub struct JNode {
    value: JValue,
    start_index: usize,
    end_index: usize,
}

impl JNode {
    fn is_primitive(&self) -> bool {
        self.value.is_primitive()
    }

    fn is_container(&self) -> bool {
        self.value.is_container()
    }

    fn len(&self) -> usize {
        debug_assert!(self.is_container(), "cannot call .len on a primitive JNode");
        self.value.len()
    }

    fn is_empty(&self) -> bool {
        debug_assert!(
            self.is_container(),
            "cannot call .is_empty on a primitive JNode"
        );
        self.value.is_empty()
    }

    fn collapse(&mut self) {
        self.set_container_state(ContainerState::Collapsed)
    }

    fn inline(&mut self) {
        self.set_container_state(ContainerState::Inlined)
    }

    fn expand(&mut self) {
        self.set_container_state(ContainerState::Expanded)
    }

    fn set_container_state(&mut self, new_state: ContainerState) {
        match self.value {
            JValue::Container(_, ref mut state) => *state = new_state,
            _ => panic!("cannot set_container_state on primitive JNode"),
        }
    }
}

impl Index<usize> for JNode {
    type Output = JNode;

    fn index(&self, index: usize) -> &Self::Output {
        match &self.value {
            JValue::Container(c, _) => &c[index],
            _ => panic!("JValue::index(i) called on a primitive"),
        }
    }
}

impl IndexMut<usize> for JNode {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        match &mut self.value {
            JValue::Container(c, _) => &mut c[index],
            _ => panic!("JValue::index(i) called on a primitive"),
        }
    }
}

// "Primitive" Values (cannot be drilled into)
#[derive(Debug)]
pub enum JPrimitive {
    Null,
    Bool(bool),
    Number(Number),
    String(String),
    EmptyArray,
    EmptyObject,
}

// "Container" Values (contain additional nodes)
#[derive(Debug)]
pub enum JContainer {
    Array(Vec<JNode>),
    Object(Vec<(String, JNode)>),
    // Special node to represent the root of the document which
    // may consist of multiple JSON objects concatenated.
    TopLevel(Vec<JNode>),
}

impl Index<usize> for JContainer {
    type Output = JNode;

    fn index(&self, index: usize) -> &Self::Output {
        match &self {
            JContainer::Array(v) => &v[index],
            JContainer::Object(kvp) => &kvp[index].1,
            JContainer::TopLevel(j) => &j[index],
        }
    }
}

impl IndexMut<usize> for JContainer {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        match self {
            JContainer::Array(v) => &mut v[index],
            JContainer::Object(kvp) => &mut kvp[index].1,
            JContainer::TopLevel(j) => &mut j[index],
        }
    }
}

impl JContainer {
    fn len(&self) -> usize {
        match self {
            JContainer::Array(v) => v.len(),
            JContainer::Object(obj) => obj.len(),
            JContainer::TopLevel(j) => j.len(),
        }
    }
}

#[derive(Debug)]
pub enum JValue {
    Primitive(JPrimitive),
    Container(JContainer, ContainerState),
}

impl JValue {
    fn is_primitive(&self) -> bool {
        match self {
            JValue::Primitive(_) => true,
            _ => false,
        }
    }

    fn is_container(&self) -> bool {
        !self.is_primitive()
    }

    fn len(&self) -> usize {
        match self {
            JValue::Container(c, _) => c.len(),
            _ => panic!("cannot call .len on a primitive JValue"),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            JValue::Container(c, _) => c.len() == 0,
            _ => panic!("cannot call .is_empty on a primitive JValue"),
        }
    }
}

// TODO: Make this type way nicer to work with.
pub struct Focus<'a>(Vec<(&'a JNode, usize)>);

impl<'a> Focus<'a> {
    pub fn indexes(&'a self) -> Vec<usize> {
        self.0.iter().map(|(_, i)| *i).collect::<Vec<usize>>()
    }
}

pub fn parse_json(json: String) -> serde_json::Result<JNode> {
    let serde_value = serde_json::from_str(&json)?;

    let top_level = JContainer::TopLevel(vec![convert_to_jnode(serde_value)]);

    Ok(JNode {
        value: JValue::Container(top_level, ContainerState::Expanded),
        start_index: 0,
        end_index: 0,
    })
}

fn convert_to_jnode(serde_value: Value) -> JNode {
    let expanded = ContainerState::Expanded;
    let value = match serde_value {
        Value::Null => JValue::Primitive(JPrimitive::Null),
        Value::Bool(b) => JValue::Primitive(JPrimitive::Bool(b)),
        Value::Number(n) => JValue::Primitive(JPrimitive::Number(n)),
        Value::String(s) => JValue::Primitive(JPrimitive::String(s)),
        Value::Array(vs) => {
            if vs.len() == 0 {
                JValue::Primitive(JPrimitive::EmptyArray)
            } else {
                let jnodes = vs.into_iter().map(convert_to_jnode).collect();
                JValue::Container(JContainer::Array(jnodes), expanded)
            }
        }
        Value::Object(obj) => {
            if obj.len() == 0 {
                JValue::Primitive(JPrimitive::EmptyObject)
            } else {
                let key_value_pairs = obj
                    .into_iter()
                    .map(|(k, val)| (k, convert_to_jnode(val)))
                    .collect();
                JValue::Container(JContainer::Object(key_value_pairs), expanded)
            }
        }
    };

    JNode {
        value,
        start_index: 0,
        end_index: 0,
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    // ToggleInline,
    // FirstElem,
    // LastElem,
    // NextOccurrenceOfKey
    // PrevOccurrenceOfKey
    // TopOfTree,
    // BottomOfTree,
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
        assert!(*index < node.len());
    }

    true
}

// Rules:
// - If parent is top level node, and you're first child, do nothing.
// - If you're first child (index == 0), go to parent.
// - Otherwise, go to previous sibling (index -= 1), then go to its
//   last child.
fn move_up(focus: &mut Focus) {
    // If we're at the very top, do nothing.
    if focus.0.len() == 1 && focus.0[0].1 == 0 {
        return;
    }

    let (parent, ref mut index) = focus.0.last_mut().unwrap();

    if *index == 0 {
        focus.0.pop();
        return;
    }

    *index -= 1;
    let mut curr_node = &parent[*index];

    while let JValue::Container(container, ContainerState::Expanded) = &curr_node.value {
        let last_child_index = container.len() - 1;
        let next = &container[last_child_index];
        focus.0.push((curr_node, last_child_index));
        curr_node = next;
    }
}

// Rules:
// - If current node is primitive, go to next sibling
// - If current node is collapsed, go to next sibling
// - If current node is empty, go to next sibling
// - Otherwise, go to first child
//
// - When going to next sibling, if current node is the
//   last child, go to the next sibling of the parent
//   (and repeat if parent is also last child)
//
// - If actually the last node in the tree, don't modify
//   focus
fn move_down(focus: &mut Focus) {
    let (parent_node, index) = focus.0.last().unwrap();
    let current_node = &parent_node[*index];
    let mut depth_index = focus.0.len() - 1;

    if let JValue::Container(_, ContainerState::Expanded) = &current_node.value {
        focus.0.push((current_node, 0));
    } else {
        while depth_index > 0 {
            let (node, curr_index) = focus.0[depth_index];

            if curr_index + 1 < node.len() {
                focus.0.truncate(depth_index + 1);
                focus.0[depth_index].1 += 1;
                break;
            }

            depth_index -= 1;
        }
    }
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

    const SIMPLE_OBJ_WITH_EMPTY: &'static str = r#"{
        "a": {},
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
        top_level[0][0].collapse();
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

    #[test]
    fn test_movement_down_skips_empty() {
        let top_level = parse_json(SIMPLE_OBJ_WITH_EMPTY.to_owned()).unwrap();
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

    #[test]
    fn test_movement_up_simple() {
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        let mut focus = construct_focus(&top_level, &[0, 1, 2]);

        assert_movements(
            &mut focus,
            vec![
                (Action::Up, vec![0, 1, 1].as_slice()),
                (Action::Up, vec![0, 1, 0].as_slice()),
                (Action::Up, vec![0, 1].as_slice()),
                (Action::Up, vec![0, 0, 2].as_slice()),
                (Action::Up, vec![0, 0, 1].as_slice()),
                (Action::Up, vec![0, 0, 0].as_slice()),
                (Action::Up, vec![0, 0].as_slice()),
                (Action::Up, vec![0].as_slice()),
                // Stay at top level.
                (Action::Up, vec![0].as_slice()),
            ]
            .as_slice(),
        );
    }

    #[test]
    fn test_movement_up_skips_collapsed() {
        let mut top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        top_level[0][0].collapse();
        let mut focus: Focus = construct_focus(&top_level, &[0, 1, 0]);

        assert_movements(
            &mut focus,
            vec![
                (Action::Up, vec![0, 1].as_slice()),
                (Action::Up, vec![0, 0].as_slice()),
                (Action::Up, vec![0].as_slice()),
            ]
            .as_slice(),
        );
    }

    #[test]
    fn test_movement_up_skips_empty() {
        let top_level = parse_json(SIMPLE_OBJ_WITH_EMPTY.to_owned()).unwrap();
        let mut focus: Focus = construct_focus(&top_level, &[0, 1, 0]);

        assert_movements(
            &mut focus,
            vec![
                (Action::Up, vec![0, 1].as_slice()),
                (Action::Up, vec![0, 0].as_slice()),
                (Action::Up, vec![0].as_slice()),
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

    fn construct_focus<'a, 'b>(top_level: &'a JNode, indexes: &'b [usize]) -> Focus<'a> {
        let mut curr_node = top_level;
        let mut focus = Vec::new();

        for index in indexes.iter() {
            focus.push((curr_node, *index));
            curr_node = &curr_node[*index];
        }

        Focus(focus)
    }
}
