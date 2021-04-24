use super::jnode::{ContainerState, Focus, JContainer, JNode, JPrimitive, JValue};

pub fn render(root: &JNode) {
    print!("\x1b[2J");
    pretty_print(root, 0);
    print!("\r\n");
}

fn pretty_print(node: &JNode, depth: usize) {
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
                pretty_print_container(&c, depth);
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

fn pretty_print_container(c: &JContainer, depth: usize) {
    let (left, right) = c.characters();

    match c {
        JContainer::Array(v) => {
            print!("{}\r\n", left);

            for (i, val) in v.iter().enumerate() {
                if i > 0 {
                    print!(",\r\n");
                }
                indent(depth + 1);
                pretty_print(val, depth + 1);
            }
            print!("\n");

            indent(depth);
            print!("{}", right);
        }
        JContainer::Object(kvp) => {
            print!("{}\r\n", left);

            for (i, (k, val)) in kvp.iter().enumerate() {
                if i > 0 {
                    print!(",\r\n");
                }
                indent(depth + 1);
                print!("\"{}\": ", k);
                pretty_print(val, depth + 1);
            }
            print!("\r\n");

            indent(depth);
            print!("{}", right);
        }
        JContainer::TopLevel(j) => {
            for val in j.iter() {
                indent(depth);
                pretty_print(val, depth);
            }
        }
    }
}

fn indent(depth: usize) {
    print!("{:n$}", "", n = depth * 2);
}
