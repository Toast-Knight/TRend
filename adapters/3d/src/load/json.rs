//! A small, strict JSON parser (RFC 8259), enough for glTF. Depth and size are bounded.

use std::collections::BTreeMap;

#[derive(Clone, Debug, PartialEq)]
pub enum Json {
    Null,
    Bool(bool),
    Number(f64),
    String(String),
    Array(Vec<Json>),
    Object(BTreeMap<String, Json>),
}

const MAX_DEPTH: usize = 128;

impl Json {
    pub fn get(&self, key: &str) -> Option<&Json> {
        match self {
            Self::Object(map) => map.get(key),
            _ => None,
        }
    }

    #[cfg(test)]
    pub fn index(&self, index: usize) -> Option<&Json> {
        match self {
            Self::Array(items) => items.get(index),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_f32(&self) -> Option<f32> {
        self.as_f64().map(|value| value as f32)
    }

    /// A non-negative whole number.
    pub fn as_usize(&self) -> Option<usize> {
        let value = self.as_f64()?;
        (value >= 0.0 && value.fract() == 0.0 && value <= usize::MAX as f64)
            .then_some(value as usize)
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(value) => Some(value),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(value) => Some(*value),
            _ => None,
        }
    }

    pub fn as_array(&self) -> &[Json] {
        match self {
            Self::Array(items) => items,
            _ => &[],
        }
    }

    /// An array of numbers, such as a vector or matrix.
    pub fn as_floats(&self) -> Option<Vec<f32>> {
        match self {
            Self::Array(items) => items.iter().map(Self::as_f32).collect(),
            _ => None,
        }
    }
}

struct Parser<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl Parser<'_> {
    fn error<T>(&self, message: &str) -> Result<T, String> {
        Err(format!("JSON: {message} at byte {}", self.position))
    }

    fn skip_space(&mut self) {
        while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.bytes.get(self.position) {
            self.position += 1;
        }
    }

    fn expect(&mut self, literal: &[u8]) -> Result<(), String> {
        if self.bytes.get(self.position..self.position + literal.len()) == Some(literal) {
            self.position += literal.len();
            Ok(())
        } else {
            self.error("unexpected text")
        }
    }

    fn value(&mut self, depth: usize) -> Result<Json, String> {
        if depth > MAX_DEPTH {
            return self.error("nested too deeply");
        }
        self.skip_space();
        match self.bytes.get(self.position) {
            Some(b'{') => {
                self.position += 1;
                let mut map = BTreeMap::new();
                self.skip_space();
                if self.bytes.get(self.position) == Some(&b'}') {
                    self.position += 1;
                    return Ok(Json::Object(map));
                }
                loop {
                    self.skip_space();
                    let key = self.string()?;
                    self.skip_space();
                    self.expect(b":")?;
                    let value = self.value(depth + 1)?;
                    map.insert(key, value);
                    self.skip_space();
                    match self.bytes.get(self.position) {
                        Some(b',') => self.position += 1,
                        Some(b'}') => {
                            self.position += 1;
                            return Ok(Json::Object(map));
                        }
                        _ => return self.error("expected , or }"),
                    }
                }
            }
            Some(b'[') => {
                self.position += 1;
                let mut items = Vec::new();
                self.skip_space();
                if self.bytes.get(self.position) == Some(&b']') {
                    self.position += 1;
                    return Ok(Json::Array(items));
                }
                loop {
                    items.push(self.value(depth + 1)?);
                    self.skip_space();
                    match self.bytes.get(self.position) {
                        Some(b',') => self.position += 1,
                        Some(b']') => {
                            self.position += 1;
                            return Ok(Json::Array(items));
                        }
                        _ => return self.error("expected , or ]"),
                    }
                }
            }
            Some(b'"') => self.string().map(Json::String),
            Some(b't') => self.expect(b"true").map(|()| Json::Bool(true)),
            Some(b'f') => self.expect(b"false").map(|()| Json::Bool(false)),
            Some(b'n') => self.expect(b"null").map(|()| Json::Null),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => self.error("expected a value"),
        }
    }

