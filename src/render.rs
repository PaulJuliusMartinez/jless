use std::io::Write;
use std::rc::Rc;
use termion::color;
use termion::color::{AnsiValue, Fg, Reset};
use termion::{clear, cursor};

use super::jnode::{ContainerState, Focus, JContainer, JNode, JPrimitive, JValue};

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum OutputSide {
    Start,
    End,
}

#[derive(Debug, Clone)]
pub struct OutputLineRef {
    pub root: Rc<JNode>,
    pub path: Vec<usize>,
    pub side: OutputSide,
}

impl OutputLineRef {
    // Moves the line ref to the next line in the output.
    // Returns whether or not the line was already the last line in the structure.
    //
    // Rules:
    // - If current node is primitive, go to next sibling
    // - If current node is inlined/collapsed, go to next sibling
    // - If on Start side of expanded container, go to first child
    // - If on End side of expanded container, go to next sibling
    //
    // - When going to next sibling, if current node is the
    //   last child, go to the End side of the parent.
    //
    // - If already on the End side of the root, don't do anything (but return false);
    fn next(&mut self) -> bool {
        let at_child_of_root = self.path.len() == 1;
        let at_last_child_of_root = at_child_of_root && self.path[0] == self.root.len() - 1;
        let at_end = self.side == OutputSide::End;

        let mut parent = Rc::clone(&self.root);
        let mut current_node = Rc::clone(&self.root);
        let mut last_index = 0;
        for index in self.path.iter() {
            let next = Rc::clone(&current_node[*index]);
            parent = current_node;
            current_node = next;
            last_index = *index;
        }

        // Check if we're at the last child of the root. If we're at the End of it, OR it's
        // a collapsed / inlined container OR it's a primitive, then return false.
        if at_last_child_of_root {
            if at_end {
                return false;
            }

            match &current_node.value {
                JValue::Primitive(_) => return false,
                JValue::Container(_, cs) => {
                    if cs.get() != ContainerState::Expanded {
                        return false;
                    }
                }
            }
        }

        match &current_node.value {
            JValue::Container(_, cs) if cs.get() == ContainerState::Expanded && !at_end => {
                // Go to first current node if it's expanded.
                self.path.push(0);
                self.side = OutputSide::Start;
            }
            _ => {
                // Otherwise go to next sibling.
                if last_index == parent.len() - 1 {
                    // But if already last sibling, go to End of parent.
                    self.path.pop();
                    self.side = OutputSide::End;
                } else {
                    let i = self.path.len() - 1;
                    self.path[i] += 1;
                    self.side = OutputSide::Start;
                }
            }
        }

        true
    }

    // Example object:          Corresponding path & side:     Parent      Current Node
    //
    // {                        0;        Start                TopLevel    Object
    //   "a": 1,                0, 0;     Start                Object      Primitive
    //   "b": [                 0, 1;     Start                Object      Array
    //      "c": { ... }        0, 1, 0;  Start                Array       Object (collapsed)
    //   ]                      0, 1;       End                Object      Array
    // }                        0;          End                TopLevel    Object
    // [                        1;        Start                TopLevel    Array
    //   "json"                 1, 0;     Start                Array       Primitive
    // ]                        1;          End                TopLevel    Array
    //
    // indentation level = 2 * (path.len - 1)
    fn print(
        &self,
        line_number: u16,
        // focus: &Focus,
        // depth_modification: usize,
        // screen_width: u16,
    ) {
        // This value is ignored, but Rust doesn't know it's guaranteed to be set in the loop.
        let mut parent = Rc::clone(&self.root);
        let mut current_node = Rc::clone(&self.root);
        let mut last_index = 0;
        for index in self.path.iter() {
            let next = Rc::clone(&current_node[*index]);
            parent = current_node;
            current_node = next;
            last_index = *index;
        }

        let depth = self.path.len() as u16 - 1;
        Self::position_cursor(depth, line_number);

        let mut print_trailing_comma = true;

        if let JValue::Container(c, _) = &parent.value {
            if c.len() - 1 == last_index {
                print_trailing_comma = false;
            }

            if let JContainer::Object(kvp) = c {
                // Only print the object key if you printing the start of the current node.
                if self.side == OutputSide::Start {
                    let (key, _) = &kvp[last_index];
                    print!("{}\"{}\"{}: ", Fg(color::LightBlue), key, Fg(color::Reset));
                }
            }
        } else {
            panic!("Parent was not container.");
        }

        match &current_node.value {
            JValue::Primitive(p) => print_primitive(p),
            JValue::Container(c, cs) => match cs.get() {
                ContainerState::Collapsed => {
                    let (left, right) = c.characters();
                    print!("{} ... {}", left, right);
                }
                ContainerState::Inlined => {
                    print_inlined_container(c);
                }
                ContainerState::Expanded => {
                    let (left, right) = c.characters();
                    match self.side {
                        OutputSide::Start => {
                            print!("{}", left);
                            print_trailing_comma = false;
                        }
                        OutputSide::End => print!("{}", right),
                    }
                }
            },
        }

        if print_trailing_comma {
            print!(",");
        }
    }

