use yaml_rust::yaml::{Array, Hash, Yaml};
use yaml_rust::YamlLoader;

use crate::flatjson::{ContainerType, Index, OptionIndex, Row, Value};

struct YamlParser {
    parents: Vec<Index>,
    rows: Vec<Row>,
    pretty_printed: String,
    max_depth: usize,
}

pub fn parse(yaml: String) -> Result<(Vec<Row>, String, usize), String> {
    let mut parser = YamlParser {
        parents: vec![],
        rows: vec![],
        pretty_printed: String::new(),
        max_depth: 0,
    };

    let docs = match YamlLoader::load_from_str(&yaml) {
        Ok(yaml_docs) => yaml_docs,
        Err(err) => return Err(format!("{}", err)),
    };

    for (i, doc) in docs.into_iter().enumerate() {
        if i != 0 {
            parser.pretty_printed.push('\n');
        }
        parser.parse_yaml_item(doc)?;
    }

    eprintln!("Pretty Printed: {}", parser.pretty_printed);

    Ok((parser.rows, parser.pretty_printed, parser.max_depth))
}

impl YamlParser {
    fn parse_yaml_item(&mut self, item: Yaml) -> Result<usize, String> {
        match &item {
            Yaml::Array(arr) => {
                eprintln!("depth: {}, array len: {:?}", self.parents.len(), arr.len());
            }
            Yaml::Hash(hash) => {
                eprintln!("depth: {}, hash len: {:?}", self.parents.len(), hash.len());
            }
            _ => {
                eprintln!("depth: {}, node: {:?}", self.parents.len(), item);
            }
        }

        let index = match item {
            Yaml::BadValue => return Err("Unknown YAML parse error".to_owned()),
            Yaml::Null => self.parse_null(),
            Yaml::Boolean(b) => self.parse_bool(b),
            Yaml::Integer(i) => self.parse_number(i.to_string()),
            Yaml::Real(real_str) => self.parse_number(real_str),
            Yaml::String(s) => self.parse_string(s),
            Yaml::Array(arr) => self.parse_array(arr)?,
            Yaml::Hash(hash) => self.parse_hash(hash)?,
            Yaml::Alias(i) => self.parse_alias(i),
        };

        Ok(index)
    }

    fn parse_null(&mut self) -> usize {
        let row_index = self.create_row(Value::Null);
        self.rows[row_index].range.end = self.rows[row_index].range.start + 4;
        self.pretty_printed.push_str("null");
        row_index
    }

    fn parse_bool(&mut self, b: bool) -> usize {
        let row_index = self.create_row(Value::Boolean);
        let (bool_str, len) = if b { ("true", 4) } else { ("false", 5) };

        self.rows[row_index].range.end = self.rows[row_index].range.start + len;
        self.pretty_printed.push_str(bool_str);

        row_index
    }

    fn parse_number(&mut self, num_s: String) -> usize {
        let row_index = self.create_row(Value::Number);
        self.pretty_printed.push_str(&num_s);

        self.rows[row_index].range.end = self.rows[row_index].range.start + num_s.len();

        row_index
    }

    fn parse_string(&mut self, s: String) -> usize {
        let row_index = self.create_row(Value::String);

        self.pretty_printed.push('"');
        self.pretty_printed.push_str(&s);
        self.pretty_printed.push('"');
        self.rows[row_index].range.end = self.rows[row_index].range.start + s.len() + 2;

        row_index
    }

    fn parse_array(&mut self, arr: Array) -> Result<usize, String> {
        if arr.is_empty() {
            let row_index = self.create_row(Value::EmptyArray);
            self.rows[row_index].range.end = self.rows[row_index].range.start + 2;
            self.pretty_printed.push_str("[]");
            return Ok(row_index);
        }

        let open_value = Value::OpenContainer {
            container_type: ContainerType::Array,
            collapsed: false,
            // To be set when parsing is complete.
            first_child: 0,
            close_index: 0,
        };

        let array_open_index = self.create_row(open_value);

        self.parents.push(array_open_index);
        self.pretty_printed.push('[');

        let mut prev_sibling = OptionIndex::Nil;

        for (i, child) in arr.into_iter().enumerate() {
            if i != 0 {
                self.pretty_printed.push_str(", ");
            }

            let child_index = self.parse_yaml_item(child)?;

            if i == 0 {
                match self.rows[array_open_index].value {
                    Value::OpenContainer {
                        ref mut first_child,
                        ..
                    } => {
                        *first_child = child_index;
                    }
                    _ => panic!("Must be Array!"),
                }
            }

            self.rows[child_index].prev_sibling = prev_sibling;
            self.rows[child_index].index = i;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child_index);
            }

