use super::jnode::{ContainerState, Focus, JContainer, JNode, JPrimitive, JValue};

pub fn render(root: &JNode, focus: &Focus) {
    print!("\x1b[2J\x1b[0;0H");
    pretty_print(root, 1, Some(focus), 0);
    print!("\r\n");
}

fn pretty_print(node: &JNode, depth: usize, focus: Option<&Focus>, focus_index: usize) {
    match &node.value {
        JValue::Primitive(p) => pretty_print_primitive(p),
        JValue::Container(c, s) => match s.get() {
            ContainerState::Collapsed => {
                let (left, right) = c.characters();
                print!("{} ... {}", left, right);
            }
            ContainerState::Inlined => {
                let (left, right) = c.characters();
                print!("{} (imagine this is inlined) {}", left, right);
            }
            ContainerState::Expanded => {
                pretty_print_container(&c, depth, focus, focus_index);
            }
        },
    }
}

fn pretty_print_primitive(p: &JPrimitive) {
    match p {
        JPrimitive::Null => print!("null"),
        JPrimitive::Bool(b) => print!("{}", b),
        JPrimitive::Number(n) => print!("{}", n),
        JPrimitive::String(s) => print!("\"{}\"", s),
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