    fn print_eof_marker(line_number: u16) {
        Self::position_cursor(0, line_number);
        print!("(END)");
    }

    fn print_past_end_of_file(line_number: u16) {
        Self::position_cursor(0, line_number);
        print!("~");
    }

    fn position_cursor(depth: u16, line_number: u16) {
        // Terminal coordinates are 1 based.
        let x = 1 + 2 * depth;
        let y = line_number + 1;
        // Position cursor and clear line.
        print!("{}{}", cursor::Goto(x, y), clear::CurrentLine);
    }
}

pub fn render_screen(root: &JNode, focus: &Focus, start_line: &OutputLineRef, screen_height: u16) {
    let mut lines_printed: u16 = 0;
    let mut current_line = start_line.clone();

    eprintln!("Rendering screen!");

    // Print lines to fill the screen
    while lines_printed < screen_height {
        eprintln!(
            "Current Line: {:?}, {:?}",
            current_line.path, current_line.side
        );
        current_line.print(lines_printed);
        lines_printed += 1;

        let more_lines = current_line.next();
        // Exit if we're done printing the JSON.
        if !more_lines {
            break;
        }
    }

    // Print end of file marker
    if lines_printed < screen_height {
        OutputLineRef::print_eof_marker(lines_printed);
        lines_printed += 1;
    }

    // Fill up remaining screen space with ~.
    while lines_printed < screen_height {
        OutputLineRef::print_past_end_of_file(lines_printed);
        lines_printed += 1;
    }

    std::io::stdout().flush().unwrap();
}

// #[derive(Debug, Copy, Clone, PartialEq, Eq)]
// enum ScrollDirection {
//     Up,
//     Down,
// }

// pub fn scroll_screen(
//     root: &JNodeRef,
//     focus: &mut Focus,
//     current_start_line: &OutputLineRef,
//     direction: ScrollDirection,
// ) -> OutputLineRef {
//     // May need to modify focus if it goes outside scroll-off area.
//     current_start_line.clone()
// }

//
// BASIC PRINTING IMPLEMENTATION BELOW
//

pub fn render(root: &JNode, focus: &Focus) {
    print!("\x1b[2J\x1b[0;0H");
    pretty_print(root, 1, Some(focus), 0);
    print!("\r\n");
}

fn pretty_print(node: &JNode, depth: usize, focus: Option<&Focus>, focus_index: usize) {
    match &node.value {
        JValue::Primitive(p) => print_primitive(p),
        JValue::Container(c, s) => match s.get() {
            ContainerState::Collapsed => {
                let (left, right) = c.characters();
                print!("{} ... {}", left, right);
            }
            ContainerState::Inlined => {
                print_inlined_container(&c);
            }
            ContainerState::Expanded => {
                pretty_print_container(&c, depth, focus, focus_index);
            }
        },
    }
}

fn print_inline(node: &JNode) {
    match &node.value {
        JValue::Primitive(p) => print_primitive(p),
        JValue::Container(c, s) => match s.get() {
            ContainerState::Collapsed => {
                let (left, right) = c.characters();
                print!("{} ... {}", left, right);
            }
            _ => {
                print_inlined_container(&c);
            }
        },
    }
}

