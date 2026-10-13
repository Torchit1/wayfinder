//! Just enough JSON for flat request bodies and hand-built responses (keeps the binary small).

pub enum Val {
    Str(String),
    Num(i64),
    Bool(bool),
    Null,
}

impl Val {
    pub fn str(&self) -> Option<&str> {
        if let Val::Str(s) = self { Some(s) } else { None }
    }
    pub fn num(&self) -> Option<i64> {
        if let Val::Num(n) = self { Some(*n) } else { None }
    }
    pub fn truthy(&self) -> bool {
        matches!(self, Val::Bool(true))
    }
}

/// A parsed flat object, in source order (a handful of keys: a list beats a hash map on size).
#[derive(Default)]
pub struct Obj(Vec<(String, Val)>);

impl Obj {
    pub fn get(&self, k: &str) -> Option<&Val> {
        self.0.iter().find(|(key, _)| key == k).map(|(_, v)| v)
    }
}

/// Parse a flat object: `{"k": "v", "n": 1.5, "b": true}`. Nested values are rejected.
pub fn parse_object(s: &str) -> Option<Obj> {
    let mut p = Parser { b: s.as_bytes(), i: 0 };
    let mut out = Obj::default();
    p.ws();
    if s.trim().is_empty() {
        return Some(out);
    }
    p.eat(b'{')?;
    p.ws();
    if p.peek() == Some(b'}') {
        return Some(out);
    }
    loop {
        p.ws();
        let k = p.string()?;
        p.ws();
        p.eat(b':')?;
        p.ws();
        let v = p.value()?;
        out.0.push((k, v));
        p.ws();
        match p.next()? {
            b',' => continue,
            b'}' => return Some(out),
            _ => return None,
        }
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<u8> {
        self.b.get(self.i).copied()
    }
    fn next(&mut self) -> Option<u8> {
        let c = self.peek()?;
        self.i += 1;
        Some(c)
    }
    fn eat(&mut self, c: u8) -> Option<()> {
        (self.next()? == c).then_some(())
    }
    fn ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\r' | b'\n')) {
            self.i += 1;
        }
    }
    fn value(&mut self) -> Option<Val> {
        match self.peek()? {
            b'"' => Some(Val::Str(self.string()?)),
            b't' => self.word("true", Val::Bool(true)),
            b'f' => self.word("false", Val::Bool(false)),
            b'n' => self.word("null", Val::Null),
            _ => {
                let start = self.i;
                while matches!(self.peek(), Some(b'-' | b'+' | b'.' | b'e' | b'E' | b'0'..=b'9')) {
                    self.i += 1;
                }
                int(std::str::from_utf8(&self.b[start..self.i]).ok()?).map(Val::Num)
            }
        }
    }
    fn word(&mut self, w: &str, v: Val) -> Option<Val> {
        self.b[self.i..].starts_with(w.as_bytes()).then(|| {
            self.i += w.len();
            v
        })
    }
    fn string(&mut self) -> Option<String> {
        self.eat(b'"')?;
        let mut out: Vec<u16> = Vec::new();
        loop {
            let start = self.i;
            while !matches!(self.peek()?, b'"' | b'\\') {
                self.i += 1;
            }
            out.extend(std::str::from_utf8(&self.b[start..self.i]).ok()?.encode_utf16());
            if self.next()? == b'"' {
                return Some(String::from_utf16_lossy(&out));
            }
            let c = self.next()?;
            out.push(match c {
                b'n' => 10,
                b't' => 9,
                b'r' => 13,
                b'b' => 8,
                b'f' => 12,
                b'u' => {
                    let hex = std::str::from_utf8(self.b.get(self.i..self.i + 4)?).ok()?;
                    self.i += 4;
                    u16::from_str_radix(hex, 16).ok()?
                }
                other => other as u16,
            });
        }
    }
}

/// Integer part of a decimal like "-12.7" (coordinates and seconds are whole numbers; skipping
/// float parsing/printing keeps the binary small).
pub fn int(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, digits) = s.strip_prefix('-').map_or((false, s), |d| (true, d));
    let whole = digits.split(['.', 'e', 'E']).next()?;
    if whole.is_empty() || !whole.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v = whole.bytes().try_fold(0i64, |a, c| a.checked_mul(10)?.checked_add((c - b'0') as i64))?;
    Some(if neg { -v } else { v })
}

/// JSON string literal, quotes included.
pub fn q(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}
