use serde_json::value::{Number, Value};

use std::cell::{Cell, RefCell};
use std::ops::Index;
use std::rc::{Rc, Weak};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ContainerState {
    Expanded,
    Inlined,
    Collapsed,
}

#[derive(Debug)]
pub struct JNode {
    pub value: JValue,
    pub parent: RefCell<Option<Weak<JNode>>>,
    pub start_index: usize,
    pub end_index: usize,
}

impl JNode {
    fn is_primitive(&self) -> bool {
        self.value.is_primitive()
    }

    fn is_container(&self) -> bool {
        self.value.is_container()
    }

    pub fn len(&self) -> usize {
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

    fn collapse(&self) {
        self.set_container_state(ContainerState::Collapsed)
    }

    fn inline(&self) {
        self.set_container_state(ContainerState::Inlined)
    }

    fn expand(&self) {
        self.set_container_state(ContainerState::Expanded)
    }

    fn set_container_state(&self, new_state: ContainerState) {
        match self.value {
            JValue::Container(_, ref state) => state.set(new_state),
            _ => panic!("cannot set_container_state on primitive JNode"),
        }
    }

    fn set_parent_on_children(parent: &Rc<JNode>) {
        match &parent.value {
            JValue::Container(JContainer::Array(v), _) => {
                for child in v.iter() {
                    *child.parent.borrow_mut() = Some(Rc::downgrade(parent));
                }
            }
            JValue::Container(JContainer::Object(kvp), _) => {
                for (_, child) in kvp.iter() {
                    *child.parent.borrow_mut() = Some(Rc::downgrade(parent));
                }
            }
            JValue::Container(JContainer::TopLevel(j), _) => {
                for child in j.iter() {
                    *child.parent.borrow_mut() = Some(Rc::downgrade(parent));
                }
            }
            _ => { /* No children */ }
        }
    }
}

impl Index<usize> for JNode {
    type Output = Rc<JNode>;

