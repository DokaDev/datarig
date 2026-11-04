//! Text pasted into a result's cells, read as rows of values ([`parse`]): tab-separated,
//! comma-separated, one value per line, or one value.
//!
//! Nothing is guessed beyond the format: rows of different widths, a line end that is a lone
//! carriage return and a quote left open are refused, never padded or repaired. A value is the
//! text as pasted; only an unquoted field that is exactly the NULL token is NULL. Nothing read
//! here is a column's default.
//!
//! One final line end is the last line's, as spreadsheets end every line; an empty line before
//! it is a row (of one empty value, so one value per line keeps an empty last cell, and rows of
//! more values refuse it as of another width). Empty lines at the start and inside are rows of
//! empty values. A byte order mark at the start is not part of the text.

use super::sql::Value;

/// The most rows a paste may have.
pub const MAX_ROWS: usize = 5_000;
/// The most bytes of text a paste may have.
pub const MAX_BYTES: usize = 1 << 20;

/// How pasted text was read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// Tab-separated values, a row per line.
    Tsv,
    /// Comma-separated values, a row per line.
    Csv,
    /// One value per line: one column.
    Column,
    /// The whole text is one value.
    One,
}

impl Format {
    /// The format to read the same text as next: TSV, CSV, one value, and TSV again; one value
    /// per line reads as one value next.
    pub fn next(self) -> Self {
        match self {
            Self::Tsv => Self::Csv,
            Self::Csv => Self::One,
            Self::Column => Self::One,
            Self::One => Self::Tsv,
        }
    }
}

/// How to read a paste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PasteOpts {
    /// An unquoted field exactly this text is NULL (`\N` by default). An empty token is none: an
    /// empty field is always an empty text.
    pub null: String,
}

impl Default for PasteOpts {
    fn default() -> Self {
        Self { null: "\\N".into() }
    }
}

/// A paste read as rows of values: every row as wide as the first, each value a text or NULL.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pasted {
    pub format: Format,
    pub rows: Vec<Vec<Value>>,
}

impl Pasted {
    /// How many values a row has.
    pub fn width(&self) -> usize {
        self.rows.first().map_or(0, Vec::len)
    }
}

/// Why a paste cannot be read. Rows are counted from 1, as the text has them (a line break inside
/// quotes is in its row).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PasteError {
    /// There is nothing but line breaks.
    Empty,
    /// More than `max` bytes of text.
    TooLarge { bytes: usize, max: usize },
    /// More than `max` rows (`rows`: as many as were read before it stopped).
    TooManyRows { rows: usize, max: usize },
    /// Row `row` has a carriage return that ends no line (not followed by a line feed) outside
    /// quotes.
    LoneCr { row: usize },
    /// Row `row` has a quoted field that is never closed, or text right after its closing quote.
    Quote { row: usize },
    /// Row `row` has `values` values, and row `first_row` (the first with any) has
    /// `first_values`.
    Ragged { row: usize, values: usize, first_row: usize, first_values: usize },
}

/// Read `text` in the format it is in: TSV when it has a tab outside quotes, else CSV when its
/// first line (that is not empty) has a comma outside quotes, else one value per line when it
/// has a line break outside quotes (before its final line end), else one value. A quote left
/// open is refused ([`PasteError::Quote`]): what it hides is not known.
pub fn parse(text: &str, opts: &PasteOpts) -> Result<Pasted, PasteError> {
    let text = unmarked(text);
    if text.len() > MAX_BYTES {
        return Err(PasteError::TooLarge { bytes: text.len(), max: MAX_BYTES });
    }
    read(text, detect(text)?, opts)
}

/// Read `text` as `format`.
///
/// Line ends are `\n` or `\r\n` (outside quotes; inside quotes every character is the value's).
/// One final line end ends the last line; an empty line before it is a row of one empty value,
/// and an empty line before any other row is a row of empty values. TSV, CSV and one value per
/// line take quotes as RFC 4180 does: a field that starts with `"` runs to the next lone `"`,
/// with `""` for a quote, and may hold delimiters and line breaks; a quote anywhere else is a
/// character of the value. One value is the text without its final line end, its `\r\n` read
/// as `\n`; when it is one quoted field as a whole, the field's text, else the text as it is
/// (a quote left open too).
pub fn parse_as(text: &str, format: Format, opts: &PasteOpts) -> Result<Pasted, PasteError> {
    let text = unmarked(text);
    if text.len() > MAX_BYTES {
        return Err(PasteError::TooLarge { bytes: text.len(), max: MAX_BYTES });
    }
    read(text, format, opts)
}

/// `text` without one byte order mark at its start.
fn unmarked(text: &str) -> &str {
    text.strip_prefix('\u{FEFF}').unwrap_or(text)
}

/// `text` without one final line end.
fn unended(text: &str) -> &str {
    text.strip_suffix('\n').map_or(text, |t| t.strip_suffix('\r').unwrap_or(t))
}

