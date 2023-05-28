use std::fmt;

pub struct UnescapeError {
    index: usize,
    codepoint_chars: [u8; 4],
    error: UnicodeError,
}

enum UnicodeError {
    UnexpectedLowSurrogate,
    UnmatchedHighSurrogate,
}

impl fmt::Display for UnescapeError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "unescaping error at char {}: ", self.index)?;
        let codepoint_chars = std::str::from_utf8(&self.codepoint_chars).unwrap();
        match &self.error {
            UnicodeError::UnexpectedLowSurrogate => {
                write!(f, "unexpected low surrogate \"\\u{}\"", codepoint_chars)
            }
            UnicodeError::UnmatchedHighSurrogate => write!(
                f,
                "high surrogate \"\\u{}\" not followed by low surrogate",
                codepoint_chars
            ),
        }
    }
}

enum DecodedCodepoint {
    Char(char),
    LowSurrogate(u16),
    HighSurrogate(u16),
}

// Unescapes a syntactically valid JSON string into a valid UTF-8 string.
//
// This makes the assumption that the only characters following a '\' are:
// - single character escapes: "\/bfnrt
// - a unicode character escape: uxxxx
//
// Unicode escapes are exactly four characters, and essentially represent
// UTF-16 encoded codepoints.
//
// Unicode codepoints between U+010000 and U+10FFFF (codepoints outside
// the Basic Multilingual Plane) must be encoded as a surrogate pair.
//
// For more information, and a walkthrough of how to convert the surrogate pairs
// back into an actual char, see:
// https://en.wikipedia.org/wiki/UTF-16#Code_points_from_U+010000_to_U+10FFFF
pub fn unescape_json_string(s: &str) -> Result<String, UnescapeError> {
    let mut chars = s.chars();
    let mut unescaped = String::with_capacity(s.len());
    let mut index = 1;

    while let Some(ch) = chars.next() {
        index += 1;
        if ch != '\\' {
            unescaped.push(ch);
            continue;
        }

        let escaped = chars.next().unwrap();
        index += 1;

        match escaped {
            '"' => unescaped.push('"'),
            '\\' => unescaped.push('\\'),
            '/' => unescaped.push('/'),
            'b' => unescaped.push('\x08'),
            'f' => unescaped.push('\x0c'),
            'n' => unescaped.push('\n'),
            'r' => unescaped.push('\r'),
            't' => unescaped.push('\t'),
            'u' => {
                let (codepoint, codepoint_chars) = parse_codepoint_from_chars(&mut chars);
                index += 4;

                match decode_codepoint(codepoint) {
                    DecodedCodepoint::Char(ch) => unescaped.push(ch),
                    DecodedCodepoint::LowSurrogate(_) => {
                        return Err(UnescapeError {
                            index: index - 6,
                            codepoint_chars,
                            error: UnicodeError::UnexpectedLowSurrogate,
                        });
                    }
                    DecodedCodepoint::HighSurrogate(hs) => match (chars.next(), chars.next()) {
                        (Some('\\'), Some('u')) => {
                            index += 2;
                            let (codepoint, _) = parse_codepoint_from_chars(&mut chars);
                            index += 4;

                            match decode_codepoint(codepoint) {
                                DecodedCodepoint::LowSurrogate(ls) => {
                                    let codepoint = (hs as u32) * 0x400 + (ls as u32) + 0x10000;
                                    unescaped.push(char::from_u32(codepoint).unwrap());
                                }
                                _ => {
                                    return Err(UnescapeError {
                                        index,
                                        codepoint_chars,
                                        error: UnicodeError::UnmatchedHighSurrogate,
                                    });
                                }
                            }
                        }
                        _ => {
                            return Err(UnescapeError {
                                index,
                                codepoint_chars,
                                error: UnicodeError::UnmatchedHighSurrogate,
                            });
                        }
                    },
                }
            }
            _ => panic!("Unexpected escape character in JSON string: {}", ch),
        }
    }

    Ok(unescaped)
}

// Consumes four hex characters from a Chars iterator, and converts it to a u16.
// Also returns the four original characters as a mini [u8] that can be safely
// interpreted as a str.
fn parse_codepoint_from_chars<'a, 'b>(chars: &'a mut std::str::Chars<'b>) -> (u16, [u8; 4]) {
    let mut codepoint = 0;
    let chars = [
        chars.next().unwrap(),
        chars.next().unwrap(),
        chars.next().unwrap(),
        chars.next().unwrap(),
    ];
    let utf8_chars = [
        chars[0] as u8,
        chars[1] as u8,
        chars[2] as u8,
        chars[3] as u8,
    ];
    codepoint += hex_char_to_int(chars[0]) * 0x1000;
    codepoint += hex_char_to_int(chars[1]) * 0x0100;
    codepoint += hex_char_to_int(chars[2]) * 0x0010;
    codepoint += hex_char_to_int(chars[3]);
    (codepoint, utf8_chars)
}

fn hex_char_to_int(ch: char) -> u16 {
    match ch {
        '0'..='9' => (ch as u16) - ('0' as u16),
        'a'..='f' => (ch as u16) - ('a' as u16) + 10,
        'A'..='f' => (ch as u16) - ('A' as u16) + 10,
        _ => panic!("Unexpected non-hex digit: {}", ch),
    }
}

// Interprets a codepoint in the Basic Multilingual Plane as either an actual
// char, or one of a surrogate pair. The value associated with the surrogate
// has had the offset removed.
fn decode_codepoint(codepoint: u16) -> DecodedCodepoint {
    match codepoint {
        0xD800..=0xDBFF => DecodedCodepoint::HighSurrogate(codepoint - 0xD800),
        0xDC00..=0xDFFF => DecodedCodepoint::LowSurrogate(codepoint - 0xDC00),
        _ => DecodedCodepoint::Char(char::from_u32(codepoint as u32).unwrap()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[track_caller]
    fn check(escaped: &str, expected_unescaped: &str) {
        let unescaped = match unescape_json_string(escaped) {
            Ok(s) => s,
            Err(err) => format!("ERR: {}", err),
        };

        assert_eq!(expected_unescaped, &unescaped);
    }

    #[test]
    fn test_unescape_json_string() {
        // Ok
        check("abc", "abc");
        check("abc \\\\ \\\"", "abc \\ \"");
        check("abc \\n \\t \\r", "abc \n \t \r");
        check("abc \\n \\t \\r", "abc \n \t \r");
        check("€ \\u20AC", "€ \u{20AC}");
        check("𐐷 \\uD801\\uDC37", "𐐷 \u{10437}");

        // Errors; make sure index is computed properly.
        check(
            "abc 𐐷 \\uD801\\uDC37 \\uD801",
            // "abc 𐐷 \uD801\uDC37 \uD801"
            // 0 2 4 6 8 0 2 4 6 8 0 2 4 6
            "ERR: unescaping error at char 26: high surrogate \"\\uD801\" not followed by low surrogate",
        );

        check(
            "abc 𐐷 \\uD801\\uDC37 \\uDC37",
            // "abc 𐐷 \uD801\uDC37 \uDC37"
            // 0 2 4 6 8 0 2 4 6 8 0 2 4 6
            "ERR: unescaping error at char 20: unexpected low surrogate \"\\uDC37\"",
        );
    }
}