    fn index(&self, index: usize) -> &Self::Output {
        match &self.value {
            JValue::Container(c, _) => &c[index],
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
    Array(Vec<Rc<JNode>>),
    Object(Vec<(String, Rc<JNode>)>),
    // Special node to represent the root of the document which
    // may consist of multiple JSON objects concatenated.
    TopLevel(Vec<Rc<JNode>>),
}

impl JContainer {
    pub fn characters(&self) -> (char, char) {
        match self {
            JContainer::Array(_) => ('[', ']'),
            JContainer::Object(_) => ('{', '}'),
            JContainer::TopLevel(_) => ('<', '>'),
        }
    }
}

impl Index<usize> for JContainer {
    type Output = Rc<JNode>;

    fn index(&self, index: usize) -> &Self::Output {
        match &self {
            JContainer::Array(v) => &v[index],
            JContainer::Object(kvp) => &kvp[index].1,
            JContainer::TopLevel(j) => &j[index],
        }
    }
}

impl JContainer {
    pub fn len(&self) -> usize {
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
    Container(JContainer, Cell<ContainerState>),
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
pub struct Focus(pub Vec<(Rc<JNode>, usize)>);

impl Focus {
    pub fn indexes(&self) -> Vec<usize> {
        self.0.iter().map(|(_, i)| *i).collect::<Vec<usize>>()
    }

    pub fn current_node(&self) -> Rc<JNode> {
        let (parent_node, index) = self.0.last().unwrap();
        Rc::clone(&parent_node[*index])
    }
}

pub fn parse_json(json: String) -> serde_json::Result<Rc<JNode>> {
    let serde_value = serde_json::from_str(&json)?;

    let top_level = JContainer::TopLevel(vec![convert_to_jnode(serde_value)]);

    let top_level = Rc::new(JNode {
        value: JValue::Container(top_level, Cell::new(ContainerState::Expanded)),
        parent: RefCell::new(None),
        start_index: 0,
        end_index: 0,
    });

    JNode::set_parent_on_children(&top_level);

    Ok(top_level)
}

fn convert_to_jnode(serde_value: Value) -> Rc<JNode> {
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
                JValue::Container(JContainer::Array(jnodes), Cell::new(expanded))
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
                JValue::Container(JContainer::Object(key_value_pairs), Cell::new(expanded))
            }
        }
    };

    let jnode = Rc::new(JNode {
        value,
        parent: RefCell::new(None),
        start_index: 0,
        end_index: 0,
    });

    JNode::set_parent_on_children(&jnode);

    jnode
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    ToggleInline,
    FocusFirstElem,
    FocusLastElem,
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
        Action::ToggleInline => toggle_inline(focus),
        Action::FocusFirstElem => focus_first_elem(focus),
        Action::FocusLastElem => focus_last_elem(focus),
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
    let focus_length = focus.0.len();

    // If we're at the very top, do nothing.
    if focus_length == 1 && focus.0[0].1 == 0 {
        return;
    }

    if focus.0[focus_length - 1].1 == 0 {
        focus.0.pop();
        return;
    }

    focus.0[focus_length - 1].1 -= 1;
    let mut curr_node = focus.current_node();

    loop {
        let last_child_index;
        let next;

        if let JValue::Container(container, cs) = &curr_node.value {
            if cs.get() != ContainerState::Expanded {
                break;
            }

            last_child_index = container.len() - 1;
            next = Rc::clone(&container[last_child_index]);
        } else {
            break;
        }

        focus.0.push((curr_node, last_child_index));
        curr_node = next;
    }
}

// Rules:
// - If current node is primitive, go to next sibling
// - If current node is inlined/collapsed, go to next sibling
// - Otherwise, go to first child
//
// - When going to next sibling, if current node is the
//   last child, go to the next sibling of the parent
//   (and repeat if parent is also last child)
//
// - If actually the last node in the tree, don't modify
//   focus
fn move_down(focus: &mut Focus) {
    let current_node = focus.current_node();
    let mut depth_index = focus.0.len() - 1;

    match &current_node.value {
        JValue::Container(_, cs) if cs.get() == ContainerState::Expanded => {
            focus.0.push((Rc::clone(&current_node), 0));
        }
        _ => {
            while depth_index > 0 {
                let (node, curr_index) = &focus.0[depth_index];

                if curr_index + 1 < node.len() {
                    focus.0.truncate(depth_index + 1);
                    focus.0[depth_index].1 += 1;
                    break;
                }

                depth_index -= 1;
            }
        }
    };
}

// Rules:
// - If a primitive, go to parent, unless already topmost node
// - If collapsed or inlined, go to parent, unless already topmost node
// - Otherwise, collapse yourself
fn move_left(focus: &mut Focus) {
    let current_node = focus.current_node();

    let mut pop_if_not_top_level = || {
        if focus.0.len() > 1 {
            focus.0.pop();
        }
    };

    match &current_node.value {
        JValue::Primitive(_) => {
            pop_if_not_top_level();
        }
        JValue::Container(_, cs) => match cs.get() {
            ContainerState::Inlined => {
                pop_if_not_top_level();
            }
            ContainerState::Collapsed => {
                pop_if_not_top_level();
            }
            ContainerState::Expanded => {
                current_node.collapse();
            }
        },
    };
}

// Rules:
// - If a primitive, do nothing
// - If inlined, do nothing
// - If collapsed, expand
// - If expanded, go to first child
fn move_right(focus: &mut Focus) {
    let current_node = focus.current_node();

    match &current_node.value {
        JValue::Primitive(_) => { /* do nothing */ }
        JValue::Container(_, cs) => {
            match cs.get() {
                ContainerState::Inlined => { /* do nothing */ }
                ContainerState::Collapsed => current_node.expand(),
                ContainerState::Expanded => {
                    focus.0.push((Rc::clone(&current_node), 0));
                }
            }
        }
    };
}

fn toggle_inline(focus: &mut Focus) {
    let current_node = focus.current_node();

    match &current_node.value {
        JValue::Primitive(_) => {
            // TODO: display a message to user that primitives
            // cannot be inlined.
        }
        JValue::Container(_, ref cs) => {
            if cs.get() == ContainerState::Inlined {
                cs.set(ContainerState::Expanded);
            } else {
                cs.set(ContainerState::Inlined);
            }
        }
    };
}

fn focus_first_elem(focus: &mut Focus) {
    let (node, ref mut index) = focus.0.last_mut().unwrap();
    *index = 0;
}

fn focus_last_elem(focus: &mut Focus) {
    let (node, ref mut index) = focus.0.last_mut().unwrap();
    *index = node.len() - 1;
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIMPLE_OBJ: &'static str = r#"{
        "a": { "aa": 1, "ab": 2, "ac": 3 },
        "z": [1, 2, 3]
    }"#;

    const SIMPLE_OBJ_WITH_EMPTY: &'static str = r#"{
        "a": {},
        "b": [1, 2, 3]
    }"#;

