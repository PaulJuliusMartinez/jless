use logos::{Lexer, Logos};
use serde_json::Number;

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
    fn next_token(&mut self) -> Option<JsonToken> {
        if self.peeked_token.is_some() {
            self.peeked_token.take().unwrap()
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
            self.peeked_token = Some(self.tokenizer.next());
        }

        // eprintln!(
        //     "Peeked: {:?} ({:?})",
        //     self.peeked_token.unwrap(),
        //     self.tokenizer.span()
        // );
        self.peeked_token.unwrap()
    }

    fn peek_token(&mut self) -> Result<JsonToken, String> {
        self.peek_token_or_eof()
            .ok_or_else(|| "Unexpected EOF".to_string())
    }

    fn unexpected_token(&mut self) -> Result<usize, String> {
        Err(format!("Unexpected_token: {:?}", self.tokenizer.span()))
    }

    fn consume_whitespace(&mut self) {
        while let Some(JsonToken::Whitespace | JsonToken::Newline) = self.peek_token_or_eof() {
            self.advance();
        }
    }

    fn parse_top_level_json(&mut self) -> Result<(), String> {
        loop {
            self.consume_whitespace();

            let top_level_elem = if self.peek_token_or_eof().is_none() {
                None
            } else {
                Some(self.parse_elem()?)
            };

            match top_level_elem {
                // Wire up top_level object siblings.
                Some(_elem) => {}
                None => break,
            }
        }

        Ok(())
    }

    fn parse_elem(&mut self) -> Result<usize, String> {
        self.consume_whitespace();

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

                JsonToken::Error => {
                    return Err("Parse error".to_string());
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
            self.rows[child].index = num_children;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child);
            }

            num_children += 1;
            prev_sibling = OptionIndex::Index(child);
        }

        if num_children == 0 {
            self.rows[array_open_index].value = Value::EmptyArray;
        } else {
            let close_value = Value::CloseContainer {
                container_type: ContainerType::Array,
                collapsed: false,
                last_child: prev_sibling.unwrap(),
                open_index: array_open_index,
            };

            let array_close_index = self.create_row(close_value);

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
        self.parents.pop();
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
                self.pretty_printed.push_str(" ");
            }

            if self.peek_token()? != JsonToken::String {
                return self.unexpected_token();
            }

            let key = {
                self.pretty_printed.push_str(self.tokenizer.slice());
                let key_span_len = self.tokenizer.span().len();
                let actual_key = self.tokenizer.slice()[1..key_span_len - 1].to_string();
                self.advance_and_consume_whitespace();
                actual_key
            };

            if self.peek_token()? != JsonToken::Colon {
                return self.unexpected_token();
            }
            self.advance_and_consume_whitespace();
            self.pretty_printed.push_str(": ");

            let child = self.parse_elem()?;
            self.rows[child].key = Some(key);
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
            self.rows[child].index = num_children;
            if let OptionIndex::Index(prev) = prev_sibling {
                self.rows[prev].next_sibling = OptionIndex::Index(child);
            }

            num_children += 1;
            prev_sibling = OptionIndex::Index(child);
        }

        if num_children == 0 {
            self.rows[object_open_index].value = Value::EmptyObject;
        } else {
            let close_value = Value::CloseContainer {
                container_type: ContainerType::Object,
                collapsed: false,
                last_child: prev_sibling.unwrap(),
                open_index: object_open_index,
            };

            let object_close_index = self.create_row(close_value);

            match self.rows[object_open_index].value {
                Value::OpenContainer {
                    ref mut close_index,
                    ..
                } => {
                    *close_index = object_close_index;
                }
                _ => panic!("Must be Object!"),
            }

            // Print space inside closing brace.
            self.pretty_printed.push_str(" ");
        }

        self.pretty_printed.push('}');
        self.parents.pop();
        Ok(object_open_index)
    }

    fn parse_null(&mut self) -> Result<usize, String> {
        self.advance();
        self.pretty_printed.push_str("null");
        Ok(self.create_row(Value::Null))
    }

    fn parse_bool(&mut self, b: bool) -> Result<usize, String> {
        self.advance();
        self.pretty_printed
            .push_str(if b { "true" } else { "false" });
        Ok(self.create_row(Value::Boolean(b)))
    }

    fn parse_number(&mut self) -> Result<usize, String> {
        self.pretty_printed.push_str(self.tokenizer.slice());
        self.advance();
        Ok(self.create_row(Value::Number(Number::from_string_unchecked(
            self.tokenizer.slice().to_string(),
        ))))
    }

    fn parse_string(&mut self) -> Result<usize, String> {
        // The token includes the quotation marks.
        self.pretty_printed.push_str(self.tokenizer.slice());
        let span_len = self.tokenizer.span().len();
        let actual_str = &self.tokenizer.slice()[1..span_len - 1];
        self.advance();
        Ok(self.create_row(Value::String(actual_str.to_string())))
    }

    fn create_row(&mut self, value: Value) -> usize {
        let index = self.rows.len();

        let parent = match self.parents.last() {
            None => OptionIndex::Nil,
            Some(row_index) => OptionIndex::Index(*row_index),
        };

        self.rows.push(Row {
            // Set correctly by us
            parent,
            depth: self.parents.len(),
            value,

            // To be filled in by caller
            prev_sibling: OptionIndex::Nil,
            next_sibling: OptionIndex::Nil,
            index: 0,
            key: None,
        });

        index
    }
}