fn print_primitive(p: &JPrimitive) {
    match p {
        // Print in gray
        JPrimitive::Null => print!("{}null{}", Fg(AnsiValue::grayscale(12)), Fg(color::Reset)),
        // Print in yellow
        JPrimitive::Bool(b) => print!("{}{}{}", Fg(color::Yellow), b, Fg(color::Reset)),
        // Print in purple
        JPrimitive::Number(n) => print!("{}{}{}", Fg(color::Magenta), n, Fg(color::Reset)),
        // Print in green
        JPrimitive::String(s) => print!("{}\"{}\"{}", Fg(color::Green), s, Fg(color::Reset)),
        JPrimitive::EmptyArray => print!("[]"),
        JPrimitive::EmptyObject => print!("{{}}"),
    }
}

fn pretty_print_container(c: &JContainer, depth: usize, focus: Option<&Focus>, focus_index: usize) {
    let (left, right) = c.characters();

    match c {
        JContainer::Array(v) => {
            print!("{}\r\n", left);

            for (i, val) in v.iter().enumerate() {
                if i > 0 {
                    print!(",\r\n");
                }
                indent_container_elem(depth, focus, focus_index, i);
                pretty_print_container_elem(val, depth + 1, focus, focus_index, i);
            }
            print!("\r\n");

            indent(depth - 1);
            print!("{}", right);
        }
        JContainer::Object(kvp) => {
            print!("{}\r\n", left);

            for (i, (k, val)) in kvp.iter().enumerate() {
                if i > 0 {
                    print!(",\r\n");
                }
                indent_container_elem(depth, focus, focus_index, i);
                print!("\"{}\": ", k);
                pretty_print_container_elem(val, depth + 1, focus, focus_index, i);
            }
            print!("\r\n");

            indent(depth - 1);
            print!("{}", right);
        }
        JContainer::TopLevel(j) => {
            for (i, val) in j.iter().enumerate() {
                indent_container_elem(depth, focus, focus_index, i);
                pretty_print_container_elem(val, depth + 1, focus, focus_index, i);
            }
        }
    }
}

fn print_inlined_container(c: &JContainer) {
    let (left, right) = c.characters();

    match c {
        JContainer::Array(v) => {
            print!("{}", left);
            for (i, val) in v.iter().enumerate() {
                if i > 0 {
                    print!(", ");
                }
                print_inline(val);
            }
            print!("{}", right);
        }
        JContainer::Object(kvp) => {
            print!("{}", left);
            for (i, (k, val)) in kvp.iter().enumerate() {
                if i > 0 {
                    print!(", ");
                }
                print!("{}\"{}\"{}: ", Fg(color::LightBlue), k, Fg(color::Reset));
                print_inline(val);
            }
            print!("{}", right);
        }
        JContainer::TopLevel(j) => {
            for val in j.iter() {
                print_inline(val);
            }
        }
    }
}

fn pretty_print_container_elem(
    node: &JNode,
    depth: usize,
    focus: Option<&Focus>,
    focus_index: usize,
    elem_index: usize,
) {
    if let Some(f) = focus {
        let focused_index = f.0[focus_index].1;
        if focused_index == elem_index && focus_index < f.0.len() - 1 {
            pretty_print(node, depth, focus, focus_index + 1);
        } else {
            pretty_print(node, depth, None, 0);
        }
    } else {
        pretty_print(node, depth, focus, 0);
    }
}

fn indent_container_elem(
    depth: usize,
    focus: Option<&Focus>,
    focus_index: usize,
    elem_index: usize,
) {
    if let Some(f) = focus {
        let at_focus_depth = f.0.len() - 1 == focus_index;
        let elem_index_matches = f.0[focus_index].1 == elem_index;

        if at_focus_depth && elem_index_matches {
            print!("* ");
            indent(depth - 1);
        } else {
            indent(depth);
        }
    } else {
        indent(depth);
    }
}

fn indent(depth: usize) {
    print!("{:n$}", "", n = (depth + 1) * 2);
}
