use std::fmt;
use std::fmt::Write;

#[derive(Debug)]
pub struct UnescapeError {
    index: usize,
    codepoint_chars: [u8; 4],
    error: UnicodeError,
}

#[derive(Debug)]
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
                write!(f, "unexpected low surrogate \"\\u{codepoint_chars}\"")
            }
            UnicodeError::UnmatchedHighSurrogate => write!(
                f,
                "high surrogate \"\\u{codepoint_chars}\" not followed by low surrogate"
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
// If [escape_control_characters] is true, Unicode control characters will be
// be escaped.
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
fn unescape_json_string(s: &str, escape_control_characters: bool) -> Result<String, UnescapeError> {
    let mut chars = s.chars();
    let mut unescaped = String::with_capacity(s.len());
    let mut index = 1;

    while let Some(ch) = chars.next() {
        index += 1;
        if ch != '\\' {
            if escape_control_characters && is_control(ch) {
                unescaped.push_str("\\u00");
                write!(unescaped, "{:02X}", ch as u32).unwrap();
            } else {
                unescaped.push(ch);
            }
            continue;
        }

        let escaped = chars.next().unwrap();
        index += 1;

        match escaped {
            '"' => unescaped.push('"'),
            '\\' => unescaped.push('\\'),
            '/' => unescaped.push('/'),
            // '\b' is backspace, a control character.
            'b' => {
                if escape_control_characters {
                    unescaped.push_str("\\b");
                } else {
                    unescaped.push(0x08 as char);
                }
            }
            'f' => unescaped.push('\x0c'),
            'n' => unescaped.push('\n'),
            'r' => unescaped.push('\r'),
            't' => unescaped.push('\t'),
            'u' => {
                let (codepoint, codepoint_chars) = parse_codepoint_from_chars(&mut chars);
                index += 4;

                match decode_codepoint(codepoint) {
                    DecodedCodepoint::Char(ch) => {
                        if escape_control_characters && is_control(ch) {
                            unescaped.push_str("\\u");
                            unescaped.push(codepoint_chars[0] as char);
                            unescaped.push(codepoint_chars[1] as char);
                            unescaped.push(codepoint_chars[2] as char);
                            unescaped.push(codepoint_chars[3] as char);
                        } else {
                            unescaped.push(ch)
                        }
                    }
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

// Unescapes a syntactically valid JSON string into a valid UTF-8 string, but
// leaves control characters escaped.
pub fn safe_unescape_json_string(s: &str) -> Result<String, UnescapeError> {
    unescape_json_string(s, true)
}

// Unescapes a syntactically valid JSON string into a valid UTF-8 string, including
// control characters.
#[allow(dead_code)] // Only used with #[cfg(feature = "sexp")], but we want to write
                    // regular tests for it
pub fn unsafe_unescape_json_string(s: &str) -> Result<String, UnescapeError> {
    unescape_json_string(s, false)
}

fn is_control(ch: char) -> bool {
    matches!(ch as u32, 0x00..=0x1F | 0x7F..=0x9F)
}

// Consumes four hex characters from a Chars iterator, and converts it to a u16.
// Also returns the four original characters as a mini [u8] that can be safely
// interpreted as a str.
fn parse_codepoint_from_chars(chars: &mut std::str::Chars<'_>) -> (u16, [u8; 4]) {
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
        let unescaped = match safe_unescape_json_string(escaped) {
            Ok(s) => s,
            Err(err) => format!("ERR: {err}"),
        };

        assert_eq!(expected_unescaped, &unescaped);
    }

    #[track_caller]
    fn check_unsafe(escaped: &str, expected_unescaped: &str) {
        let unescaped = match unsafe_unescape_json_string(escaped) {
            Ok(s) => s,
            Err(err) => format!("ERR: {err}"),
        };

        assert_eq!(expected_unescaped, &unescaped);
    }

    #[test]
    fn test_unescape_json_string() {
        // Ok
        check("abc", "abc");
        check("abc \\\\ \\\"", "abc \\ \"");
        check("abc \\n \\t \\r", "abc \n \t \r");
        check("€ \\u20AC", "€ \u{20AC}");
        check("𐐷 \\uD801\\uDC37", "𐐷 \u{10437}");

        // Control characters are escaped
        check("12x\\b34", "12x\\b34");
        check(
            "\\u0000 | \\u001f | \\u0020 | \\u007e | \\u007f | \\u0080 | \\u009F | \\u00a0",
            "\\u0000 | \\u001f | \u{0020} | \u{007e} | \\u007f | \\u0080 | \\u009F | \u{00a0}",
        );

        // But not if unsafe is called
        check_unsafe("12x\\b34", "12x\x0834");
        check_unsafe(
            "\\u0000 | \\u001f | \\u0020 | \\u007e | \\u007f | \\u0080 | \\u009F | \\u00a0",
            "\x00 | \x1f | \u{0020} | \u{007e} | \x7f | \u{0080} | \u{009F} | \u{00a0}",
        );

        // Non-ASCII unescaped control codes also get escaped
        check("12 \u{0080} 34", "12 \\u0080 34");

        // But not if unsafe is called
        check_unsafe("12 \u{0080} 34", "12 \u{0080} 34");

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
