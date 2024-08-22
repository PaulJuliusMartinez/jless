use logos::{Lexer, Logos};

use crate::flatjson::{ContainerType, Index, OptionIndex, Row, Value};
use crate::jsontokenizer::JsonToken;

struct JsonParser<'a> {
    tokenizer: Lexer<'a, JsonToken>,
    parents: Vec<Index>,
    rows: Vec<Row>,
    pretty_printed: String,
    max_depth: usize,

    peeked_token: Option<Option<JsonToken>>,
}

pub fn parse(json: String) -> Result<(Vec<Row>, String, usize), String> {
    let mut parser = JsonParser {
        tokenizer: JsonToken::lexer(&json),
        parents: vec![],
        rows: vec![],
        pretty_printed: String::new(),
        max_depth: 0,
        peeked_token: None,
    };

    parser.parse_top_level_json()?;

    Ok((parser.rows, parser.pretty_printed, parser.max_depth))
}

impl<'a> JsonParser<'a> {
    fn next_token(&mut self) -> Option<Result<JsonToken, <JsonToken as Logos>::Error>> {
        if self.peeked_token.is_some() {
            self.peeked_token.take().unwrap().map(Ok)
        } else {
            self.tokenizer.next()
        }
    }

    fn advance(&mut self) {
        self.next_token();
    }

    fn advance_and_consume_whitespace(&mut self) {
        self.advance();
        self.consume_whitespace();
    }

    fn peek_token_or_eof(&mut self) -> Option<JsonToken> {
        if self.peeked_token.is_none() {
            self.peeked_token = Some(self.tokenizer.next().and_then(|r| r.ok()));
        }

        self.peeked_token.unwrap()
    }

    fn peek_token(&mut self) -> Result<JsonToken, String> {
        self.peek_token_or_eof()
            .ok_or_else(|| "Unexpected EOF".to_string())
    }

    fn unexpected_token(&mut self) -> Result<usize, String> {
        Err(format!("Unexpected token: {:?}", self.peek_token()))
    }

    fn consume_whitespace(&mut self) {
        while let Some(JsonToken::Whitespace | JsonToken::Newline) = self.peek_token_or_eof() {
            self.advance();
        }
    }

    fn parse_top_level_json(&mut self) -> Result<(), String> {
        self.consume_whitespace();
        let mut prev_top_level = self.parse_elem()?;
        let mut num_child = 0;

        loop {
            self.consume_whitespace();

            if self.peek_token_or_eof().is_none() {
                break;
            }

            self.pretty_printed.push('\n');
            let next_top_level = self.parse_elem()?;
            num_child += 1;

            self.rows[next_top_level].prev_sibling = OptionIndex::Index(prev_top_level);
            self.rows[next_top_level].index_in_parent = num_child;
            self.rows[prev_top_level].next_sibling = OptionIndex::Index(next_top_level);

            prev_top_level = next_top_level;
        }

        Ok(())
    }

    fn parse_elem(&mut self) -> Result<usize, String> {
        self.consume_whitespace();

        self.max_depth = self.max_depth.max(self.parents.len());

        loop {
            match self.peek_token()? {
                JsonToken::OpenCurly => {
                    return self.parse_object();
                }
                JsonToken::OpenSquare => {
                    return self.parse_array();
                }
                JsonToken::Null => {
                    return self.parse_null();
                }
                JsonToken::True => {
                    return self.parse_bool(true);
                }
                JsonToken::False => {
                    return self.parse_bool(false);
                }
                JsonToken::Number => {
                    return self.parse_number();
                }
                JsonToken::String => {
                    return self.parse_string();
                }

                JsonToken::Whitespace | JsonToken::Newline => {
                    panic!("Should have just consumed whitespace");
                }

                JsonToken::CloseCurly
                | JsonToken::CloseSquare
                | JsonToken::Colon
                | JsonToken::Comma => {
                    return Err(format!("Unexpected character: {:?}", self.tokenizer.span()));
                }
            }
        }
    }