            prev_sibling = OptionIndex::Index(child_index);
        }

        self.parents.pop();

        let close_value = Value::CloseContainer {
            container_type: ContainerType::Array,
            collapsed: false,
            last_child: prev_sibling.unwrap(),
            open_index: array_open_index,
        };

        let array_close_index = self.create_row(close_value);

        // Update end of the Array range; we add the ']' to pretty_printed
        // below, hence the + 1.
        self.rows[array_open_index].range.end = self.pretty_printed.len() + 1;

        match self.rows[array_open_index].value {
            Value::OpenContainer {
                ref mut close_index,
                ..
            } => {
                *close_index = array_close_index;
            }
            _ => panic!("Must be Array!"),
        }

        self.pretty_printed.push(']');
        Ok(array_open_index)
    }

    fn parse_hash(&mut self, hash: Hash) -> Result<usize, String> {
        if hash.is_empty() {
            let row_index = self.create_row(Value::EmptyObject);
            self.rows[row_index].range.end = self.rows[row_index].range.start + 2;
            self.pretty_printed.push_str("{}");
            return Ok(row_index);
        }

        let open_value = Value::OpenContainer {
            container_type: ContainerType::Object,
            collapsed: false,
            // To be set when parsing is complete.
            first_child: 0,
            close_index: 0,
        };

        let object_open_index = self.create_row(open_value);

        self.parents.push(object_open_index);
        self.pretty_printed.push('{');

        let mut prev_sibling = OptionIndex::Nil;

        for (i, (key, value)) in hash.into_iter().enumerate() {
            if i == 0 {
                // Add space inside objects.
                self.pretty_printed.push(' ');
            } else {
                self.pretty_printed.push_str(", ");
            }

            /////////////////////////////////

            let key_range = {
                let key_range_start = self.pretty_printed.len();

                self.pretty_print_key_item(key, true)?;

                let key_range_end = self.pretty_printed.len();

                key_range_start..key_range_end
            };

            self.pretty_printed.push_str(": ");

            let child_index = self.parse_yaml_item(value)?;

            self.rows[child_index].key_range = Some(key_range);

            if i == 0 {
                match self.rows[object_open_index].value {
                    Value::OpenContainer {
                        ref mut first_child,
                        ..
                    } => {
                        *first_child = child_index;
                    }
                    _ => panic!("Must be Object!"),
                }
            }

            self.rows[child_index].prev_sibling = prev_sibling;
            self.rows[child_index].index = i;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child_index);
            }

            prev_sibling = OptionIndex::Index(child_index);
        }

        self.parents.pop();

        // Print space inside closing brace.
        self.pretty_printed.push(' ');

        let close_value = Value::CloseContainer {
            container_type: ContainerType::Object,
            collapsed: false,
            last_child: prev_sibling.unwrap(),
            open_index: object_open_index,
        };

        let object_close_index = self.create_row(close_value);

        // Update end of the Object range; we add the '}' to pretty_printed
        // below, hence the + 1.
        self.rows[object_open_index].range.end = self.pretty_printed.len() + 1;

        match self.rows[object_open_index].value {
            Value::OpenContainer {
                ref mut close_index,
                ..
            } => {
                *close_index = object_close_index;
            }
            _ => panic!("Must be Object!"),
        }

        self.pretty_printed.push('}');
        Ok(object_open_index)
    }

    fn parse_alias(&mut self, n: usize) -> usize {
        0
    }

    fn pretty_print_key_item(&mut self, item: Yaml, is_key: bool) -> Result<(), String> {
        if let Yaml::String(s) = item {
            self.pretty_printed.push('"');
            self.pretty_printed.push_str(&s);
            self.pretty_printed.push('"');
            return Ok(());
        }

        if is_key {
            self.pretty_printed.push('[');
        }

        match item {
            Yaml::BadValue => return Err("Unknown YAML parse error".to_owned()),
            Yaml::Null => self.pretty_printed.push_str("null"),
            Yaml::Boolean(b) => self
                .pretty_printed
                .push_str(if b { "true" } else { "false " }),
            Yaml::Integer(i) => self.pretty_printed.push_str(&i.to_string()),
            Yaml::Real(real_str) => self.pretty_printed.push_str(&real_str),
            Yaml::Array(arr) => {
                if arr.is_empty() {
                    self.pretty_printed.push_str("[]");
                } else {
                    self.pretty_printed.push('[');
                    for (i, elem) in arr.into_iter().enumerate() {
                        if i != 0 {
                            self.pretty_printed.push_str(", ");
                        }
                        self.pretty_print_key_item(elem, false)?;
                    }
                    self.pretty_printed.push(']');
                }
            }
            Yaml::Hash(hash) => {
                if hash.is_empty() {
                    self.pretty_printed.push_str("{}");
                } else {
                    self.pretty_printed.push_str("{ ");
                    for (i, (key, value)) in hash.into_iter().enumerate() {
                        if i != 0 {
                            self.pretty_printed.push_str(", ");
                        }
                        self.pretty_print_key_item(key, true)?;
                        self.pretty_printed.push_str(": ");
                        self.pretty_print_key_item(value, false)?;
                    }
                    self.pretty_printed.push_str(" }");
                }
            }
            Yaml::Alias(i) => {}
            Yaml::String(_) => unreachable!(),
        }

        if is_key {
            self.pretty_printed.push(']');
        }

        Ok(())
    }

    // Add a new row to the FlatJson representation.
    //
    // self.pretty_printed should NOT include the added row yet;
    // we use the current length of self.pretty_printed as the
    // starting index of the row's range.
    fn create_row(&mut self, value: Value) -> usize {
        let index = self.rows.len();

        let parent = match self.parents.last() {
            None => OptionIndex::Nil,
            Some(row_index) => OptionIndex::Index(*row_index),
        };

        let range_start = self.pretty_printed.len();

        self.rows.push(Row {
            // Set correctly by us
            parent,
            depth: self.parents.len(),
            value,

            // The start of this range is set by us, but then we set
            // the end when we're done parsing the row. We'll set
            // the default end to be one character so we don't have to
            // update it after ']' and '}'.
            range: range_start..range_start + 1,

            // To be filled in by caller
            prev_sibling: OptionIndex::Nil,
            next_sibling: OptionIndex::Nil,
            index: 0,
            key_range: None,
        });

        index
    }
}
