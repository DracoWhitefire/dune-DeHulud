//! A small, strict JSON reader (the subset of rules Python's `json` module applies) and string escaping.
//!
//! Used for the window config file, which is shared with the Python version: a file that Python's
//! `json.load` would refuse is refused here too, so both versions fall back to the same defaults.

/// A parsed JSON value. Integers that do not fit an i64 are `Int(None)` (valid JSON, out of any range).
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    Int(Option<i64>),
    Float(f64),
    Str(String),
    Array(Vec<Value>),
    Object(Vec<(String, Value)>),
}

impl Value {
    /// The value of `key` in an object (the last one when a key repeats, as in Python), else None.
    pub fn get(&self, key: &str) -> Option<&Value> {
        match self {
            Value::Object(items) => items.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }
}

/// The text is not a JSON document (or one Python refuses).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Invalid;

/// Nesting deeper than this is refused (Python refuses deep nesting with a RecursionError).
const MAX_DEPTH: usize = 500;
/// Python refuses integer literals longer than this many digits.
const MAX_INT_DIGITS: usize = 4300;

struct Parser<'a> {
    s: &'a [u8],
    i: usize,
}

/// Parses a whole JSON document (surrounding whitespace allowed, nothing else).
pub fn parse(text: &str) -> Result<Value, Invalid> {
    let mut p = Parser { s: text.as_bytes(), i: 0 };
    p.ws();
    let v = p.value(0).map_err(|()| Invalid)?;
    p.ws();
    if p.i != p.s.len() {
        return Err(Invalid);
    }
    Ok(v)
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && matches!(self.s[self.i], b' ' | b'\t' | b'\n' | b'\r') {
            self.i += 1;
        }
    }

    fn eat(&mut self, lit: &str) -> bool {
        if self.s[self.i..].starts_with(lit.as_bytes()) {
            self.i += lit.len();
            true
        } else {
            false
        }
    }

    fn value(&mut self, depth: usize) -> Result<Value, ()> {
        if depth > MAX_DEPTH {
            return Err(());
        }
        match self.s.get(self.i) {
            Some(b'{') => self.object(depth),
            Some(b'[') => self.array(depth),
            Some(b'"') => self.string().map(Value::Str),
            Some(_) if self.eat("null") => Ok(Value::Null),
            Some(_) if self.eat("true") => Ok(Value::Bool(true)),
            Some(_) if self.eat("false") => Ok(Value::Bool(false)),
            Some(_) if self.eat("NaN") => Ok(Value::Float(f64::NAN)),
            Some(_) if self.eat("Infinity") => Ok(Value::Float(f64::INFINITY)),
            Some(_) if self.eat("-Infinity") => Ok(Value::Float(f64::NEG_INFINITY)),
            Some(b'-' | b'0'..=b'9') => self.number(),
            _ => Err(()),
        }
    }

    fn object(&mut self, depth: usize) -> Result<Value, ()> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.eat("}") {
            return Ok(Value::Object(items));
        }
        loop {
            self.ws();
            if self.s.get(self.i) != Some(&b'"') {
                return Err(());
            }
            let key = self.string()?;
            self.ws();
            if !self.eat(":") {
                return Err(());
            }
            self.ws();
            items.push((key, self.value(depth + 1)?));
            self.ws();
            if self.eat("}") {
                return Ok(Value::Object(items));
            }
            if !self.eat(",") {
                return Err(());
            }
        }
    }

    fn array(&mut self, depth: usize) -> Result<Value, ()> {
        self.i += 1;
        let mut items = Vec::new();
        self.ws();
        if self.eat("]") {
            return Ok(Value::Array(items));
        }
        loop {
            self.ws();
            items.push(self.value(depth + 1)?);
            self.ws();
            if self.eat("]") {
                return Ok(Value::Array(items));
            }
            if !self.eat(",") {
                return Err(());
            }
        }
    }

    fn digits(&mut self) -> usize {
        let start = self.i;
        while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
            self.i += 1;
        }
        self.i - start
    }

    fn number(&mut self) -> Result<Value, ()> {
        let start = self.i;
        self.eat("-");
        let int_start = self.i;
        if self.eat("0") {
            // a leading zero stands alone
        } else if self.digits() == 0 {
            return Err(());
        }
        let int_digits = self.i - int_start;
        let mut float = false;
        if self.s.get(self.i) == Some(&b'.') && self.s.get(self.i + 1).is_some_and(u8::is_ascii_digit) {
            self.i += 1;
            self.digits();
            float = true;
        }
        if matches!(self.s.get(self.i), Some(b'e' | b'E')) {
            let save = self.i;
            self.i += 1;
            if matches!(self.s.get(self.i), Some(b'+' | b'-')) {
                self.i += 1;
            }
            if self.digits() == 0 {
                self.i = save; // not an exponent: the document then fails on the leftover text
            } else {
                float = true;
            }
        }
        let text = std::str::from_utf8(&self.s[start..self.i]).map_err(|_| ())?;
        if float {
            return Ok(Value::Float(text.parse().map_err(|_| ())?));
        }
        if int_digits > MAX_INT_DIGITS {
            return Err(());
        }
        Ok(Value::Int(text.parse().ok()))
    }

    fn hex4(&mut self) -> Result<u32, ()> {
        let h = self.s.get(self.i..self.i + 4).ok_or(())?;
        let h = std::str::from_utf8(h).map_err(|_| ())?;
        if !h.bytes().all(|c| c.is_ascii_hexdigit()) {
            return Err(());
        }
        self.i += 4;
        u32::from_str_radix(h, 16).map_err(|_| ())
    }

    fn string(&mut self) -> Result<String, ()> {
        self.i += 1;
        let mut out: Vec<u8> = Vec::new();
        loop {
            let c = *self.s.get(self.i).ok_or(())?;
            self.i += 1;
            match c {
                b'"' => return String::from_utf8(out).map_err(|_| ()),
                0..=0x1f => return Err(()), // control characters must be escaped
                b'\\' => {
                    let e = *self.s.get(self.i).ok_or(())?;
                    self.i += 1;
                    let ch = match e {
                        b'"' => '"',
                        b'\\' => '\\',
                        b'/' => '/',
                        b'b' => '\u{8}',
                        b'f' => '\u{c}',
                        b'n' => '\n',
                        b'r' => '\r',
                        b't' => '\t',
                        b'u' => {
                            let mut u = self.hex4()?;
                            if (0xD800..0xDC00).contains(&u) && self.s[self.i..].starts_with(b"\\u") {
                                let save = self.i;
                                self.i += 2;
                                let lo = self.hex4()?;
                                if (0xDC00..0xE000).contains(&lo) {
                                    u = 0x10000 + ((u - 0xD800) << 10) + (lo - 0xDC00);
                                } else {
                                    self.i = save;
                                }
                            }
                            char::from_u32(u).unwrap_or('\u{fffd}') // a lone surrogate: kept by Python, replaced here
                        }
                        _ => return Err(()),
                    };
                    let mut buf = [0u8; 4];
                    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
                }
                _ => out.push(c),
            }
        }
    }
}