    fn parse_array(&mut self) -> Result<usize, String> {
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
        self.advance_and_consume_whitespace();

        let mut prev_sibling = OptionIndex::Nil;
        let mut num_children = 0;

        loop {
            if num_children != 0 {
                match self.peek_token()? {
                    // Great, we needed a comma; eat it up.
                    JsonToken::Comma => self.advance_and_consume_whitespace(),
                    // We're going to peek again below and check for ']', so we don't
                    // need to do anything.
                    JsonToken::CloseSquare => {}
                    _ => return self.unexpected_token(),
                }
            }

            if self.peek_token()? == JsonToken::CloseSquare {
                self.advance();
                break;
            }

            // Add comma to pretty printed version _after_ we know
            // we didn't see a CloseSquare so we don't add a trailing comma.
            if num_children != 0 {
                self.pretty_printed.push_str(", ");
            }

            let child = self.parse_elem()?;
            self.consume_whitespace();

            if num_children == 0 {
                match self.rows[array_open_index].value {
                    Value::OpenContainer {
                        ref mut first_child,
                        ..
                    } => {
                        *first_child = child;
                    }
                    _ => panic!("Must be Array!"),
                }
            }

            self.rows[child].prev_sibling = prev_sibling;
            self.rows[child].index_in_parent = num_children;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child);
            }

