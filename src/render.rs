use std::cmp::Ordering;
use std::io::Write;
use std::rc::Rc;
use termion::color;
use termion::color::{AnsiValue, Bg, Fg, Reset};
use termion::{clear, cursor};

use super::jnode::{ContainerState, Focus, JContainer, JNode, JPrimitive, JValue};

// Output line     0
// Output line     1
// Output line     2
// ...
// Output line h - 3
// Output line h - 2
// Output line h - 1
const DEFAULT_SCROLLOFF: u16 = 3;

pub struct JsonViewer {
    // The first line to be printed out the last time we
    // rendered the screen.
    start_line: OutputLineRef,
    // Which line the focused element appeared on the last time
    // we rendered the screen.
    focused_index: u16,
    // Current dimension of the screen
    screen_width: u16,
    screen_height: u16,
    // How many lines of buffer between the focused element and
    // the top/bottom of the screen.
    scrolloff: u16,
}

enum FocusPosition {
    BeforeScrolloff,
    WithinScrolloff(u16),
    AfterScrolloff,
}

impl JsonViewer {
    pub fn new(root: &Rc<JNode>, width: u16, height: u16) -> JsonViewer {
        let first_line = OutputLineRef {
            root: Rc::clone(&root),
            path: vec![0],
            side: OutputSide::Start,
        };

        JsonViewer {
            start_line: first_line,
            focused_index: 0,
            screen_width: width,
            screen_height: height,
            scrolloff: DEFAULT_SCROLLOFF,
        }
    }

    pub fn change_focus(&mut self, focus: &Focus) {
        match self.check_where_focus_is_on_screen(focus) {
            FocusPosition::BeforeScrolloff => {
                // Want focus to be at scrolloff. Set start line to current focus, and work
                // backwards scrolloff times.
                self.start_line.path = focus.indexes.clone();
                self.start_line.side = OutputSide::Start;

                let mut focused_index = 0;

                for _ in 0..self.scrolloff {
                    if self.start_line.prev() {
                        focused_index += 1;
                    } else {
                        break;
                    }
                }

                self.focused_index = focused_index;
            }
            FocusPosition::WithinScrolloff(focused_index) => {
                self.focused_index = focused_index;
            }
            FocusPosition::AfterScrolloff => {
                // Want focus to be at the bottom of the screen. Set start line to current_focus
                // and work backwards height - scrolloff times.
                self.start_line.path = focus.indexes.clone();
                self.start_line.side = OutputSide::Start;

                let mut focused_index = 0;

                for _ in 0..(self.screen_height - self.scrolloff) {
                    if self.start_line.prev() {
                        focused_index += 1;
                    } else {
                        break;
                    }
                }

                self.focused_index = focused_index;
            }
        }
        // Check where focus element is on screen.
        // If before the start of the screen (or in scrolloff zone),
        // then just make update the start line to focus - scrolloff.
        // If on screen, within scrolloff zone, do nothing.
        // If past end of screen, update start to focus - (height - scrolloff)
        // so that focused element is at bottom of screen, but before
        // scrolloff buffer.
    }

    fn check_where_focus_is_on_screen(&mut self, focus: &Focus) -> FocusPosition {
        let mut current_line = self.start_line.clone();
        let mut current_index = 0;

        // Update current_line to the scrolloff position.
        for _ in 0..self.scrolloff {
            current_line.next();
            current_index += 1;
        }

        let mut first_comparison = true;
        while current_index < self.screen_height - self.scrolloff {
            match JsonViewer::compare_focus_and_output_line(focus, &current_line) {
                Ordering::Less => {
                    if first_comparison {
                        return FocusPosition::BeforeScrolloff;
                    } else {
                        panic!("focus was before output line after we incremented output line");
                    }
                }
                Ordering::Equal => return FocusPosition::WithinScrolloff(current_index),
                _ => { /* We'll keep incrementing current_line until the end of the screen */ }
            }

            current_line.next();
            current_index += 1;
            first_comparison = false;
        }

        FocusPosition::AfterScrolloff
    }

    // Focus: [1, 3, 1, 2]
    // OutputLineRef: [1, 3, 6, 1]; End
    //
    //          Path:   Side:
    // [            0   Start
    //   1,      0, 0
    //   2       0, 1
    // ]            0     End
    //
    // OutputLineRef [0; Start] is before Focus 0, 0
    // OutputLineRef [0; End] is after Focus 0, 0
    //
    // When OutputLineRef is at End, can equivalently increment last index by 0.5.
    // (The focus will never be on an end brace.)
    fn compare_focus_and_output_line(focus: &Focus, output_line: &OutputLineRef) -> Ordering {
        let focus_length = focus.indexes.len();
        let output_path_length = output_line.path.len();
        let min_length = std::cmp::min(focus_length, output_path_length);

        // Will only ever return equal if lengths are the same AND output side is Start.

        for index in 0..min_length {
            if focus.indexes[index] < output_line.path[index] {
                return Ordering::Less;
            } else if focus.indexes[index] > output_line.path[index] {
                return Ordering::Greater;
            }
        }

        // Focus definitely occurs before output_path.
        if focus_length < output_path_length {
            return Ordering::Less;
        } else if focus_length > output_path_length {
            // Focus appears inside the element printed by output path.
            // If printing the start, then output is before, otherwise
            // if printing the end, then output is after.
            match output_line.side {
                OutputSide::Start => return Ordering::Greater,
                OutputSide::End => return Ordering::Less,
            }
        }

        // Focus and output line are on same element. If output line
        // is printing end of the element, then focus is before, otherwise
        // they are equal.

        match output_line.side {
            OutputSide::Start => return Ordering::Equal,
            OutputSide::End => return Ordering::Less,
        }
    }

