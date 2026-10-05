//! A JSON reader for plans: every value in one arena, read without recursion, so a plan nested
//! as deeply as the server allows neither hits a recursion limit nor overflows the stack (and
//! dropping it frees one vector). Numbers keep their text as the server wrote it (`2932.50`),
//! because the psql-style text prints them that way.

/// A value of the arena. Arrays and objects refer to their items by index.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Bool(bool),
    /// The number as written, and its value.
    Number(String, f64),
    String(String),
    Array(Vec<usize>),
    Object(Vec<(String, usize)>),
}

/// A parsed document: `values[root]` is its top value.
#[derive(Clone, Debug, Default)]
pub struct Doc {
    pub values: Vec<Value>,
    pub root: usize,
}

/// Why a text is not JSON (byte offset).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JsonError(pub usize);

/// At most this many values: a plan of a hundred thousand nodes has fewer than this many.
pub const MAX_VALUES: usize = 20_000_000;

impl Doc {
    pub fn get(&self, i: usize) -> &Value {
        &self.values[i]
    }

    /// Member `key` of object `i`.
    pub fn field(&self, i: usize, key: &str) -> Option<usize> {
        match &self.values[i] {
            Value::Object(m) => m.iter().find(|(k, _)| k == key).map(|(_, v)| *v),
            _ => None,
        }
    }

    pub fn str(&self, i: usize) -> Option<&str> {
        match &self.values[i] {
            Value::String(s) => Some(s),
            _ => None,
        }
    }

    pub fn num(&self, i: usize) -> Option<f64> {
        match &self.values[i] {
            Value::Number(_, n) => Some(*n),
            _ => None,
        }
    }

    pub fn items(&self, i: usize) -> &[usize] {
        match &self.values[i] {
            Value::Array(v) => v,
            _ => &[],
        }
    }

    pub fn members(&self, i: usize) -> &[(String, usize)] {
        match &self.values[i] {
            Value::Object(m) => m,
            _ => &[],
        }
    }

    /// Value `i` as text: a string as it is, a number as written, `true`/`false`; an array
    /// of scalars joined with `, `; anything else `None`.
    pub fn scalar_text(&self, i: usize) -> Option<String> {
        match &self.values[i] {
            Value::String(s) => Some(s.clone()),
            Value::Number(t, _) => Some(t.clone()),
            Value::Bool(b) => Some(b.to_string()),
            Value::Null => None,
            Value::Array(items) => {
                let parts: Option<Vec<String>> = items
                    .iter()
                    .map(|&v| match self.values[v] {
                        Value::Array(_) | Value::Object(_) | Value::Null => None,
                        _ => self.scalar_text(v),
                    })
                    .collect();
                parts.map(|p| p.join(", "))
            }
            Value::Object(_) => None,
        }
    }
}

/// An array or object being read: where it is in the arena, and an object's pending key.
enum Open {
    Array(usize),
    Object(usize, Option<String>),
}

/// Parse `text` (one JSON value, surrounded by whitespace at most).
pub fn parse(text: &str) -> Result<Doc, JsonError> {
    let b = text.as_bytes();
    let mut doc = Doc::default();
    let mut stack: Vec<Open> = Vec::new();
    let mut i = 0;
    let root: Option<usize>;
    loop {
        i = skip_ws(b, i);
        // Inside an object, a key comes first (or the end).
        if let Some(Open::Object(at, key @ None)) = stack.last_mut() {
            let at = *at;
            if b.get(i) == Some(&b'}') {
                // `{}`; after a `,` another member must follow.
                if !doc.members(at).is_empty() {
                    return Err(JsonError(i));
                }
                i += 1;
                stack.pop();
                if let Some(done) = close(&mut stack, &mut i, b)? {
                    root = Some(done.unwrap_or(at));
                    break;
                }
                continue;
            }
            let (k, next) = string(b, i)?;
            i = skip_ws(b, next);
            if b.get(i) != Some(&b':') {
                return Err(JsonError(i));
            }
            i = skip_ws(b, i + 1);
            *key = Some(k);
        }
        // A value.
        let at = doc.values.len();
        if at >= MAX_VALUES {
            return Err(JsonError(i));
        }
        let opened = match b.get(i) {
            Some(b'{') => {
                i += 1;
                doc.values.push(Value::Object(Vec::new()));
                Some(Open::Object(at, None))
            }
            Some(b'[') => {
                i = skip_ws(b, i + 1);
                doc.values.push(Value::Array(Vec::new()));
                if b.get(i) == Some(&b']') {
                    i += 1;
                    None
                } else {
                    Some(Open::Array(at))
                }
            }
            Some(b'"') => {
                let (s, next) = string(b, i)?;
                i = next;
                doc.values.push(Value::String(s));
                None
            }
            Some(b't') if b[i..].starts_with(b"true") => {
                i += 4;
                doc.values.push(Value::Bool(true));
                None
            }
            Some(b'f') if b[i..].starts_with(b"false") => {
                i += 5;
                doc.values.push(Value::Bool(false));
                None
            }
            Some(b'n') if b[i..].starts_with(b"null") => {
                i += 4;
                doc.values.push(Value::Null);
                None
            }
            Some(c) if *c == b'-' || c.is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < b.len() && matches!(b[i], b'0'..=b'9' | b'.' | b'e' | b'E' | b'+' | b'-') {
                    i += 1;
                }
                let t = &text[start..i];
                let n: f64 = t.parse().map_err(|_| JsonError(start))?;
                doc.values.push(Value::Number(t.to_string(), n));
                None
            }
            _ => return Err(JsonError(i)),
        };
        // Attach it to the array or object it is in.
        match stack.last_mut() {
            Some(Open::Array(arr)) => {
                let arr = *arr;
                if let Value::Array(v) = &mut doc.values[arr] {
                    v.push(at);
                }
            }
            Some(Open::Object(obj, key)) => {
                let (obj, k) = (*obj, key.take().unwrap_or_default());
                if let Value::Object(m) = &mut doc.values[obj] {
                    m.push((k, at));
                }
            }
            None => {}
        }
        if let Some(o) = opened {
            stack.push(o);
            continue;
        }
        match stack.is_empty() {
            true => {
                root = Some(at);
                break;
            }
            false => {
                if let Some(done) = close(&mut stack, &mut i, b)? {
                    root = Some(done.unwrap_or(at));
                    break;
                }
            }
        }
    }
    if skip_ws(b, i) != b.len() {
        return Err(JsonError(i));
    }
    doc.root = root.unwrap_or(0);
    Ok(doc)
}