/// A JSON string literal for `s`.
pub fn quote(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_what_python_accepts() {
        assert_eq!(parse(" {\"x\": 10, \"y\": -0, \"f\": true} ").unwrap().get("x"), Some(&Value::Int(Some(10))));
        assert_eq!(parse("{\"x\": 1, \"x\": 2}").unwrap().get("x"), Some(&Value::Int(Some(2))));
        assert_eq!(parse("1e309").unwrap(), Value::Float(f64::INFINITY));
        assert_eq!(parse("99999999999999999999999").unwrap(), Value::Int(None));
        assert_eq!(parse("\"a\\u00e9\\ud83d\\ude00\\n\"").unwrap(), Value::Str("aé😀\n".into()));
        assert!(matches!(parse("[NaN, Infinity, -Infinity, 1.5e-3, 0.0]").unwrap(), Value::Array(v) if v.len() == 5));
    }

    #[test]
    fn refuses_what_python_refuses() {
        for bad in [
            "{",
            "",
            "01",
            "1.",
            ".5",
            "-",
            "+1",
            "{\"a\" 1}",
            "{'a': 1}",
            "[1,]",
            "{\"a\":1,}",
            "tru",
            "1 2",
            "\"\t\"",
            "\"\\x\"",
            "\u{feff}{}",
            "[1e]",
            "nan",
        ] {
            assert!(parse(bad).is_err(), "{bad:?}");
        }
        assert!(parse(&"[".repeat(100000)).is_err()); // no stack overflow
        assert!(parse(&"9".repeat(4301)).is_err());
        assert!(parse(&"9".repeat(4300)).is_ok());
    }

    #[test]
    fn quoting() {
        assert_eq!(quote("a\"b\\c\nd\u{1}"), "\"a\\\"b\\\\c\\nd\\u0001\"");
        assert_eq!(parse(&quote("x\ty\"z")).unwrap(), Value::Str("x\ty\"z".into()));
    }
}