    fn number(&mut self) -> Result<Json, String> {
        let start = self.position;
        while let Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9') =
            self.bytes.get(self.position)
        {
            self.position += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.position]).unwrap_or_default();
        match text.parse::<f64>() {
            Ok(value) if value.is_finite() => Ok(Json::Number(value)),
            _ => self.error("bad number"),
        }
    }

    fn string(&mut self) -> Result<String, String> {
        if self.bytes.get(self.position) != Some(&b'"') {
            return self.error("expected a string");
        }
        self.position += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let Some(&byte) = self.bytes.get(self.position) else {
                return self.error("unterminated string");
            };
            self.position += 1;
            match byte {
                b'"' => break,
                b'\\' => {
                    let Some(&escape) = self.bytes.get(self.position) else {
                        return self.error("unterminated escape");
                    };
                    self.position += 1;
                    match escape {
                        b'"' => out.push(b'"'),
                        b'\\' => out.push(b'\\'),
                        b'/' => out.push(b'/'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'n' => out.push(b'\n'),
                        b'r' => out.push(b'\r'),
                        b't' => out.push(b'\t'),
                        b'u' => {
                            let mut code = self.hex4()?;
                            if (0xD800..0xDC00).contains(&code) {
                                // A surrogate pair.
                                self.expect(b"\\u")?;
                                let low = self.hex4()?;
                                if !(0xDC00..0xE000).contains(&low) {
                                    return self.error("bad surrogate pair");
                                }
                                code = 0x10000 + ((code - 0xD800) << 10) + (low - 0xDC00);
                            }
                            let character = char::from_u32(code).unwrap_or('\u{fffd}');
                            let mut buffer = [0; 4];
                            out.extend_from_slice(character.encode_utf8(&mut buffer).as_bytes());
                        }
                        _ => return self.error("bad escape"),
                    }
                }
                0..=0x1f => return self.error("control character in string"),
                _ => out.push(byte),
            }
        }
        String::from_utf8(out).or_else(|_| self.error("string is not UTF-8"))
    }

    fn hex4(&mut self) -> Result<u32, String> {
        let digits = self
            .bytes
            .get(self.position..self.position + 4)
            .and_then(|digits| std::str::from_utf8(digits).ok())
            .and_then(|digits| u32::from_str_radix(digits, 16).ok());
        match digits {
            Some(value) => {
                self.position += 4;
                Ok(value)
            }
            None => self.error("bad \\u escape"),
        }
    }
}

/// Parses one JSON document (UTF-8, optional byte-order mark).
pub fn parse(text: &[u8]) -> Result<Json, String> {
    let text = text.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(text);
    let mut parser = Parser {
        bytes: text,
        position: 0,
    };
    let value = parser.value(0)?;
    parser.skip_space();
    if parser.position != text.len() {
        return parser.error("trailing text");
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_documents() {
        let json = parse(
            r#"{"asset": {"version": "2.0"}, "list": [1, -2.5e2, true, false, null],
                "text": "tab\tquote\" é 😀", "empty": {}, "none": []}"#
                .as_bytes(),
        )
        .unwrap();
        assert_eq!(
            json.get("asset")
                .and_then(|asset| asset.get("version"))
                .and_then(Json::as_str),
            Some("2.0")
        );
        let list = json.get("list").unwrap();
        assert_eq!(list.index(1).and_then(Json::as_f64), Some(-250.0));
        assert_eq!(list.index(2).and_then(Json::as_bool), Some(true));
        assert_eq!(list.index(4), Some(&Json::Null));
        assert_eq!(
            json.get("text").and_then(Json::as_str),
            Some("tab\tquote\" é 😀")
        );
        assert_eq!(Json::Number(3.0).as_usize(), Some(3));
        assert_eq!(Json::Number(3.5).as_usize(), None);
        assert!(json.get("none").unwrap().as_array().is_empty());
    }

    #[test]
    fn rejects_bad_documents() {
        for bad in [
            &b"{"[..],
            b"[1,]",
            b"{\"a\" 1}",
            b"\"unterminated",
            b"nul",
            b"1 2",
            b"[\"\x01\"]",
            b"1e999",
        ] {
            assert!(parse(bad).is_err(), "{:?}", String::from_utf8_lossy(bad));
        }
        let deep = "[".repeat(200) + &"]".repeat(200);
        assert!(parse(deep.as_bytes()).is_err());
    }
}
