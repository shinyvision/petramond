//! A minimal JSON reader for the row-data interop surface.
//!
//! Item/block consumer-data entries ([`crate::item_data`],
//! [`crate::items_with_data`] and the block twins) cross the ABI as RAW JSON
//! TEXT — the catalogs' native substance — so a consuming mod needs to read
//! JSON without hauling a full serde stack into its wasm. This is a small
//! strict recursive-descent parser producing a [`Value`] tree with the
//! accessor helpers an interop consumer actually needs (`get`, `as_*`).
//! Parsing is deterministic and allocation-bounded by the input (which the
//! engine caps at load); malformed input is `None` — the consumer's contract
//! is "parse what you understand, ignore the rest".

/// One parsed JSON value. Object fields keep declaration order.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// All JSON numbers, as f64 (the data surface carries row metadata, not
    /// precision-critical integers).
    Num(f64),
    Str(String),
    Arr(Vec<Value>),
    Obj(Vec<(String, Value)>),
}

impl Value {
    /// Parse a complete JSON document (trailing garbage is an error).
    pub fn parse(text: &str) -> Option<Value> {
        let bytes = text.as_bytes();
        let mut i = 0;
        let v = parse_value(bytes, &mut i)?;
        skip_ws(bytes, &mut i);
        (i == bytes.len()).then_some(v)
    }

    /// Object field `key`, or `None` (also for non-objects).
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Obj(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Num(n) => Some(*n),
            _ => None,
        }
    }

    /// The value as a u8, if it is a number representing one exactly.
    pub fn as_u8(&self) -> Option<u8> {
        let n = self.as_f64()?;
        (n.fract() == 0.0 && (0.0..=255.0).contains(&n)).then_some(n as u8)
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Value::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Value::Str(s) => Some(s),
            _ => None,
        }
    }

    pub fn as_array(&self) -> Option<&[Value]> {
        match self {
            Value::Arr(items) => Some(items),
            _ => None,
        }
    }

    /// The object's fields in declaration order, or `None` for a non-object —
    /// for consumers that treat an object as a MAP with unknown keys (a
    /// per-family art table) rather than fetching named fields.
    pub fn as_object(&self) -> Option<&[(String, Value)]> {
        match self {
            Value::Obj(fields) => Some(fields),
            _ => None,
        }
    }
}

fn skip_ws(b: &[u8], i: &mut usize) {
    while *i < b.len() && matches!(b[*i], b' ' | b'\t' | b'\n' | b'\r') {
        *i += 1;
    }
}

fn eat(b: &[u8], i: &mut usize, c: u8) -> Option<()> {
    skip_ws(b, i);
    (*i < b.len() && b[*i] == c).then(|| *i += 1)
}

fn parse_value(b: &[u8], i: &mut usize) -> Option<Value> {
    skip_ws(b, i);
    match *b.get(*i)? {
        b'n' => parse_lit(b, i, b"null", Value::Null),
        b't' => parse_lit(b, i, b"true", Value::Bool(true)),
        b'f' => parse_lit(b, i, b"false", Value::Bool(false)),
        b'"' => parse_string(b, i).map(Value::Str),
        b'[' => {
            *i += 1;
            let mut items = Vec::new();
            skip_ws(b, i);
            if b.get(*i) == Some(&b']') {
                *i += 1;
                return Some(Value::Arr(items));
            }
            loop {
                items.push(parse_value(b, i)?);
                skip_ws(b, i);
                match b.get(*i)? {
                    b',' => *i += 1,
                    b']' => {
                        *i += 1;
                        return Some(Value::Arr(items));
                    }
                    _ => return None,
                }
            }
        }
        b'{' => {
            *i += 1;
            let mut fields = Vec::new();
            skip_ws(b, i);
            if b.get(*i) == Some(&b'}') {
                *i += 1;
                return Some(Value::Obj(fields));
            }
            loop {
                skip_ws(b, i);
                let key = parse_string(b, i)?;
                eat(b, i, b':')?;
                fields.push((key, parse_value(b, i)?));
                skip_ws(b, i);
                match b.get(*i)? {
                    b',' => *i += 1,
                    b'}' => {
                        *i += 1;
                        return Some(Value::Obj(fields));
                    }
                    _ => return None,
                }
            }
        }
        _ => parse_number(b, i),
    }
}

fn parse_lit(b: &[u8], i: &mut usize, lit: &[u8], v: Value) -> Option<Value> {
    b[*i..].starts_with(lit).then(|| {
        *i += lit.len();
        v
    })
}

fn parse_number(b: &[u8], i: &mut usize) -> Option<Value> {
    let start = *i;
    if b.get(*i) == Some(&b'-') {
        *i += 1;
    }
    while *i < b.len() && matches!(b[*i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
        *i += 1;
    }
    (*i > start)
        .then(|| std::str::from_utf8(&b[start..*i]).ok()?.parse().ok())
        .flatten()
        .map(Value::Num)
}

fn parse_string(b: &[u8], i: &mut usize) -> Option<String> {
    if b.get(*i) != Some(&b'"') {
        return None;
    }
    *i += 1;
    let mut out = String::new();
    loop {
        match *b.get(*i)? {
            b'"' => {
                *i += 1;
                return Some(out);
            }
            b'\\' => {
                *i += 1;
                match *b.get(*i)? {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => {
                        let hex = b.get(*i + 1..*i + 5)?;
                        let code = u32::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?;
                        // Surrogate pairs are out of scope for row metadata;
                        // a lone surrogate reads as the replacement char.
                        out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                        *i += 4;
                    }
                    _ => return None,
                }
                *i += 1;
            }
            _ => {
                // Consume one UTF-8 scalar (multi-byte sequences pass through).
                let rest = std::str::from_utf8(&b[*i..]).ok()?;
                let c = rest.chars().next()?;
                out.push(c);
                *i += c.len_utf8();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Value;

    #[test]
    fn parses_the_interop_shapes() {
        let v = Value::parse(r#"{"color":[250,224,28],"dilute":true,"name":"a\"b"}"#).unwrap();
        let color: Vec<u8> = v
            .get("color")
            .unwrap()
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_u8().unwrap())
            .collect();
        assert_eq!(color, [250, 224, 28]);
        assert_eq!(v.get("dilute").unwrap().as_bool(), Some(true));
        assert_eq!(v.get("name").unwrap().as_str(), Some("a\"b"));
        assert_eq!(v.get("absent"), None);
        assert_eq!(Value::parse("-2.5e2").unwrap().as_f64(), Some(-250.0));
        assert!(Value::parse("{\"unterminated\":").is_none());
        assert!(Value::parse("[1,2] trailing").is_none());
        assert_eq!(Value::parse("256").unwrap().as_u8(), None);
    }
}
