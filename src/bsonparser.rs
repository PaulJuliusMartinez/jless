use bson::Document;

pub fn parse(bson_bytes: &[u8]) -> Result<String, String> {
    let doc = Document::from_reader(&mut &bson_bytes[..]).map_err(|e| e.to_string())?;
    serde_json::to_string_pretty(&doc).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use indoc::indoc;

    #[test]
    fn test_bson_input_conversion() {
        // BSON for `{"hello": "world"}`
        let bson_bytes: Vec<u8> = vec![
            0x16, 0x00, 0x00, 0x00, 0x02, b'h', b'e', b'l', b'l', b'o', 0x00, 0x06, 0x00, 0x00,
            0x00, b'w', b'o', b'r', b'l', b'd', 0x00, 0x00,
        ];

        let json_string = parse(&bson_bytes).unwrap();

        let expected_json = indoc! {r#"
            {
              "hello": "world"
            }"#
        };

        assert_eq!(json_string.trim(), expected_json.trim());
    }
}