/// Read `text` (of at most [`MAX_BYTES`]) as `format`.
fn read(text: &str, format: Format, opts: &PasteOpts) -> Result<Pasted, PasteError> {
    let null = |f: Field| match f {
        Field { text, quoted: false } if !opts.null.is_empty() && text == opts.null => Value::Null,
        Field { text, .. } => Value::Text(text),
    };
    let records = match format {
        Format::Tsv => records(text, Some('\t'))?,
        Format::Csv => records(text, Some(','))?,
        Format::Column => records(text, None)?,
        Format::One => vec![vec![one(text)?]],
    };
    let first = records.iter().position(|r| !is_blank(r)).ok_or(PasteError::Empty)?;
    let last = records.iter().rposition(|r| !is_blank(r)).unwrap_or(first);
    let width = records[first].len();
    let mut rows = Vec::with_capacity(records.len());
    for (i, r) in records.into_iter().enumerate() {
        if is_blank(&r) && i < last {
            rows.push(vec![Value::Text(String::new()); width]);
        } else if r.len() != width {
            return Err(PasteError::Ragged { row: i + 1, values: r.len(), first_row: first + 1, first_values: width });
        } else {
            rows.push(r.into_iter().map(null).collect());
        }
    }
    Ok(Pasted { format, rows })
}

/// A field as read: its text, and whether it was quoted.
struct Field {
    text: String,
    quoted: bool,
}

/// A record of an empty line.
fn is_blank(r: &[Field]) -> bool {
    matches!(r, [Field { text, quoted: false }] if text.is_empty())
}

/// The format [`parse`] reads `text` in. A quote left open hides the rest of the text, so what
/// it hides is not known: unless what comes before it names a format (which then refuses the
/// quote), it is refused.
fn detect(text: &str) -> Result<Format, PasteError> {
    let t = unended(text);
    if seen(t, Some('\t')).any {
        Ok(Format::Tsv)
    } else if seen(t.trim_start_matches(['\n', '\r']), Some(',')).first {
        Ok(Format::Csv)
    } else {
        let lines = seen(t, None);
        if lines.lines {
            Ok(Format::Column)
        } else if lines.open {
            Err(PasteError::Quote { row: 1 })
        } else {
            Ok(Format::One)
        }
    }
}

/// What [`seen`] found outside quotes.
#[derive(Default)]
struct Seen {
    /// The delimiter, on the first line.
    first: bool,
    /// The delimiter, anywhere.
    any: bool,
    /// A line break.
    lines: bool,
    /// A quote that is never closed (it hides the rest of the text).
    open: bool,
}

/// What `text`, split at `delim` with fields quoted as [`parse_as`] reads them, has outside
/// quotes.
fn seen(text: &str, delim: Option<char>) -> Seen {
    let mut out = Seen::default();
    let mut start = true;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if start && c == '"' {
            loop {
                match chars.next() {
                    None => return Seen { open: true, ..out },
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                    }
                    Some('"') => break,
                    Some(_) => {}
                }
            }
            start = false;
        } else if Some(c) == delim {
            out.any = true;
            out.first |= !out.lines;
            start = true;
        } else if c == '\n' {
            out.lines = true;
            start = true;
        } else {
            start = false;
        }
    }
    out
}

/// The records of `text`, its fields split at `delim` (none: one field per record); a line end
/// ends its record, so a final one adds none. It stops at the first record past [`MAX_ROWS`].
fn records(text: &str, delim: Option<char>) -> Result<Vec<Vec<Field>>, PasteError> {
    let mut out: Vec<Vec<Field>> = Vec::new();
    let mut row: Vec<Field> = Vec::new();
    let mut field = Field { text: String::new(), quoted: false };
    // At the start of a field, and of a record with nothing in it yet.
    let (mut start, mut fresh) = (true, true);
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        let at = out.len() + 1;
        fresh = false;
        if start && c == '"' {
            field.quoted = true;
            loop {
                match chars.next() {
                    None => return Err(PasteError::Quote { row: at }),
                    Some('"') if chars.peek() == Some(&'"') => {
                        chars.next();
                        field.text.push('"');
                    }
                    Some('"') => break,
                    Some(ch) => field.text.push(ch),
                }
            }
            match chars.peek() {
                None | Some('\n' | '\r') => {}
                Some(&d) if Some(d) == delim => {}
                Some(_) => return Err(PasteError::Quote { row: at }),
            }
            start = false;
            continue;
        }
        start = false;
        let end = match c {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                true
            }
            '\r' => return Err(PasteError::LoneCr { row: at }),
            '\n' => true,
            d if Some(d) == delim => {
                row.push(std::mem::replace(&mut field, Field { text: String::new(), quoted: false }));
                start = true;
                false
            }
            _ => {
                field.text.push(c);
                false
            }
        };
        if end {
            row.push(std::mem::replace(&mut field, Field { text: String::new(), quoted: false }));
            out.push(std::mem::take(&mut row));
            if out.len() > MAX_ROWS {
                return Err(PasteError::TooManyRows { rows: out.len(), max: MAX_ROWS });
            }
            (start, fresh) = (true, true);
        }
    }
    if !fresh {
        row.push(field);
        out.push(row);
        if out.len() > MAX_ROWS {
            return Err(PasteError::TooManyRows { rows: out.len(), max: MAX_ROWS });
        }
    }
    Ok(out)
}

/// The text as one value ([`Format::One`]).
fn one(text: &str) -> Result<Field, PasteError> {
    let s = unended(text);
    if s.is_empty() {
        return Err(PasteError::Empty);
    }
    let inner = s.strip_prefix('"').and_then(|t| t.strip_suffix('"'));
    if let Some(inner) = inner.filter(|i| i.split("\"\"").all(|p| !p.contains('"'))) {
        return Ok(Field { text: inner.replace("\"\"", "\""), quoted: true });
    }
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\r' && chars.peek() != Some(&'\n') {
            return Err(PasteError::LoneCr { row: 1 });
        }
    }
    Ok(Field { text: s.replace("\r\n", "\n"), quoted: false })
}

#[cfg(test)]
mod tests;