/// After a value inside the arrays and objects of `stack`: a `,` (another value follows), or
/// the ends of as many of them as end here. `Some(top)` once the outermost one ended: the
/// document is complete (`top` is its index).
fn close(stack: &mut Vec<Open>, i: &mut usize, b: &[u8]) -> Result<Option<Option<usize>>, JsonError> {
    loop {
        *i = skip_ws(b, *i);
        let Some(top) = stack.last() else { return Ok(Some(None)) };
        let (end, at) = match top {
            Open::Array(at) => (b']', *at),
            Open::Object(at, _) => (b'}', *at),
        };
        match b.get(*i) {
            Some(b',') => {
                *i += 1;
                return Ok(None);
            }
            Some(c) if *c == end => {
                *i += 1;
                stack.pop();
                if stack.is_empty() {
                    return Ok(Some(Some(at)));
                }
            }
            _ => return Err(JsonError(*i)),
        }
    }
}

fn skip_ws(b: &[u8], mut i: usize) -> usize {
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    i
}

/// The string starting at `b[i]` (a `"`), unescaped, and the index after it.
fn string(b: &[u8], i: usize) -> Result<(String, usize), JsonError> {
    if b.get(i) != Some(&b'"') {
        return Err(JsonError(i));
    }
    let mut out: Vec<u8> = Vec::new();
    let mut j = i + 1;
    loop {
        match b.get(j) {
            None => return Err(JsonError(j)),
            Some(b'"') => break,
            Some(b'\\') => {
                let esc = *b.get(j + 1).ok_or(JsonError(j))?;
                j += 2;
                match esc {
                    b'"' => out.push(b'"'),
                    b'\\' => out.push(b'\\'),
                    b'/' => out.push(b'/'),
                    b'b' => out.push(8),
                    b'f' => out.push(12),
                    b'n' => out.push(b'\n'),
                    b'r' => out.push(b'\r'),
                    b't' => out.push(b'\t'),
                    b'u' => {
                        let hi = hex4(b, j)?;
                        j += 4;
                        let c = if (0xD800..0xDC00).contains(&hi)
                            && b.get(j) == Some(&b'\\')
                            && b.get(j + 1) == Some(&b'u')
                        {
                            let lo = hex4(b, j + 2)?;
                            if (0xDC00..0xE000).contains(&lo) {
                                j += 6;
                                char::from_u32(0x10000 + ((hi - 0xD800) << 10) + (lo - 0xDC00))
                            } else {
                                None
                            }
                        } else {
                            char::from_u32(hi)
                        };
                        let mut buf = [0; 4];
                        out.extend_from_slice(c.unwrap_or('\u{FFFD}').encode_utf8(&mut buf).as_bytes());
                    }
                    _ => return Err(JsonError(j - 1)),
                }
            }
            Some(&c) if c < 0x20 => return Err(JsonError(j)),
            Some(&c) => {
                out.push(c);
                j += 1;
            }
        }
    }
    // The input is UTF-8 and only whole escapes were replaced.
    let s = String::from_utf8(out).map_err(|_| JsonError(i))?;
    Ok((s, j + 1))
}

fn hex4(b: &[u8], i: usize) -> Result<u32, JsonError> {
    let s = b.get(i..i + 4).ok_or(JsonError(i))?;
    let s = std::str::from_utf8(s).map_err(|_| JsonError(i))?;
    u32::from_str_radix(s, 16).map_err(|_| JsonError(i))
}