            num_children += 1;
            prev_sibling = OptionIndex::Index(child);
        }

        self.parents.pop();

        if num_children == 0 {
            self.rows[array_open_index].value = Value::EmptyArray;
            self.rows[array_open_index].range.end = self.rows[array_open_index].range.start + 2;
        } else {
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
        }

        self.pretty_printed.push(']');
        Ok(array_open_index)
    }

    fn parse_object(&mut self) -> Result<usize, String> {
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
        self.advance_and_consume_whitespace();

        let mut prev_sibling = OptionIndex::Nil;
        let mut num_children = 0;

        loop {
            if num_children != 0 {
                match self.peek_token()? {
                    // Great, we needed a comma; eat it up.
                    JsonToken::Comma => self.advance_and_consume_whitespace(),
                    // We're going to peek again below and check for '}', so we don't
                    // need to do anything.
                    JsonToken::CloseCurly => {}
                    _ => return self.unexpected_token(),
                }
            }

            if self.peek_token()? == JsonToken::CloseCurly {
                self.advance();
                break;
            }

            // Add comma to pretty printed version _after_ we know
            // we didn't see a CloseSquare so we don't add a trailing comma.
            if num_children != 0 {
                self.pretty_printed.push_str(", ");
            } else {
                // Add space inside objects.
                self.pretty_printed.push(' ');
            }

            if self.peek_token()? != JsonToken::String {
                return self.unexpected_token();
            }

            let key_range = {
                let key_range_start = self.pretty_printed.len();
                let key_span_len = self.tokenizer.span().len();
                let key_range = key_range_start..key_range_start + key_span_len;

                self.pretty_printed.push_str(self.tokenizer.slice());
                self.advance_and_consume_whitespace();
                key_range
            };

            if self.peek_token()? != JsonToken::Colon {
                return self.unexpected_token();
            }
            self.advance_and_consume_whitespace();
            self.pretty_printed.push_str(": ");

            let child = self.parse_elem()?;
            self.rows[child].key_range = Some(key_range);
            self.consume_whitespace();

            if num_children == 0 {
                match self.rows[object_open_index].value {
                    Value::OpenContainer {
                        ref mut first_child,
                        ..
                    } => {
                        *first_child = child;
                    }
                    _ => panic!("Must be Object!"),
                }
            }

            self.rows[child].prev_sibling = prev_sibling;
            self.rows[child].index_in_parent = num_children;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child);
            }

            num_children += 1;
            prev_sibling = OptionIndex::Index(child);
        }

        self.parents.pop();

        if num_children == 0 {
            self.rows[object_open_index].value = Value::EmptyObject;
            self.rows[object_open_index].range.end = self.rows[object_open_index].range.start + 2;
        } else {
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
        }

        self.pretty_printed.push('}');
        Ok(object_open_index)
    }

    fn parse_null(&mut self) -> Result<usize, String> {
        self.advance();
        let row_index = self.create_row(Value::Null);
        self.rows[row_index].range.end = self.rows[row_index].range.start + 4;
        self.pretty_printed.push_str("null");
        Ok(row_index)
    }

    fn parse_bool(&mut self, b: bool) -> Result<usize, String> {
        self.advance();

        let row_index = self.create_row(Value::Boolean);
        let (bool_str, len) = if b { ("true", 4) } else { ("false", 5) };

        self.rows[row_index].range.end = self.rows[row_index].range.start + len;
        self.pretty_printed.push_str(bool_str);

        Ok(row_index)
    }

    fn parse_number(&mut self) -> Result<usize, String> {
        let row_index = self.create_row(Value::Number);
        self.pretty_printed.push_str(self.tokenizer.slice());

        self.rows[row_index].range.end =
            self.rows[row_index].range.start + self.tokenizer.slice().len();

        self.advance();
        Ok(row_index)
    }

    fn parse_string(&mut self) -> Result<usize, String> {
        let row_index = self.create_row(Value::String);

        // The token includes the quotation marks.
        self.pretty_printed.push_str(self.tokenizer.slice());
        self.rows[row_index].range.end =
            self.rows[row_index].range.start + self.tokenizer.slice().len();

        self.advance();
        Ok(row_index)
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
            index_in_parent: 0,
            key_range: None,
        });

        index
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_row_ranges() {
        //            0 2    7  10   15    21   26    32     39 42
        let json = r#"{ "a": 1, "b": true, "c": null, "ddd": [] }"#.to_owned();
        let (rows, _, _) = parse(json).unwrap();

        assert_eq!(rows[0].range, 0..43); // Object
        assert_eq!(rows[1].key_range, Some(2..5)); // "a": 1
        assert_eq!(rows[1].range, 7..8); // "a": 1
        assert_eq!(rows[2].key_range, Some(10..13)); // "b": true
        assert_eq!(rows[2].range, 15..19); // "b": true
        assert_eq!(rows[3].key_range, Some(21..24)); // "c": null
        assert_eq!(rows[3].range, 26..30); // "c": null
        assert_eq!(rows[4].range, 39..41); // "ddd": []
        assert_eq!(rows[5].range, 42..43); // }

        //            01   5        14     21 23
        let json = r#"[14, "apple", false, {}]"#.to_owned();
        let (rows, _, _) = parse(json).unwrap();

        assert_eq!(rows[0].range, 0..24); // Array
        assert_eq!(rows[1].range, 1..3); // 14
        assert_eq!(rows[2].range, 5..12); // "apple"
        assert_eq!(rows[3].range, 14..19); // false
        assert_eq!(rows[4].range, 21..23); // {}
        assert_eq!(rows[5].range, 23..24); // ]

        //            01 3      10     17    23  27   32   37 40    46   51
        let json = r#"[{ "abc": "str", "de": 14, "f": null }, true, false]"#.to_owned();
        let (rows, _, _) = parse(json).unwrap();

        assert_eq!(rows[0].range, 0..52); // Array
        assert_eq!(rows[1].range, 1..38); // Object
        assert_eq!(rows[2].key_range, Some(3..8)); // "abc": "str"
        assert_eq!(rows[2].range, 10..15); // "abc": "str"
        assert_eq!(rows[3].key_range, Some(17..21)); // "de": 14
        assert_eq!(rows[3].range, 23..25); // "de": 14
        assert_eq!(rows[4].key_range, Some(27..30)); // "f": null
        assert_eq!(rows[4].range, 32..36); // "f": null
        assert_eq!(rows[5].range, 37..38); // }
        assert_eq!(rows[6].range, 40..44); // true
        assert_eq!(rows[7].range, 46..51); // false
        assert_eq!(rows[8].range, 51..52); // ]
    }
}