    pub fn render(&self) {
        let mut lines_printed: u16 = 0;
        let mut current_line = self.start_line.clone();

        eprintln!("Rendering screen!");

        // Print lines to fill the screen
        while lines_printed < self.screen_height {
            eprintln!(
                "Current Line: {:?}, {:?}",
                current_line.path, current_line.side
            );
            current_line.print(lines_printed, lines_printed == self.focused_index);
            lines_printed += 1;

            let more_lines = current_line.next();
            // Exit if we're done printing the JSON.
            if !more_lines {
                break;
            }
        }

        // Print end of file marker
        if lines_printed < self.screen_height {
            OutputLineRef::print_eof_marker(lines_printed);
            lines_printed += 1;
        }

        // Fill up remaining screen space with ~.
        while lines_printed < self.screen_height {
            OutputLineRef::print_past_end_of_file(lines_printed);
            lines_printed += 1;
        }

        std::io::stdout().flush().unwrap();
    }
}

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
    // Moves the line ref to the prev line in the output.
    // Returns whether or not the line was already the first line in the structure.
    //
    // Rules:
    // - If current node is primitive, go to prev sibling
    // - If current node is inlined/collapsed, go to prev sibling
    // - If on Start side of expanded container, go to prev sibling
    // - If on End side of expanded container, go to last child
    //
    // - When going to prev sibling, if current node is the
    //   first child, go to the Start side of the parent.
    //
    // - If already on the Start side of the root, don't do anything (but return false);
    fn prev(&mut self) -> bool {
        let at_child_of_root = self.path.len() == 1;
        let at_first_child_of_root = at_child_of_root && self.path[0] == 0;
        let at_start = self.side == OutputSide::Start;

        let mut parent = Rc::clone(&self.root);
        let mut current_node = Rc::clone(&self.root);
        let mut last_index = 0;
        for index in self.path.iter() {
            let next = Rc::clone(&current_node[*index]);
            parent = current_node;
            current_node = next;
            last_index = *index;
        }

        // Check if we're at the first child of the root. If we're at the Start of it, OR it's
        // a collapsed / inlined container OR it's a primitive, then return false.
        if at_first_child_of_root {
            if at_start {
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

        // Rules:
        // - If current node is primitive, go to prev sibling
        // - If current node is inlined/collapsed, go to prev sibling
        // - If on Start side of expanded container, go to prev sibling
        //
        // - If on End side of expanded container, go to last child
        //
        // - When going to prev sibling, if current node is the
        //   first child, go to the Start side of the parent.
        if current_node.is_expanded() && !at_start {
            // Go to last child current node if it's expanded.
            let last_child_index = current_node.len() - 1;
            self.path.push(last_child_index);
            self.side = if current_node[last_child_index].is_expanded() {
                OutputSide::End
            } else {
                OutputSide::Start
            };
        } else {
            // Otherwise go to previous sibling.
            if last_index == 0 {
                // But if already first sibling, go to Start of parent.
                self.path.pop();
                self.side = OutputSide::Start;
            } else {
                let i = self.path.len() - 1;
                self.path[i] -= 1;
                self.side = if parent[self.path[i]].is_expanded() {
                    OutputSide::End
                } else {
                    OutputSide::Start
                }
            }
        }

        true
    }

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
                // Go to first child of current node if it's expanded.
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
        mut is_line_focused: bool,
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
                    if is_line_focused {
                        print!(
                            "{}{}\"{}\"{}{}: ",
                            Bg(color::LightWhite),
                            Fg(color::Blue),
                            key,
                            Bg(color::Reset),
                            Fg(color::Reset)
                        );
                        is_line_focused = false;
                    } else {
                        print!("{}\"{}\"{}: ", Fg(color::LightBlue), key, Fg(color::Reset));
                    }
                }
            }
        } else {
            panic!("Parent was not container.");
        }

        match &current_node.value {
            JValue::Primitive(p) => {
                if is_line_focused {
                    print!("* ");
                }
                print_primitive(p);
            }
            JValue::Container(c, cs) => match cs.get() {
                ContainerState::Collapsed => {
                    if is_line_focused {
                        print!("* ");
                    }
                    let (left, right) = c.characters();
                    print!("{} ... {}", left, right);
                }
                ContainerState::Inlined => {
                    if is_line_focused {
                        print!("* ");
                    }
                    print_inlined_container(c);
                }
                ContainerState::Expanded => {
                    let (left, right) = c.characters();
                    match self.side {
                        OutputSide::Start => {
                            if is_line_focused {
                                print!("* ");
                            }
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