    #[test]
    fn test_movement_down_simple() {
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        let mut focus: Focus = Focus(vec![(top_level, 0)]);

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
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        top_level[0][0].collapse();
        let mut focus: Focus = Focus(vec![(top_level, 0)]);

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
        let mut focus: Focus = Focus(vec![(top_level, 0)]);

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
        let mut focus = construct_focus(top_level, &[0, 1, 2]);

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
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        top_level[0][0].collapse();
        let mut focus: Focus = construct_focus(top_level, &[0, 1, 0]);

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
        let mut focus: Focus = construct_focus(top_level, &[0, 1, 0]);

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
    fn test_movement_right() {
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        top_level[0].collapse();
        top_level[0][0].collapse();
        let mut focus: Focus = construct_focus(Rc::clone(&top_level), &[0]);

        // Expand first node, but don't enter
        perform_action(&mut focus, Action::Right);
        assert_focus_indexes(&focus, &[0]);
        assert_container_state(&top_level[0], ContainerState::Expanded);

        // Enter first node
        perform_action(&mut focus, Action::Right);
        assert_focus_indexes(&focus, &[0, 0]);
        assert_container_state(&top_level[0][0], ContainerState::Collapsed);

        // Expand inner node, but don't enter
        perform_action(&mut focus, Action::Right);
        assert_focus_indexes(&focus, &[0, 0]);
        assert_container_state(&top_level[0][0], ContainerState::Expanded);

        // Enter inner node
        perform_action(&mut focus, Action::Right);
        assert_focus_indexes(&focus, &[0, 0, 0]);

        // Focused on a primitive, going more right doesn't do anything.
        perform_action(&mut focus, Action::Right);
        assert_focus_indexes(&focus, &[0, 0, 0]);
    }

    #[test]
    fn test_movement_left() {
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        let mut focus: Focus = construct_focus(Rc::clone(&top_level), &[0, 1, 1]);

        // Exit inner node
        perform_action(&mut focus, Action::Left);
        assert_focus_indexes(&focus, &[0, 1]);
        assert_container_state(&top_level[0][1], ContainerState::Expanded);

        // Collapse inner node
        perform_action(&mut focus, Action::Left);
        assert_focus_indexes(&focus, &[0, 1]);
        assert_container_state(&top_level[0][1], ContainerState::Collapsed);

        // Exit inner node to outer node
        perform_action(&mut focus, Action::Left);
        assert_focus_indexes(&focus, &[0]);
        assert_container_state(&top_level[0], ContainerState::Expanded);

        // Collapse outer node
        perform_action(&mut focus, Action::Left);
        assert_focus_indexes(&focus, &[0]);
        assert_container_state(&top_level[0], ContainerState::Collapsed);

        // At top left, can't go anywhere.
        perform_action(&mut focus, Action::Left);
        assert_focus_indexes(&focus, &[0]);
    }

    #[test]
    fn test_toggle_inline() {
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        let mut focus: Focus = construct_focus(Rc::clone(&top_level), &[0]);

        perform_action(&mut focus, Action::ToggleInline);
        assert_container_state(&top_level[0], ContainerState::Inlined);

        perform_action(&mut focus, Action::ToggleInline);
        assert_container_state(&top_level[0], ContainerState::Expanded);

        perform_action(&mut focus, Action::Right);
        perform_action(&mut focus, Action::ToggleInline);
        assert_container_state(&top_level[0][0], ContainerState::Inlined);
    }

    #[test]
    fn test_focus_first_and_last() {
        let top_level = parse_json(SIMPLE_OBJ.to_owned()).unwrap();
        let mut focus: Focus = construct_focus(top_level, &[0, 0, 1]);

        assert_movements(
            &mut focus,
            vec![
                (Action::FocusFirstElem, vec![0, 0, 0].as_slice()),
                (Action::FocusFirstElem, vec![0, 0, 0].as_slice()),
                (Action::FocusLastElem, vec![0, 0, 2].as_slice()),
                (Action::FocusLastElem, vec![0, 0, 2].as_slice()),
                (Action::Left, vec![0, 0].as_slice()),
                (Action::FocusLastElem, vec![0, 1].as_slice()),
                (Action::Right, vec![0, 1, 0].as_slice()),
                (Action::FocusLastElem, vec![0, 1, 2].as_slice()),
            ]
            .as_slice(),
        );
    }

    fn assert_focus_indexes(focus: &Focus, indexes: &[usize]) {
        assert_eq!(focus.indexes().as_slice(), indexes);
    }

    fn assert_movements<'a>(
        focus: &'a mut Focus,
        actions_and_focuses: &'a [(Action, &'a [usize])],
    ) {
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

    fn construct_focus(top_level: Rc<JNode>, indexes: &[usize]) -> Focus {
        let mut curr_node = top_level;
        let mut focus = Vec::new();

        for index in indexes.iter() {
            let next = Rc::clone(&curr_node[*index]);
            focus.push((curr_node, *index));
            curr_node = next;
        }

        Focus(focus)
    }

    fn assert_container_state(node: &JNode, state: ContainerState) {
        match &node.value {
            JValue::Container(_, node_state) => assert_eq!(state, node_state.get()),
            _ => panic!("called assert_container_state on a primitive node"),
        }
    }
}
